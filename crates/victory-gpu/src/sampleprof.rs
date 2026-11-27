//! In-process sampling profiler, off by default (`M1LK_SAMPLE_PROF=1`).
//!
//! Tracing and the sub-stage timers say how long a stage takes, not which
//! function in it is slow, and an event-tracing session needs administrator
//! rights. This is the cheap substitute: a background thread wakes every
//! millisecond (`M1LK_SAMPLE_MS`), looks at each thread of the process that ran
//! since the last look, suspends it, copies its registers, walks its stack
//! with the unwind tables (`RtlLookupFunctionEntry` / `RtlVirtualUnwind`, up
//! to 32 frames) into a fixed buffer, and resumes it. Nothing is allocated,
//! locked or logged while a thread is suspended; counting happens after.
//!
//! Threads are found by listing the process's threads once a second, so the
//! title's own threads (which have no name) are covered as well as ours (named
//! through `std::thread::Builder`, read back with `GetThreadDescription`).
//! `M1LK_SAMPLE_THREADS=a,b` keeps only threads whose name contains one of the
//! words (`tid-1234` for an unnamed one).
//!
//! Every 30 s, and at the end of `run`, `sample-prof-<pid>.txt` is rewritten
//! (cumulative) in `M1LK_SAMPLE_DIR` (the launcher sets it to its `logs`
//! folder), else beside the exe: for each thread, the busiest functions by
//! where the thread stood (leaf) and by being anywhere on the stack
//! (inclusive). Functions of our own exe are named from the PDB beside it
//! (`m1lk.pdb`, shipped by the Build artifact); other modules are
//! `module+0xFUNCTION_RVA`; addresses outside every loaded module are the
//! title's mapped code: `title code +0xRVA`.
//!
//! Cost with the switch off: one cached environment read at start-up.

use std::collections::HashMap;
use std::fmt::Write as _;

/// One thread's counts, as the report takes them.
pub struct ThreadStats {
    pub tid: u32,
    pub name: String,
    /// Looks at the thread where it had not run since the previous look.
    pub idle: u64,
    /// Each distinct stack (leaf first) and how often it was seen.
    pub stacks: Vec<(Vec<u64>, u64)>,
}

/// Turns an address of a stack into the name it is counted under.
pub trait Resolve {
    /// The function the address is in (the same text for all its addresses).
    fn function(&mut self, addr: u64) -> String;
    /// `file:line` of the address, where the PDB knows it.
    fn line(&mut self, addr: u64) -> Option<String>;
}

fn add(map: &mut HashMap<String, u64>, key: String, by: u64) {
    *map.entry(key).or_insert(0) += by;
}

fn top(map: HashMap<String, u64>, count: usize) -> Vec<(String, u64)> {
    let mut all: Vec<(String, u64)> = map.into_iter().collect();
    all.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    all.truncate(count);
    all
}

/// The text of a report: per thread (busiest first), the top `top_leaf`
/// functions by leaf samples, the top 30 leaf lines, and the top
/// `top_inclusive` functions by samples that had them anywhere on the stack.
pub fn format_report(threads: &[ThreadStats], resolve: &mut dyn Resolve, top_leaf: usize, top_inclusive: usize) -> String {
    let busy_of = |t: &ThreadStats| t.stacks.iter().map(|s| s.1).sum::<u64>();
    let mut order: Vec<&ThreadStats> = threads.iter().filter(|t| busy_of(t) > 0).collect();
    order.sort_by(|a, b| busy_of(b).cmp(&busy_of(a)).then_with(|| a.tid.cmp(&b.tid)));
    let mut out = String::new();
    for t in order {
        let busy = busy_of(t);
        let looks = busy + t.idle;
        let _ = writeln!(
            out,
            "== {} (tid {}): {busy} samples on the processor, {} looks where it had not run ({:.1}% of looks busy)",
            t.name,
            t.tid,
            t.idle,
            100.0 * busy as f64 / looks.max(1) as f64
        );
        let mut leaf = HashMap::new();
        let mut leaf_line = HashMap::new();
        let mut inclusive = HashMap::new();
        for (frames, count) in &t.stacks {
            let Some(&first) = frames.first() else { continue };
            let name = resolve.function(first);
            if let Some(line) = resolve.line(first) {
                add(&mut leaf_line, format!("{name} @ {line}"), *count);
            }
            add(&mut leaf, name, *count);
            let mut seen: Vec<String> = Vec::with_capacity(frames.len());
            for &frame in frames {
                let name = resolve.function(frame);
                if !seen.contains(&name) {
                    seen.push(name);
                }
            }
            for name in seen {
                add(&mut inclusive, name, *count);
            }
        }
        let mut section = |title: &str, map: HashMap<String, u64>, count: usize| {
            let _ = writeln!(out, "-- {title}");
            for (name, n) in top(map, count) {
                let _ = writeln!(out, "{:6.2}%  {n:>8}  {name}", 100.0 * n as f64 / busy as f64);
            }
        };
        section(&format!("top {top_leaf} leaf functions (where the thread stood)"), leaf, top_leaf);
        section("top 30 leaf lines (our own code)", leaf_line, 30);
        section(&format!("top {top_inclusive} inclusive functions (anywhere on the stack)"), inclusive, top_inclusive);
        out.push('\n');
    }
    if out.is_empty() {
        out.push_str("no samples yet\n");
    }
    out
}

