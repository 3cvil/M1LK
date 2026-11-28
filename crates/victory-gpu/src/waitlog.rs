//! (M1LK Wait Handle) What the title's presenting thread waits on in native synchronisation
//! calls, off by default (`VICTORY_WAIT_HANDLE=1`, with `VICTORY_CRIT_PATH=1`).
//!
//! The title's imports of `WaitForSingleObject` and the like resolve to wrappers in
//! `victory-hle` (installed only with the switch on) that time each call on the presenting
//! thread and record it here: the handle, how long, and a few return addresses. A handle is
//! `fence` when it is an event we registered with one of our fences (`fence.rs`), `title`
//! when the title made it through its own `CreateEvent`/`CreateSemaphore` import, and
//! `other` otherwise (made by our own code, the system or the title through a path we do not
//! see). Each distinct handle also gets its kernel object type (event, thread, file, ...).
//!
//! Cost: with the switch off, nothing runs (no wrapper installed). On: a lock and a hash map
//! update per wait of the one thread, a few dozen waits a frame at most.

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};

/// `VICTORY_WAIT_HANDLE=1`.
pub fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("VICTORY_WAIT_HANDLE").is_ok_and(|v| v.trim() == "1"))
}

/// Whether the calling thread is the one that presents (and the profile is on).
pub fn on_critical_thread() -> bool {
    enabled() && crate::prof::is_crit()
}

/// What a wait was in.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Kind {
    Single,
    Multiple,
    Sleep,
    Address,
    Condition,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::Single => "wait1",
            Kind::Multiple => "waitN",
            Kind::Sleep => "sleep",
            Kind::Address => "address",
            Kind::Condition => "condvar",
        }
    }
}

#[derive(Default)]
struct Entry {
    ns: u64,
    waits: u64,
    callers: [usize; 3],
    kind_type: String,
}

#[derive(Default)]
struct State {
    waits: HashMap<(Kind, usize), Entry>,
    /// Handles the title made with its own imports, and events we gave our fences.
    title: HashSet<usize>,
    fence: HashSet<usize>,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);
static LOCATE: OnceLock<fn(usize) -> String> = OnceLock::new();

fn with<R>(f: impl FnOnce(&mut State) -> R) -> R {
    let mut guard = STATE.lock().unwrap_or_else(|e| e.into_inner());
    f(guard.get_or_insert_with(State::default))
}

/// How return addresses are named in the line (`module+rva`).
pub fn set_locator(f: fn(usize) -> String) {
    let _ = LOCATE.set(f);
}

/// The title made `handle` with its own import.
pub fn note_title(handle: usize) {
    if enabled() && handle != 0 {
        with(|s| s.title.insert(handle));
    }
}

/// `handle` is an event we set from one of our fences.
pub fn note_fence(handle: usize) {
    if enabled() && handle != 0 {
        with(|s| s.fence.insert(handle));
    }
}

/// The presenting thread waited `ns` in a wait of `kind` on `handle` (the first of its
/// handles, or an address, or 0), called from `callers`.
pub fn record(kind: Kind, handle: usize, ns: u64, callers: [usize; 3]) {
    let new = with(|s| {
        let e = s.waits.entry((kind, handle)).or_default();
        e.ns += ns;
        e.waits += 1;
        e.callers = callers;
        e.kind_type.is_empty() && matches!(kind, Kind::Single | Kind::Multiple)
    });
    if new {
        let ty = object_type(handle);
        with(|s| {
            if let Some(e) = s.waits.get_mut(&(kind, handle)) {
                e.kind_type = ty;
            }
        });
    }
}

