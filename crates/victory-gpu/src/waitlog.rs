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
    if matches!(kind, Kind::Single | Kind::Multiple) {
        note_waited(handle);
    }
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


// ---- (M1LK Sig Source) who signals what the presenting thread waits on ----

/// Per thread: the time it ran our translation code (recording, release_held, present) and
/// the signals it sent to what the presenting thread waits on.
pub(crate) struct ThreadStat {
    tid: u32,
    ours_ns: std::sync::atomic::AtomicU64,
    signals: std::sync::atomic::AtomicU64,
    signals_ours: std::sync::atomic::AtomicU64,
    cpu_ns: std::sync::atomic::AtomicU64,
}

static THREADS: Mutex<Vec<&'static ThreadStat>> = Mutex::new(Vec::new());

thread_local! {
    static ME: std::cell::Cell<Option<&'static ThreadStat>> = const { std::cell::Cell::new(None) };
    /// This thread's `ours_ns` at its last signal.
    static LAST_OURS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

fn me() -> &'static ThreadStat {
    ME.with(|m| {
        if let Some(stat) = m.get() {
            return stat;
        }
        // SAFETY: plain query.
        let tid = unsafe { windows_sys::Win32::System::Threading::GetCurrentThreadId() };
        let stat: &'static ThreadStat = Box::leak(Box::new(ThreadStat {
            tid,
            ours_ns: Default::default(),
            signals: Default::default(),
            signals_ours: Default::default(),
            cpu_ns: Default::default(),
        }));
        THREADS.lock().unwrap_or_else(|e| e.into_inner()).push(stat);
        m.set(Some(stat));
        stat
    })
}

/// This thread ran `ns` of our translation code (not nested in other such time).
#[inline]
pub(crate) fn ours_add(ns: u64) {
    if enabled() {
        me().ours_ns.fetch_add(ns, std::sync::atomic::Ordering::Relaxed);
    }
}

/// The handles the presenting thread has waited on (a fixed set, no lock).
static WAITED: [std::sync::atomic::AtomicUsize; 32] = [const { std::sync::atomic::AtomicUsize::new(0) }; 32];

fn note_waited(handle: usize) {
    use std::sync::atomic::Ordering::Relaxed;
    if handle == 0 {
        return;
    }
    for slot in &WAITED {
        match slot.load(Relaxed) {
            h if h == handle => return,
            0 => {
                if slot.compare_exchange(0, handle, Relaxed, Relaxed).is_ok() {
                    return;
                }
                if slot.load(Relaxed) == handle {
                    return;
                }
            }
            _ => {}
        }
    }
}

fn is_waited(handle: usize) -> bool {
    use std::sync::atomic::Ordering::Relaxed;
    handle != 0 && WAITED.iter().any(|slot| slot.load(Relaxed) == handle)
}

/// Our translation time since the thread's last signal that makes a signal "ours": it ran at
/// least this much of our code (ns) in the job it just finished.
const OURS_FLOOR_NS: u64 = 100_000;

/// Whether a signal followed enough of our translation time (`ours` ns since the last one).
pub(crate) fn signal_is_ours(ours_since_last: u64) -> bool {
    ours_since_last >= OURS_FLOOR_NS
}

/// The title signalled `handle` (a semaphore release or an event set) on this thread.
pub fn signal(handle: usize) {
    use std::sync::atomic::Ordering::Relaxed;
    if !is_waited(handle) {
        return;
    }
    let stat = me();
    stat.signals.fetch_add(1, Relaxed);
    let now = stat.ours_ns.load(Relaxed);
    let before = LAST_OURS.with(|l| l.replace(now));
    if signal_is_ours(now.saturating_sub(before)) {
        stat.signals_ours.fetch_add(1, Relaxed);
    }
}

/// CPU time (ns) of thread `tid`, 0 if it cannot be read.
fn cpu_of(tid: u32) -> u64 {
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME};
    use windows_sys::Win32::System::Threading::{GetThreadTimes, OpenThread};
    // SAFETY: a query handle, used and closed here.
    unsafe {
        let handle = OpenThread(0x800, 0, tid);
        if handle.is_null() {
            return 0;
        }
        let zero = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
        let (mut create, mut exit, mut kernel, mut user) = (zero, zero, zero, zero);
        let ok = GetThreadTimes(handle, &mut create, &mut exit, &mut kernel, &mut user);
        CloseHandle(handle);
        let ticks = |t: FILETIME| u64::from(t.dwHighDateTime) << 32 | u64::from(t.dwLowDateTime);
        if ok != 0 { (ticks(kernel) + ticks(user)) * 100 } else { 0 }
    }
}

fn sig_line(frames: f64) {
    use std::sync::atomic::Ordering::Relaxed;
    let threads: Vec<&'static ThreadStat> = THREADS.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let ms = |ns: u64| ns as f64 / 1_000_000.0 / frames;
    let (mut signals, mut ours, mut tagged) = (0u64, 0u64, 0usize);
    let mut top: Option<(u64, u32, u64, u64, u64)> = None;
    for t in &threads {
        let s = t.signals.swap(0, Relaxed);
        let so = t.signals_ours.swap(0, Relaxed);
        let o = t.ours_ns.swap(0, Relaxed);
        let cpu_now = if s > 0 || o > 0 { cpu_of(t.tid) } else { 0 };
        let cpu = cpu_now.saturating_sub(t.cpu_ns.swap(cpu_now, Relaxed));
        signals += s;
        ours += so;
        if o > 0 {
            tagged += 1;
        }
        if s > 0 && top.is_none_or(|b| s > b.0) {
            top = Some((s, t.tid, o, cpu, so));
        }
    }
    let (ts, tid, to, tcpu, tso) = top.unwrap_or((0, 0, 0, 0, 0));
    let pct = if signals > 0 { 100.0 * ours as f64 / signals as f64 } else { 0.0 };
    tracing::info!(
        "M1LK Sig Source: signals={:.1} | from_our_translation={:.1} ({pct:.0}%) from_title_only={:.1} | \
         top_signaller_tid={tid} signals={:.1} from_ours={:.1} ran_in_ours={:.2} ms ran_in_title={:.2} ms | \
         threads_tagged_as_translation={tagged}/{}",
        signals as f64 / frames,
        ours as f64 / frames,
        (signals - ours) as f64 / frames,
        ts as f64 / frames,
        tso as f64 / frames,
        ms(to),
        ms(tcpu.saturating_sub(to)),
        threads.len(),
    );
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
    sig_line(frames);
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
    fn a_signal_is_ours_after_enough_translation_time() {
        assert!(!signal_is_ours(0));
        assert!(!signal_is_ours(OURS_FLOOR_NS - 1));
        assert!(signal_is_ours(OURS_FLOOR_NS));
    }

    #[test]
    fn waited_handles_are_remembered_once() {
        note_waited(0x1234_5670);
        note_waited(0x1234_5670);
        assert!(is_waited(0x1234_5670));
        assert!(!is_waited(0x1234_5678));
        assert!(!is_waited(0));
    }

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