/// Is `sp` a place to read a return address from: 8-aligned and inside the
/// thread's stack `[lo, hi)` with room for one more word? With no bounds
/// known (`hi == 0`) nothing is.
pub fn stack_pointer_ok(sp: u64, lo: u64, hi: u64) -> bool {
    hi != 0 && sp >= lo && sp & 7 == 0 && sp.checked_add(16).is_some_and(|end| end < hi)
}

/// Does a function table entry (`begin..end`, image-relative) cover `pc`
/// (image-relative), and does it point at plain unwind data (not a chained
/// entry, bit 0)? An entry that fails this is not unwound with.
pub fn entry_ok(begin: u32, end: u32, unwind: u32, pc_rva: u64) -> bool {
    begin < end && pc_rva >= u64::from(begin) && pc_rva < u64::from(end) && unwind & 1 == 0 && unwind != 0
}

/// The unwind data's first bytes: version 1 or 2, no machine-frame code
/// (opcode 10), and the whole code array (`4 + 2 * count` bytes) lies within
/// `readable` bytes and within `data`.
pub fn unwind_info_ok(data: &[u8], readable: usize) -> bool {
    if data.len() < 4 || !matches!(data[0] & 7, 1 | 2) {
        return false;
    }
    let need = 4 + 2 * usize::from(data[2]);
    if need > readable || data.len() < need {
        return false;
    }
    !data[4..need].chunks_exact(2).any(|code| code[1] & 0xf == 10)
}

/// `M1LK_SAMPLE_PROF=1`: the profiler is on. Read once.
pub fn on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("VICTORY_SAMPLE_PROF").is_ok_and(|v| v.trim() == "1"))
}

#[cfg(windows)]
pub use imp::{finish, start};

/// Starts the sampling thread (once), if the switch is on.
#[cfg(not(windows))]
pub fn start() {}
/// Writes the report now, if the switch is on.
#[cfg(not(windows))]
pub fn finish() {}

