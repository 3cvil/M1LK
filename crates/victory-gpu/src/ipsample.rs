//! (M1LK IP Sample) Where the title's busy threads spend their CPU, off by default
//! (`VICTORY_IP_SAMPLE=1`).
//!
//! A sampler thread walks the process's threads about every 250 ms and keeps those that ran
//! at least 5 ms since the last walk, other than itself and the thread that presents. About
//! every millisecond it takes each of them, one at a time, through `SuspendThread`,
//! `GetThreadContext` (control registers, for `Rip`) and `ResumeThread`, and counts the `Rip`.
//! Every 2 s it names the modules the addresses are in and logs the share of the title's
//! image, of ours and of the system, and the hottest functions and addresses of the title
//! as RVAs (from the image's base, function starts from its unwind table).
//!
//! Safety: a thread is suspended only for the two calls between suspend and resume, which
//! allocate nothing and take no lock of ours or the process's, and never two at once. All
//! counting and logging runs while no thread is suspended. A thread that cannot be suspended
//! or read is left alone.
//!
//! Cost: some 20-30 microseconds a suspended thread a sample, a few busy threads at roughly
//! 1 ms: one core of the sampler thread's own and a few percent on each sampled thread.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering::Relaxed};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, HANDLE, HMODULE};
use windows_sys::Win32::System::Diagnostics::Debug::{CONTEXT, GetThreadContext, RtlLookupFunctionEntry};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
};
use windows_sys::Win32::System::LibraryLoader::{
    GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, GetModuleFileNameW,
    GetModuleHandleExW,
};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcessId, GetCurrentThreadId, GetThreadTimes, OpenThread, ResumeThread, SuspendThread,
};

/// `VICTORY_IP_SAMPLE=1`.
pub(crate) fn enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("VICTORY_IP_SAMPLE").is_ok_and(|v| v.trim() == "1"))
}

/// The thread that presents (never sampled).
static PRESENT_TID: AtomicU32 = AtomicU32::new(0);

/// Starts the sampler once, from the thread that presents.
pub(crate) fn start_once() {
    if !enabled() {
        return;
    }
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        // SAFETY: plain query.
        PRESENT_TID.store(unsafe { GetCurrentThreadId() }, Relaxed);
        let spawned = std::thread::Builder::new().name("victory-ipsample".into()).spawn(run);
        match spawned {
            Ok(_) => tracing::info!("M1LK IP Sample: on (sampling the busy threads of the process about every ms)"),
            Err(e) => tracing::warn!(%e, "M1LK IP Sample: the sampler thread was not started"),
        }
    });
}

/// CPU time of a thread (100 ns units), 0 if it cannot be read.
fn cpu_of(handle: HANDLE) -> u64 {
    let zero = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
    let (mut create, mut exit, mut kernel, mut user) = (zero, zero, zero, zero);
    // SAFETY: a thread handle of ours and four out values.
    let ok = unsafe { GetThreadTimes(handle, &mut create, &mut exit, &mut kernel, &mut user) };
    let ticks = |t: FILETIME| u64::from(t.dwHighDateTime) << 32 | u64::from(t.dwLowDateTime);
    if ok != 0 { ticks(kernel) + ticks(user) } else { 0 }
}

/// The ids of the process's threads.
fn thread_ids() -> Vec<u32> {
    let mut ids = Vec::new();
    // SAFETY: a snapshot handle closed below; the entry is sized as the API asks.
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
        if snapshot.is_null() || snapshot as isize == -1 {
            return ids;
        }
        let me = GetCurrentProcessId();
        let mut entry: THREADENTRY32 = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
        let mut more = Thread32First(snapshot, &mut entry) != 0;
        while more {
            if entry.th32OwnerProcessID == me {
                ids.push(entry.th32ThreadID);
            }
            more = Thread32Next(snapshot, &mut entry) != 0;
        }
        CloseHandle(snapshot);
    }
    ids
}