/// The kernel object type of `handle` ("Event", "Thread", ...), "?" if it cannot be asked.
fn object_type(handle: usize) -> String {
    use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress};
    type Query = unsafe extern "system" fn(*mut std::ffi::c_void, i32, *mut std::ffi::c_void, u32, *mut u32) -> i32;
    // SAFETY: ntdll is loaded in every process; the exported function has this signature.
    let query: Option<Query> = unsafe {
        let module = GetModuleHandleA(c"ntdll.dll".as_ptr().cast());
        GetProcAddress(module, c"NtQueryObject".as_ptr().cast()).map(|f| std::mem::transmute::<_, Query>(f))
    };
    let Some(query) = query else { return "?".into() };
    let mut buffer = [0u64; 128];
    let mut length = 0u32;
    // SAFETY: an out buffer of 1 KiB; class 2 is ObjectTypeInformation, which starts with
    // a UNICODE_STRING (length, maximum length, buffer pointer) naming the type.
    let status = unsafe {
        query(handle as *mut _, 2, buffer.as_mut_ptr().cast(), std::mem::size_of_val(&buffer) as u32, &mut length)
    };
    if status < 0 {
        return "?".into();
    }
    let chars = u16::from_ne_bytes([buffer[0].to_ne_bytes()[0], buffer[0].to_ne_bytes()[1]]) as usize / 2;
    let text = buffer[1] as usize as *const u16;
    if text.is_null() || chars == 0 || chars > 64 {
        return "?".into();
    }
    // SAFETY: the pointer names `chars` UTF-16 units inside our buffer.
    String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(text, chars) })
}

/// The owner label of a wait.
fn owner(s: &State, kind: Kind, handle: usize) -> &'static str {
    match kind {
        Kind::Single | Kind::Multiple if s.fence.contains(&handle) => "fence",
        Kind::Single | Kind::Multiple if s.title.contains(&handle) => "title",
        Kind::Single | Kind::Multiple => "other",
        // Sleeps and address waits are the title's own code.
        _ => "title-code",
    }
}

/// The ranking of waits by time: the totals by owner (ns) and the top `n` rows.
pub(crate) fn rank(rows: &[(&'static str, u64)]) -> Vec<(&'static str, u64)> {
    let mut totals: Vec<(&'static str, u64)> = Vec::new();
    for &(who, ns) in rows {
        match totals.iter_mut().find(|t| t.0 == who) {
            Some(t) => t.1 += ns,
            None => totals.push((who, ns)),
        }
    }
    totals
}

/// The line over `frames` presents; takes the sums.
pub(crate) fn line(frames: f64) {
    if !enabled() || frames <= 0.0 {
        return;
    }
    let locate = |a: usize| LOCATE.get().map_or_else(|| format!("{a:#x}"), |f| f(a));
    let waits = with(|s| {
        let waits = std::mem::take(&mut s.waits);
        waits
            .into_iter()
            .map(|((kind, handle), e)| (kind, handle, owner(s, kind, handle), e))
            .collect::<Vec<_>>()
    });
    let ms = |ns: u64| ns as f64 / 1_000_000.0 / frames;
    let totals = rank(&waits.iter().map(|w| (w.2, w.3.ns)).collect::<Vec<_>>());
    let total = |who: &str| totals.iter().find(|t| t.0 == who).map_or(0, |t| t.1);
    let mut order: Vec<_> = waits.iter().collect();
    order.sort_by_key(|w| std::cmp::Reverse(w.3.ns));
    let mut text = String::new();
    for (i, (kind, handle, who, e)) in order.iter().take(3).enumerate() {
        use std::fmt::Write;
        let _ = write!(
            text,
            " | top{}={:#x} {:.2}ms owner={who} type={} kind={} waits={:.1} caller={}",
            i + 1,
            handle,
            ms(e.ns),
            if e.kind_type.is_empty() { "-" } else { &e.kind_type },
            kind.name(),
            e.waits as f64 / frames,
            e.callers.iter().filter(|&&a| a != 0).map(|&a| locate(a)).collect::<Vec<_>>().join(" < "),
        );
    }
    tracing::info!(
        "M1LK Wait Handle ms/frame: fence={:.2} title={:.2} other={:.2} title_code={:.2}{text}",
        ms(total("fence")),
        ms(total("title")),
        ms(total("other")),
        ms(total("title-code")),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rank_sums_by_owner() {
        let t = rank(&[("fence", 5), ("title", 7), ("fence", 3)]);
        assert_eq!(t, vec![("fence", 8), ("title", 7)]);
    }

    #[test]
    fn owners_by_registry() {
        let mut s = State::default();
        s.fence.insert(4);
        s.title.insert(8);
        assert_eq!(owner(&s, Kind::Single, 4), "fence");
        assert_eq!(owner(&s, Kind::Multiple, 8), "title");
        assert_eq!(owner(&s, Kind::Single, 12), "other");
        assert_eq!(owner(&s, Kind::Sleep, 0), "title-code");
    }
}