#[cfg(windows)]
mod imp {
    use super::{Resolve, ThreadStats, entry_ok, format_report, on, stack_pointer_ok, unwind_info_ok};
    use std::sync::atomic::{AtomicPtr, AtomicU32};
    use windows_sys::Win32::System::Diagnostics::Debug::{AddVectoredExceptionHandler, EXCEPTION_POINTERS};
    use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEMORY_BASIC_INFORMATION, VirtualQuery};
    use windows_sys::Win32::System::Threading::ExitThread;
    use std::collections::HashMap;
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE, LocalFree};
    use windows_sys::Win32::System::Diagnostics::Debug::{
        CONTEXT, CONTEXT_CONTROL_AMD64, CONTEXT_INTEGER_AMD64, GetThreadContext, IMAGEHLP_LINE64,
        RtlLookupFunctionEntry, RtlVirtualUnwind, SYMBOL_INFO, SYMOPT_DEFERRED_LOADS, SYMOPT_FAIL_CRITICAL_ERRORS,
        SYMOPT_LOAD_LINES, SYMOPT_NO_PROMPTS, SYMOPT_UNDNAME, SymFromAddr, SymGetLineFromAddr64, SymInitialize,
        SymLoadModuleExW, SymSetOptions, UNW_FLAG_NHANDLER,
    };
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
    };
    use windows_sys::Win32::System::LibraryLoader::{
        GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, GetModuleFileNameW,
        GetModuleHandleExW, GetModuleHandleW,
    };
    use windows_sys::Win32::System::Threading::{
        CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, CreateWaitableTimerExW, GetCurrentProcess, GetCurrentProcessId,
        GetCurrentThreadId, GetThreadDescription, OpenThread, ResumeThread, SetWaitableTimer, SuspendThread,
        THREAD_GET_CONTEXT, THREAD_QUERY_LIMITED_INFORMATION, THREAD_SUSPEND_RESUME, WaitForSingleObject,
    };

    /// Frames kept of a stack.
    const MAX_FRAMES: usize = 32;
    /// `THREAD_QUERY_INFORMATION`, for `NtQueryInformationThread`.
    const THREAD_QUERY_INFORMATION: u32 = 0x0040;
    const TIMER_ALL_ACCESS: u32 = 0x001F_0003;

    #[link(name = "kernel32", kind = "raw-dylib")]
    unsafe extern "system" {
        fn QueryThreadCycleTime(thread: HANDLE, cycles: *mut u64) -> i32;
    }

    #[link(name = "ntdll", kind = "raw-dylib")]
    unsafe extern "system" {
        fn NtQueryInformationThread(thread: HANDLE, class: u32, info: *mut c_void, len: u32, returned: *mut u32) -> i32;
    }

    #[repr(C)]
    #[allow(dead_code)]
    struct ThreadBasicInformation {
        exit_status: i32,
        teb: *mut c_void,
        process: usize,
        thread: usize,
        affinity: usize,
        priority: i32,
        base_priority: i32,
    }

    /// Set once the sampler thread is gone (stopped, or ended by a fault).
    static DEAD: AtomicBool = AtomicBool::new(false);
    /// Asked to stop (the run is ending, modules are about to go).
    static STOP: AtomicBool = AtomicBool::new(false);
    /// The sampler thread's id, for the fault guard; 0 before it runs.
    static SAMPLER_TID: AtomicU32 = AtomicU32::new(0);
    /// The handle of the thread suspended right now, to resume if the sampler faults.
    static SUSPENDED: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());

    /// Where a faulting sampler thread is sent: resume what it suspended, mark
    /// the sampler dead and end only this thread. The game goes on.
    unsafe extern "system" fn sampler_fault_exit() -> ! {
        let held = SUSPENDED.swap(std::ptr::null_mut(), Ordering::SeqCst);
        if !held.is_null() {
            // SAFETY: the handle of the thread this thread suspended, still open.
            unsafe { ResumeThread(held) };
        }
        DEAD.store(true, Ordering::SeqCst);
        // SAFETY: ends the calling (sampler) thread.
        unsafe { ExitThread(0) }
    }

    /// First in line: a memory fault on the sampler thread (a bad unwind, a
    /// symbol lookup into an unloaded module) ends the sampler, not the game.
    unsafe extern "system" fn guard(info: *mut EXCEPTION_POINTERS) -> i32 {
        // SAFETY: the system passes valid exception pointers.
        unsafe {
            let code = (*(*info).ExceptionRecord).ExceptionCode as u32;
            let mine = SAMPLER_TID.load(Ordering::Relaxed);
            if mine != 0 && GetCurrentThreadId() == mine && matches!(code, 0xC000_0005 | 0xC000_0006 | 0xC000_001D | 0xC000_0094) {
                let ctx = &mut *(*info).ContextRecord;
                ctx.Rip = sampler_fault_exit as usize as u64;
                // As if called: aligned, with a return address slot and shadow space.
                ctx.Rsp = (ctx.Rsp.saturating_sub(0x200) & !0xf) - 8;
                return -1; // EXCEPTION_CONTINUE_EXECUTION
            }
        }
        0 // EXCEPTION_CONTINUE_SEARCH
    }

    /// The memory at `addr` as `(committed, bytes readable from addr to the region's end)`.
    fn region(addr: u64) -> (bool, usize) {
        // SAFETY: a zeroed MEMORY_BASIC_INFORMATION is valid; a query of our own address space.
        unsafe {
            let mut info: MEMORY_BASIC_INFORMATION = std::mem::zeroed();
            if VirtualQuery(addr as usize as *const c_void, &mut info, size_of::<MEMORY_BASIC_INFORMATION>()) == 0 {
                return (false, 0);
            }
            let end = info.BaseAddress as u64 + info.RegionSize as u64;
            (info.State == MEM_COMMIT, end.saturating_sub(addr) as usize)
        }
    }

    /// Is the function entry safe to unwind with at `pc`: it covers `pc`, and
    /// its unwind data lies in committed memory with a sane header.
    ///
    /// # Safety
    /// `entry` points at a function table entry (three `u32`).
    unsafe fn entry_usable(base: u64, entry: *const u32, pc: u64) -> bool {
        // SAFETY: the entry's fields are read; the unwind data only after the
        // region query says it is committed.
        unsafe {
            let (begin, end, unwind) = (entry.read(), entry.add(1).read(), entry.add(2).read());
            if base == 0 || !entry_ok(begin, end, unwind, pc.wrapping_sub(base)) {
                return false;
            }
            let at = base + u64::from(unwind);
            let (committed, readable) = region(at);
            if !committed || readable < 4 {
                return false;
            }
            let count = (at as *const u8).add(2).read();
            let have = readable.min(4 + 2 * usize::from(count));
            unwind_info_ok(std::slice::from_raw_parts(at as *const u8, have), readable)
        }
    }

    struct Target {
        tid: u32,
        handle: HANDLE,
        name: String,
        /// The stack's bounds (high end, lowest reserved address); 0 if unknown.
        stack_hi: u64,
        stack_lo: u64,
        cycles: u64,
        idle: u64,
        stacks: HashMap<Vec<u64>, u64>,
    }

    fn thread_name(handle: HANDLE) -> String {
        let mut name = std::ptr::null_mut();
        // SAFETY: the system allocates the UTF-16 string; it is freed with LocalFree.
        unsafe {
            if GetThreadDescription(handle, &mut name) < 0 || name.is_null() {
                return String::new();
            }
            let len = (0..).take_while(|&i| *name.add(i) != 0).count();
            let text = String::from_utf16_lossy(std::slice::from_raw_parts(name, len));
            LocalFree(name.cast::<c_void>());
            text
        }
    }

    /// The thread's stack bounds from its TEB, or zeros.
    fn stack_bounds(handle: HANDLE) -> (u64, u64) {
        let mut info: ThreadBasicInformation = unsafe { std::mem::zeroed() };
        let mut got = 0u32;
        // SAFETY: class 0 is ThreadBasicInformation, which `info` has the room for.
        let status = unsafe {
            NtQueryInformationThread(handle, 0, (&raw mut info).cast(), size_of::<ThreadBasicInformation>() as u32, &mut got)
        };
        if status < 0 || info.teb.is_null() {
            return (0, 0);
        }
        // SAFETY: a live thread's TEB: StackBase at +8, DeallocationStack at +0x1478 (x64).
        unsafe {
            let hi = info.teb.cast::<u8>().add(8).cast::<u64>().read();
            let lo = info.teb.cast::<u8>().add(0x1478).cast::<u64>().read();
            if lo != 0 && hi > lo { (hi, lo) } else { (0, 0) }
        }
    }

    /// The thread ids of the process.
    fn thread_ids(out: &mut Vec<u32>) {
        out.clear();
        // SAFETY: a snapshot handle, walked and closed here.
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
            if snap == INVALID_HANDLE_VALUE || snap.is_null() {
                return;
            }
            let pid = GetCurrentProcessId();
            let mut entry = THREADENTRY32 { dwSize: size_of::<THREADENTRY32>() as u32, ..Default::default() };
            let mut more = Thread32First(snap, &mut entry);
            while more != 0 {
                if entry.th32OwnerProcessID == pid {
                    out.push(entry.th32ThreadID);
                }
                entry.dwSize = size_of::<THREADENTRY32>() as u32;
                more = Thread32Next(snap, &mut entry);
            }
            CloseHandle(snap);
        }
    }

    /// Looks at `target` if it ran since the last look. The thread is suspended
    /// only while its registers and stack are walked into `frames`: no
    /// allocation, lock or log in between. Returns the frame count (0: none).
    fn look(target: &mut Target, ctx: &mut CONTEXT, frames: &mut [u64; MAX_FRAMES]) -> usize {
        let mut cycles = 0u64;
        // SAFETY: a handle we opened, and an out value.
        if unsafe { QueryThreadCycleTime(target.handle, &mut cycles) } == 0 {
            return 0;
        }
        if cycles == target.cycles {
            target.idle += 1;
            return 0;
        }
        target.cycles = cycles;
        let (hi, lo) = (target.stack_hi, target.stack_lo);
        let mut n = 0usize;
        // SAFETY: the thread is ours to suspend; every read of its stack is inside the bounds
        // its TEB gave; the thread is resumed before anything else is done.
        unsafe {
            if SuspendThread(target.handle) == u32::MAX {
                return 0;
            }
            SUSPENDED.store(target.handle, Ordering::SeqCst);
            ctx.ContextFlags = CONTEXT_CONTROL_AMD64 | CONTEXT_INTEGER_AMD64;
            if GetThreadContext(target.handle, ctx) != 0 {
                let mut rip = ctx.Rip;
                while rip != 0 {
                    // Return addresses point after the call: back up into it.
                    frames[n] = if n == 0 { rip } else { rip - 1 };
                    n += 1;
                    let sp = ctx.Rsp;
                    if n == MAX_FRAMES || !stack_pointer_ok(sp, lo, hi) {
                        break;
                    }
                    // Only code in committed memory is looked up and stepped into.
                    if !region(frames[n - 1]).0 {
                        break;
                    }
                    let mut base = 0u64;
                    let entry = RtlLookupFunctionEntry(frames[n - 1], &mut base, std::ptr::null_mut());
                    if !entry.is_null() && !entry_usable(base, entry.cast(), frames[n - 1]) {
                        break;
                    }
                    if entry.is_null() {
                        // A leaf without unwind data: the return address is on top.
                        rip = (sp as *const u64).read();
                        ctx.Rsp = sp + 8;
                    } else {
                        let mut data = std::ptr::null_mut();
                        let mut frame = 0u64;
                        RtlVirtualUnwind(UNW_FLAG_NHANDLER, base, frames[n - 1], entry, ctx, &mut data, &mut frame, std::ptr::null_mut());
                        rip = ctx.Rip;
                    }
                    if ctx.Rsp <= sp {
                        break;
                    }
                }
            }
            SUSPENDED.store(std::ptr::null_mut(), Ordering::SeqCst);
            ResumeThread(target.handle);
        }
        n
    }

    fn wide(text: &std::path::Path) -> Vec<u16> {
        use std::os::windows::ffi::OsStrExt;
        text.as_os_str().encode_wide().chain(std::iter::once(0)).collect()
    }

    /// Names addresses: our exe from its PDB, the rest by module and offset.
    struct Symbols {
        process: HANDLE,
        ours: (u64, u64),
        loaded: bool,
        functions: HashMap<u64, String>,
        lines: HashMap<u64, Option<String>>,
        buffer: Vec<u64>,
    }

    impl Symbols {
        fn new() -> Self {
            // SAFETY: dbghelp calls from this one thread, on our own process.
            unsafe {
                let process = GetCurrentProcess();
                let exe = std::env::current_exe().unwrap_or_default();
                let mut dir = exe.parent().map(|d| d.as_os_str().to_string_lossy().into_owned().into_bytes()).unwrap_or_default();
                dir.push(0);
                SymSetOptions(SYMOPT_UNDNAME | SYMOPT_LOAD_LINES | SYMOPT_DEFERRED_LOADS | SYMOPT_FAIL_CRITICAL_ERRORS | SYMOPT_NO_PROMPTS);
                let ready = SymInitialize(process, dir.as_ptr(), 0) != 0;
                let base = GetModuleHandleW(std::ptr::null()) as u64;
                let mut size = 0u64;
                if base != 0 {
                    let nt = base + u64::from((base as *const u32).byte_add(0x3c).read());
                    size = u64::from(((nt + 24 + 56) as *const u32).read());
                }
                let mut loaded = false;
                if ready && size != 0 {
                    let path = wide(&exe);
                    loaded = SymLoadModuleExW(process, std::ptr::null_mut(), path.as_ptr(), std::ptr::null(), base, size as u32, std::ptr::null(), 0) != 0;
                }
                Self {
                    process,
                    ours: (base, base + size),
                    loaded,
                    functions: HashMap::new(),
                    lines: HashMap::new(),
                    buffer: vec![0u64; (size_of::<SYMBOL_INFO>() + 1024).div_ceil(8)],
                }
            }
        }

        fn ours(&self, addr: u64) -> bool {
            addr >= self.ours.0 && addr < self.ours.1
        }

        fn other(addr: u64) -> String {
            // SAFETY: lookups by address; buffers have the sizes passed.
            unsafe {
                let mut module: HANDLE = std::ptr::null_mut();
                let found = GetModuleHandleExW(
                    GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
                    addr as usize as *const u16,
                    &mut module,
                ) != 0;
                let mut base = 0u64;
                let entry = RtlLookupFunctionEntry(addr, &mut base, std::ptr::null_mut());
                let begin = if entry.is_null() { addr.wrapping_sub(base) } else { u64::from((*entry).BeginAddress) };
                if found {
                    let mut name = [0u16; 260];
                    let len = GetModuleFileNameW(module, name.as_mut_ptr(), name.len() as u32) as usize;
                    let full = String::from_utf16_lossy(&name[..len.min(name.len())]);
                    let short = full.rsplit(['\\', '/']).next().unwrap_or("?").to_owned();
                    if entry.is_null() {
                        format!("{short}+{:#x}", addr - module as u64)
                    } else {
                        format!("{short}+{begin:#x}")
                    }
                } else if entry.is_null() {
                    format!("title code (no unwind data) {addr:#x}")
                } else {
                    format!("title code +{begin:#x}")
                }
            }
        }
    }

    impl Resolve for Symbols {
        fn function(&mut self, addr: u64) -> String {
            if let Some(name) = self.functions.get(&addr) {
                return name.clone();
            }
            let name = if self.ours(addr) {
                let mut shown = None;
                if self.loaded {
                    self.buffer.fill(0);
                    let info = self.buffer.as_mut_ptr().cast::<SYMBOL_INFO>();
                    // SAFETY: the buffer holds a SYMBOL_INFO and 1,024 bytes of name.
                    unsafe {
                        (*info).SizeOfStruct = size_of::<SYMBOL_INFO>() as u32;
                        (*info).MaxNameLen = 1000;
                        let mut shift = 0u64;
                        if SymFromAddr(self.process, addr, &mut shift, info) != 0 {
                            let len = ((*info).NameLen as usize).min(1000);
                            let bytes = std::slice::from_raw_parts((*info).Name.as_ptr().cast::<u8>(), len);
                            shown = Some(String::from_utf8_lossy(bytes).into_owned());
                        }
                    }
                }
                shown.unwrap_or_else(|| format!("m1lk+{:#x} (no PDB)", addr - self.ours.0))
            } else {
                Self::other(addr)
            };
            self.functions.insert(addr, name.clone());
            name
        }

        fn line(&mut self, addr: u64) -> Option<String> {
            if !self.loaded || !self.ours(addr) {
                return None;
            }
            if let Some(line) = self.lines.get(&addr) {
                return line.clone();
            }
            let mut line: IMAGEHLP_LINE64 = unsafe { std::mem::zeroed() };
            line.SizeOfStruct = size_of::<IMAGEHLP_LINE64>() as u32;
            let mut shift = 0u32;
            // SAFETY: dbghelp fills `line`; its file name is a C string it owns.
            let text = unsafe {
                if SymGetLineFromAddr64(self.process, addr, &mut shift, &mut line) != 0 && !line.FileName.is_null() {
                    let file = std::ffi::CStr::from_ptr(line.FileName.cast()).to_string_lossy().into_owned();
                    let short = file.rsplit(['\\', '/']).next().unwrap_or("?").to_owned();
                    Some(format!("{short}:{}", line.LineNumber))
                } else {
                    None
                }
            };
            self.lines.insert(addr, text.clone());
            text
        }
    }

    struct State {
        targets: Vec<Target>,
        symbols: Option<Symbols>,
    }

    // SAFETY: the handles are process-wide values any thread may use; the state is behind a mutex.
    unsafe impl Send for State {}

    static STATE: std::sync::Mutex<State> = std::sync::Mutex::new(State { targets: Vec::new(), symbols: None });
    static STARTED: AtomicBool = AtomicBool::new(false);

    fn report_path() -> std::path::PathBuf {
        let dir = std::env::var_os("VICTORY_SAMPLE_DIR")
            .filter(|v| !v.is_empty())
            .map(std::path::PathBuf::from)
            .or_else(|| std::env::current_exe().ok().and_then(|e| e.parent().map(std::path::Path::to_path_buf)))
            .unwrap_or_default();
        dir.join(format!("sample-prof-{}.txt", std::process::id()))
    }

    /// Rewrites the report from everything counted so far.
    fn write_report() {
        let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
        let state = &mut *state;
        let stats: Vec<ThreadStats> = state
            .targets
            .iter()
            .map(|t| ThreadStats {
                tid: t.tid,
                name: if t.name.is_empty() { format!("tid-{}", t.tid) } else { t.name.clone() },
                idle: t.idle,
                stacks: t.stacks.iter().map(|(k, v)| (k.clone(), *v)).collect(),
            })
            .collect();
        let symbols = state.symbols.get_or_insert_with(Symbols::new);
        let text = format_report(&stats, symbols, 60, 40);
        let head = format!(
            "sampling profile of process {}: every {} ms, {} frames deep; cumulative\n\n",
            std::process::id(),
            interval_ms(),
            MAX_FRAMES
        );
        let _ = std::fs::write(report_path(), head + &text);
    }

    fn interval_ms() -> u64 {
        static MS: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
        *MS.get_or_init(|| std::env::var("VICTORY_SAMPLE_MS").ok().and_then(|v| v.trim().parse().ok()).filter(|&v| v > 0).unwrap_or(1))
    }

    fn wanted(name: &str, tid: u32) -> bool {
        static FILTER: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
        let filter = FILTER.get_or_init(|| {
            std::env::var("VICTORY_SAMPLE_THREADS")
                .map(|v| v.split(',').map(|w| w.trim().to_lowercase()).filter(|w| !w.is_empty()).collect())
                .unwrap_or_default()
        });
        if filter.is_empty() {
            return true;
        }
        let shown = if name.is_empty() { format!("tid-{tid}") } else { name.to_lowercase() };
        filter.iter().any(|w| shown.contains(w.as_str()))
    }

    fn sampler() {
        // Not while the title loads: a thread suspended inside the loader's or the
        // unwind tables' lock would hold up the lookups that follow.
        let delay = std::env::var("VICTORY_SAMPLE_DELAY_S").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(20u64);
        std::thread::sleep(Duration::from_secs(delay));
        // SAFETY: a timer handle of our own.
        let timer = unsafe { CreateWaitableTimerExW(std::ptr::null(), std::ptr::null(), CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, TIMER_ALL_ACCESS) };
        let me = unsafe { GetCurrentThreadId() };
        SAMPLER_TID.store(me, Ordering::SeqCst);
        // SAFETY: registers `guard`, which only acts on this thread.
        unsafe { AddVectoredExceptionHandler(1, Some(guard)) };
        let interval = interval_ms();
        let mut ids = Vec::new();
        let mut ctx: Box<CONTEXT> = Box::new(unsafe { std::mem::zeroed() });
        let mut frames = [0u64; MAX_FRAMES];
        let mut next_list = Instant::now();
        let mut next_report = Instant::now() + Duration::from_secs(30);
        let mut skipped: Vec<u32> = Vec::new();
        while !STOP.load(Ordering::SeqCst) {
            if timer.is_null() {
                std::thread::sleep(Duration::from_millis(interval));
            } else {
                let due = -((interval * 10_000) as i64);
                // SAFETY: our timer; a relative due time in 100 ns units.
                unsafe {
                    SetWaitableTimer(timer, &due, 0, None, std::ptr::null(), 0);
                    WaitForSingleObject(timer, 100);
                }
            }
            let now = Instant::now();
            let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
            if now >= next_list {
                next_list = now + Duration::from_secs(1);
                thread_ids(&mut ids);
                // The counts of a finished thread stay.
                state.targets.retain(|t| ids.contains(&t.tid) || !t.stacks.is_empty());
                for t in state.targets.iter_mut() {
                    if t.name.is_empty() && !t.handle.is_null() {
                        t.name = thread_name(t.handle);
                    }
                }
                for &tid in &ids {
                    if tid == me || skipped.contains(&tid) || state.targets.iter().any(|t| t.tid == tid) {
                        continue;
                    }
                    // SAFETY: opening a thread of our own process.
                    let handle = unsafe { OpenThread(THREAD_SUSPEND_RESUME | THREAD_GET_CONTEXT | THREAD_QUERY_INFORMATION | THREAD_QUERY_LIMITED_INFORMATION, 0, tid) };
                    if handle.is_null() {
                        skipped.push(tid);
                        continue;
                    }
                    let name = thread_name(handle);
                    if !wanted(&name, tid) {
                        // SAFETY: the handle was just opened.
                        unsafe { CloseHandle(handle) };
                        skipped.push(tid);
                        continue;
                    }
                    let (stack_hi, stack_lo) = stack_bounds(handle);
                    state.targets.push(Target { tid, handle, name, stack_hi, stack_lo, cycles: 0, idle: 0, stacks: HashMap::new() });
                }
            }
            let mut at = 0;
            while at < state.targets.len() {
                let n = look(&mut state.targets[at], &mut ctx, &mut frames);
                if n > 0 {
                    // The thread runs again: counting may allocate now.
                    let stacks = &mut state.targets[at].stacks;
                    match stacks.get_mut(&frames[..n]) {
                        Some(count) => *count += 1,
                        None => {
                            stacks.insert(frames[..n].to_vec(), 1);
                        }
                    }
                }
                at += 1;
            }
            drop(state);
            if now >= next_report {
                next_report = now + Duration::from_secs(30);
                write_report();
            }
        }
        DEAD.store(true, Ordering::SeqCst);
    }

    /// Starts the sampling thread (once), if `M1LK_SAMPLE_PROF=1`.
    pub fn start() {
        if !on() || STARTED.swap(true, Ordering::SeqCst) {
            return;
        }
        match std::thread::Builder::new().name("victory-sample-prof".into()).spawn(sampler) {
            Ok(_) => tracing::info!(report = %report_path().display(), "the sampling profiler is on (M1LK_SAMPLE_PROF=1)"),
            Err(error) => tracing::warn!(%error, "the sampling profiler could not start"),
        }
    }

    /// Writes the report now (at the end of a run), if the profiler is on.
    pub fn finish() {
        if on() && STARTED.load(Ordering::SeqCst) {
            // Stop sampling first: no thread is to be suspended while modules unload.
            STOP.store(true, Ordering::SeqCst);
            let until = Instant::now() + Duration::from_millis(500);
            while !DEAD.load(Ordering::SeqCst) && Instant::now() < until {
                std::thread::sleep(Duration::from_millis(5));
            }
            // A sampler that faulted may have died holding the counts' lock: no report then.
            if STATE.try_lock().is_ok() {
                write_report();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake;
    impl Resolve for Fake {
        fn function(&mut self, addr: u64) -> String {
            format!("fn_{}", addr / 0x100)
        }
        fn line(&mut self, addr: u64) -> Option<String> {
            (addr < 0x1000).then(|| format!("a.rs:{}", addr % 0x100))
        }
    }

    #[test]
    fn report_counts_leaf_and_inclusive_per_thread() {
        let threads = vec![
            ThreadStats {
                tid: 7,
                name: "victory-relay".into(),
                idle: 4,
                // Three samples in fn_1 (two stacks at different addresses), one in fn_2; fn_9 is the caller of all.
                stacks: vec![
                    (vec![0x104, 0x900], 2),
                    (vec![0x1a0, 0x2a0, 0x900], 1),
                    (vec![0x210, 0x900], 1),
                ],
            },
            ThreadStats { tid: 8, name: "quiet".into(), idle: 50, stacks: vec![] },
        ];
        let text = format_report(&threads, &mut Fake, 5, 5);
        assert!(text.contains("== victory-relay (tid 7): 4 samples on the processor, 4 looks where it had not run (50.0% of looks busy)"));
        assert!(!text.contains("quiet"));
        let (leaf, inclusive) = text.split_once("-- top 5 inclusive").unwrap();
        let has = |part: &str, percent: &str, name: &str| {
            part.lines().any(|l| l.contains(percent) && l.ends_with(name))
        };
        assert!(has(leaf, "75.00%", "  fn_1"));
        assert!(has(leaf, "25.00%", "  fn_2"));
        // The caller is on all four stacks, counted once each.
        assert!(has(inclusive, "100.00%", "  fn_9"));
        assert!(text.contains("fn_1 @ a.rs:4"));
        assert_eq!(format_report(&[], &mut Fake, 5, 5), "no samples yet\n");
    }

    #[test]
    fn stack_pointer_is_checked_against_bounds() {
        assert!(stack_pointer_ok(0x1000, 0x800, 0x2000));
        assert!(!stack_pointer_ok(0x1004, 0x800, 0x2000));
        assert!(!stack_pointer_ok(0x700, 0x800, 0x2000));
        assert!(!stack_pointer_ok(0x1ff8, 0x800, 0x2000));
        assert!(!stack_pointer_ok(0x1000, 0, 0));
        assert!(!stack_pointer_ok(u64::MAX - 7, 0x800, u64::MAX));
    }

    #[test]
    fn function_entry_must_cover_pc_and_have_plain_unwind_data() {
        assert!(entry_ok(0x100, 0x200, 0x3000, 0x150));
        assert!(!entry_ok(0x100, 0x200, 0x3000, 0x200));
        assert!(!entry_ok(0x100, 0x200, 0x3000, 0xff));
        assert!(!entry_ok(0x200, 0x100, 0x3000, 0x150));
        assert!(!entry_ok(0x100, 0x200, 0x3001, 0x150));
        assert!(!entry_ok(0x100, 0x200, 0, 0x150));
    }

    #[test]
    fn unwind_header_is_validated() {
        // version 1, two codes
        let ok = [1u8, 0, 2, 0, 0x04, 0x01, 0x08, 0x04];
        assert!(unwind_info_ok(&ok, 64));
        assert!(!unwind_info_ok(&ok, 6));
        let mut bad_version = ok;
        bad_version[0] = 4;
        assert!(!unwind_info_ok(&bad_version, 64));
        let mut machine_frame = ok;
        machine_frame[5] = 0x0a;
        assert!(!unwind_info_ok(&machine_frame, 64));
        assert!(!unwind_info_ok(&ok[..3], 64));
        assert!(!unwind_info_ok(&ok[..6], 64));
    }
}