/// The control registers' `Rip` of `thread`, which is suspended for the two calls only.
/// Nothing here allocates or locks.
fn sample(thread: HANDLE) -> Option<usize> {
    // SAFETY: a handle opened for suspend/resume and context reads; a zeroed CONTEXT is
    // valid; the thread is resumed before anything else runs on this one.
    unsafe {
        if SuspendThread(thread) == u32::MAX {
            return None;
        }
        let mut context: CONTEXT = std::mem::zeroed();
        // CONTEXT_CONTROL on x64.
        context.ContextFlags = 0x0010_0001;
        let read = GetThreadContext(thread, &mut context);
        ResumeThread(thread);
        (read != 0).then_some(context.Rip as usize)
    }
}

/// What a sampled address is in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Class {
    Title,
    Ours,
    System,
}

/// Which class a module file name is in: the title's image (`title` is a lower-case prefix of
/// its file name), ours (`victory*` or the runtime's own exe `exe`), or the system.
pub(crate) fn classify(file_name: &str, title: &str, exe: &str) -> Class {
    let name = file_name.to_ascii_lowercase();
    if !title.is_empty() && name.starts_with(title) {
        Class::Title
    } else if name.starts_with("victory") || (!exe.is_empty() && name == exe) {
        Class::Ours
    } else {
        Class::System
    }
}

/// The `n` biggest of `counts` by count (ties by address), as (key, count).
pub(crate) fn top(counts: &HashMap<usize, u64>, n: usize) -> Vec<(usize, u64)> {
    let mut all: Vec<(usize, u64)> = counts.iter().map(|(&k, &c)| (k, c)).collect();
    all.sort_by_key(|&(k, c)| (std::cmp::Reverse(c), k));
    all.truncate(n);
    all
}

fn module_file(module: HMODULE) -> String {
    let mut path = [0u16; 260];
    // SAFETY: a loaded module and a buffer whose length is passed.
    let len = unsafe { GetModuleFileNameW(module, path.as_mut_ptr(), path.len() as u32) } as usize;
    let path = String::from_utf16_lossy(&path[..len.min(path.len())]);
    path.rsplit('\\').next().unwrap_or(&path).to_string()
}

fn run() {
    let title = std::env::var("VICTORY_IP_TITLE").unwrap_or_else(|_| "victory-image".into()).to_ascii_lowercase();
    // SAFETY: the process's own exe.
    let exe = module_file(std::ptr::null_mut()).to_ascii_lowercase();
    // SAFETY: plain query.
    let me = unsafe { GetCurrentThreadId() };
    let present = PRESENT_TID.load(Relaxed);
    // Thread id -> (handle, CPU at the last walk).
    let mut handles: HashMap<u32, (HANDLE, u64)> = HashMap::new();
    let mut busy: Vec<HANDLE> = Vec::new();
    let mut counts: HashMap<usize, u64> = HashMap::new();
    let mut samples = 0u64;
    let mut next_walk = Instant::now();
    let mut next_flush = Instant::now() + Duration::from_secs(2);
    let mut flushed = Instant::now();
    loop {
        let now = Instant::now();
        if now >= next_walk {
            next_walk = now + Duration::from_millis(250);
            let ids = thread_ids();
            handles.retain(|id, (h, _)| {
                let alive = ids.contains(id);
                if !alive {
                    // SAFETY: a handle we opened.
                    unsafe { CloseHandle(*h) };
                }
                alive
            });
            busy.clear();
            for id in ids {
                if id == me || id == present {
                    continue;
                }
                let entry = handles.entry(id).or_insert_with(|| {
                    // SUSPEND_RESUME | GET_CONTEXT | QUERY_LIMITED_INFORMATION.
                    // SAFETY: opening a thread of this process.
                    let h = unsafe { OpenThread(0x2 | 0x8 | 0x800, 0, id) };
                    (h, 0)
                });
                if entry.0.is_null() {
                    continue;
                }
                let cpu = cpu_of(entry.0);
                // 5 ms of CPU since the last walk.
                if cpu.saturating_sub(entry.1) >= 50_000 && entry.1 != 0 {
                    busy.push(entry.0);
                }
                entry.1 = cpu;
            }
        }
        for &thread in &busy {
            // Resumed again before the next line runs.
            if let Some(rip) = sample(thread) {
                *counts.entry(rip).or_default() += 1;
                samples += 1;
            }
        }
        if now >= next_flush {
            next_flush = now + Duration::from_secs(2);
            flush(&mut counts, &mut samples, flushed.elapsed(), busy.len(), &title, &exe);
            flushed = Instant::now();
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn flush(counts: &mut HashMap<usize, u64>, samples: &mut u64, window: Duration, threads: usize, title: &str, exe: &str) {
    let total = std::mem::take(samples);
    let counts = std::mem::take(counts);
    if total == 0 {
        return;
    }
    let (mut in_title, mut in_ours, mut in_system) = (0u64, 0u64, 0u64);
    let mut functions: HashMap<usize, u64> = HashMap::new();
    let mut addresses: HashMap<usize, u64> = HashMap::new();
    for (&rip, &count) in &counts {
        let mut module: HMODULE = std::ptr::null_mut();
        // SAFETY: looks up the module holding an address; no reference is taken.
        let found = unsafe {
            GetModuleHandleExW(
                GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
                rip as *const u16,
                &mut module,
            )
        } != 0;
        let class = if found { classify(&module_file(module), title, exe) } else { Class::System };
        let base = if found { module as usize } else { 0 };
        match class {
            Class::Title => {
                in_title += count;
                *addresses.entry(rip - base).or_default() += count;
                let mut image = 0u64;
                // SAFETY: the unwind table lookup of an address in a loaded image.
                let entry = unsafe { RtlLookupFunctionEntry(rip as u64, &mut image, std::ptr::null_mut()) };
                let function = if entry.is_null() {
                    rip - base
                } else {
                    // SAFETY: a RUNTIME_FUNCTION of that image.
                    unsafe { (*entry).BeginAddress as usize }
                };
                *functions.entry(function).or_default() += count;
            }
            Class::Ours => in_ours += count,
            Class::System => in_system += count,
        }
    }
    let pct = |n: u64| 100.0 * n as f64 / total as f64;
    let list = |m: &HashMap<usize, u64>, n: usize| {
        top(m, n)
            .iter()
            .map(|&(rva, c)| format!("{rva:#x} {c} ({:.1}%)", pct(c)))
            .collect::<Vec<_>>()
            .join(" | ")
    };
    tracing::info!(
        "M1LK IP Sample: samples={total} ({:.0}/s, {threads} busy threads) title={:.1}% ours={:.1}% sys={:.1}% | top title functions (RVA of start): {}",
        total as f64 / window.as_secs_f64().max(0.001),
        pct(in_title),
        pct(in_ours),
        pct(in_system),
        list(&functions, 15),
    );
    tracing::info!("M1LK IP Sample top title addresses (RVA): {}", list(&addresses, 15));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modules_are_classified_by_name() {
        assert_eq!(classify("victory-image.dll", "victory-image", "m1lk.exe"), Class::Title);
        assert_eq!(classify("Victory-Image.dll", "victory-image", "m1lk.exe"), Class::Title);
        assert_eq!(classify("victory-gpu.dll", "victory-image", "m1lk.exe"), Class::Ours);
        assert_eq!(classify("m1lk.exe", "victory-image", "m1lk.exe"), Class::Ours);
        assert_eq!(classify("ntdll.dll", "victory-image", "m1lk.exe"), Class::System);
        assert_eq!(classify("kernelbase.dll", "", ""), Class::System);
    }

    #[test]
    fn the_hottest_come_first_with_stable_ties() {
        let mut m = HashMap::new();
        m.insert(0x30, 5);
        m.insert(0x10, 9);
        m.insert(0x20, 5);
        m.insert(0x40, 1);
        assert_eq!(top(&m, 3), vec![(0x10, 9), (0x20, 5), (0x30, 5)]);
        assert_eq!(top(&m, 10).len(), 4);
    }
}
