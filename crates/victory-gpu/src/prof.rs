//! (Neptune) Sub-stage profiler of the frame's critical path, off by default (VICTORY_SUBMIT_PROF=1).
//!
//! The frame line splits a frame into "preparing", "on the GPU" and "copying back", and in a
//! heavy scene (4,600 draws, 150-180 ms a frame) most of the submitting thread's time was in
//! none of them: the wait for the list ahead of the frame (36 ms), the pass over records the
//! title copied into its heap (33 ms, before "preparing" starts) and some 60 ms of
//! "preparing" that was neither copies nor textures. This module names every stage of the
//! thread that runs the queues' lines (`victory-submit`) and of the thread that records the
//! list ahead of a frame (`victory-ahead`), in microseconds summed since the last report,
//! and says them in one line of `key=value` pairs (ms a frame) that `neptune-diag.ps1`
//! reads.
//!
//! Cost: one `Instant::now()` pair per stage per submission (some 30 submissions a frame,
//! about 25 stages: well under 0.05 ms a frame). No lock: relaxed atomic adds only, so it
//! takes no part in any lock order.

use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::time::Instant;

/// `VICTORY_SUBMIT_PROF=1` turns the profiler on; unset or anything else: off, no timing taken.
pub(crate) fn on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("VICTORY_SUBMIT_PROF").is_ok_and(|v| v.trim() == "1"))
}

/// A stage whose time is summed.
#[derive(Clone, Copy)]
#[repr(usize)]
pub(crate) enum S {
    /// `Host::submit`: settling the flights before a submission (waited for, or blocked).
    SettleBefore,
    /// `descriptor::sync_heaps`, all of it.
    SyncHeaps,
    /// Of it: the write-watch question of the heaps' changed pages.
    SyncWatch,
    /// Of it: reading the changed pages and choosing the slots to look at (summed over threads).
    SyncPages,
    /// Of it: comparing and mirroring the chosen records (summed over threads).
    SyncMirror,
    /// `sweep` (and the settle made for it).
    Sweep,
    /// `descriptor::make_waiting`.
    MakeWaiting,
    /// `shadow::preparing` and `stamp::advance`.
    PreMisc,
    /// The submission's one write-watch asking (`VICTORY_ASK_ONCE`), with the stale check.
    AskOnce,
    /// Deferred steps, by kind, without the refreshes they flush (those are `Refresh`).
    StepCopy,
    StepFill,
    StepRun,
    StepHost,
    StepGpuWrite,
    StepTexture,
    /// Refreshes of host copies (`Batch::flush` and `flush_one`), wherever they run.
    Refresh,
    /// The step loop's own overhead: what the deferred steps took beyond the kinds above.
    StepLoop,
    /// `shadow::take_snapshots`.
    Snapshots,
    /// Flights settled in the middle of a submission (stale under flights, card results low).
    SettleMid,
    /// `Host::card_before`.
    CardBefore,
    /// `ExecuteCommandLists` and the fence signal of a submission sent ahead.
    Execute,
    /// What a submission sent ahead collects after its lists went (declared writes).
    Tail,
    /// `Host::execute_late` after `submit`: the line of flights, waits for the relay, landing.
    LateTail,
    /// Synchronous submissions: the wait for the card.
    SyncWait,
    /// `victory-submit` waiting for the list ahead of the frame (`queue::ahead_of_frame`).
    WaitAhead,
    /// `victory-submit` at a present, waiting for the frame's lists to be translated.
    WaitTranslations,
    /// `victory-submit` running the present job.
    Present,
    /// `victory-submit` with nothing to do (waiting to be woken).
    Idle,
    /// `release_held`, all of it, summed over every thread that calls it (the title's
    /// `ExecuteCommandLists`/`Signal`/`Wait`/`Present`, `victory-late-wait`, ...): it nests the
    /// held submissions it runs (`settle`, `sweep`, ... are inside it) and lock waits, so it
    /// can exceed the frame. Parts: the `lines_*` slots.
    Lines,
    /// `victory-ahead`: planning the pass over the heap's buffer views.
    AheadPlan,
    /// `victory-ahead`: the slots of the pass (mirror, host copy lookup, bookkeeping).
    AheadSlots,
    /// `victory-ahead`: the spans of the host copies the views read, and their refreshes.
    AheadSpans,
    /// `victory-ahead`: the pass over textures picked by index.
    AheadTextures,
    /// Of `lines`: the part spent on threads named `victory-*` (the rest is the title's own
    /// threads: `release_held` runs on whoever calls it, not on one thread of ours).
    LinesVictory,
    /// Of `lines`: waiting for `release_held`'s one-caller gate (`RELEASING`).
    LinesGate,
    /// Of `lines`: waiting for a queue's `held` lock (the title's submissions take it too).
    LinesHeldLock,
    /// Of `lines`: the fence and memory-value checks of what queues hold.
    LinesCheck,
    /// Of `lines`: `Host::execute` / `execute_late` of a held submission, all of it (the
    /// submission stages `settle`, `sweep`, `snapshots`, ... are inside this).
    LinesRun,
    /// Of `lines`: held signals (`late_signal` / `fence::set`).
    LinesSignal,
    /// Of `lines`: host lists given back (`list::recycle`).
    LinesRecycle,
    /// Of `lines`: what none of the `lines_*` slots above took (bookkeeping, logging).
    LinesOther,
    /// `Host::submit`: waiting for the submission lock (`Host::submitted`), any caller.
    SubmitLock,
    /// `Host::submit_part`: from after `make_waiting` to the lists being ready to send
    /// (translation of the title's lists into ours, snapshots, the card's own list).
    SubmitBuild,
    /// `Host::execute_late` after the submission: the settle or relay wait for the card.
    LateCardWait,
    /// `Host::execute_late`: landing finished flights (copies back to title memory).
    LateLand,
    /// `Host::submit_part`: the runs' preludes, planned and recorded (sparse uploads with them).
    Preludes,
}

/// Stages ([`S`]).
const STAGES: usize = S::Preludes as usize + 1;

/// Counts summed with the stages.
#[derive(Clone, Copy)]
#[repr(usize)]
pub(crate) enum C {
    /// Submissions prepared (`Host::submit`).
    Submissions,
    /// Deferred steps run.
    Steps,
    /// Slots the pass over the heap's buffer views looked at.
    AheadDue,
    /// Records `sync_heaps` mirrored.
    SyncMirrored,
    /// `release_held` calls (any thread).
    LinesCalls,
    /// Held submissions `release_held` ran.
    LinesRuns,
    /// Passes of the M1LK Skimmer thread (`crate::queue::skim`).
    SkimPasses,
    /// (M1LK Pour) Chunks of a submission sent to the card ahead of its last.
    PourChunks,
    /// (M1LK Rennet) `VirtualProtect` calls of the coalesced re-guard passes.
    RennetProtectCalls,
    /// (M1LK Rennet) 64 KiB blocks those passes made read-only again.
    RennetBlocks,
}

const COUNTS: usize = C::RennetBlocks as usize + 1;

static US: [AtomicU64; STAGES] = [const { AtomicU64::new(0) }; STAGES];
static N: [AtomicU64; COUNTS] = [const { AtomicU64::new(0) }; COUNTS];

/// Adds the time since `since` to `stage`; returns now, for the next stage.
#[inline]
pub(crate) fn lap(stage: S, since: Instant) -> Instant {
    if !on() {
        return since;
    }
    let now = Instant::now();
    bump(stage, now.saturating_duration_since(since).as_micros() as u64);
    now
}

/// Adds `micros` to `stage`.
#[inline]
pub(crate) fn add_us(stage: S, micros: u64) {
    if !on() {
        return;
    }
    bump(stage, micros);
}

// ---- (M1LK Crit Path) the title thread that presents: where its frame goes ----

/// `VICTORY_CRIT_PATH=1` (with the profiler on): the stages run on the thread that calls the
/// title's present are summed apart, with its present-to-present time and its CPU time.
pub(crate) fn crit_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| {
        let on = on() && std::env::var("VICTORY_CRIT_PATH").is_ok_and(|v| v.trim() == "1");
        if on {
            tracing::info!("M1LK Crit Path: on (the thread that calls the title's present)");
        }
        on
    })
}

static CRIT_US: [AtomicU64; STAGES] = [const { AtomicU64::new(0) }; STAGES];
/// Nanoseconds: present-to-present on the thread, inside `present`, CPU time, recording
/// (`tprof`, exclusive), `Lines` inside `present` (us); and the presents counted.
static CRIT_FRAME_NS: AtomicU64 = AtomicU64::new(0);
static CRIT_PRESENT_NS: AtomicU64 = AtomicU64::new(0);
static CRIT_CPU_NS: AtomicU64 = AtomicU64::new(0);
static CRIT_RECORD_NS: AtomicU64 = AtomicU64::new(0);
static CRIT_LINES_IN_PRESENT_US: AtomicU64 = AtomicU64::new(0);
static CRIT_FRAMES: AtomicU64 = AtomicU64::new(0);
/// The thread that presents: the first to call `present`.
static CRIT_THREAD: AtomicU64 = AtomicU64::new(0);

thread_local! {
    static THIS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    static IN_PRESENT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// The start of the last present on this thread, and its CPU time then (ns).
    static LAST: std::cell::Cell<Option<(Instant, u64)>> = const { std::cell::Cell::new(None) };
}

fn this_thread() -> u64 {
    THIS.with(|t| {
        if t.get() == 0 {
            static NEXT: AtomicU64 = AtomicU64::new(1);
            t.set(NEXT.fetch_add(1, Relaxed));
        }
        t.get()
    })
}

/// Whether this is the presenting thread (once one is known).
#[inline]
pub(crate) fn is_crit() -> bool {
    crit_on() && CRIT_THREAD.load(Relaxed) == this_thread()
}

/// CPU time (user and kernel) of this thread in ns; 0 where it cannot be read.
fn thread_cpu_ns() -> u64 {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::FILETIME;
        use windows_sys::Win32::System::Threading::{GetCurrentThread, GetThreadTimes};
        let zero = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
        let (mut create, mut exit, mut kernel, mut user) = (zero, zero, zero, zero);
        // SAFETY: the pseudo handle of this thread and four out values.
        let ok = unsafe { GetThreadTimes(GetCurrentThread(), &mut create, &mut exit, &mut kernel, &mut user) };
        let ticks = |t: FILETIME| u64::from(t.dwHighDateTime) << 32 | u64::from(t.dwLowDateTime);
        if ok != 0 { (ticks(kernel) + ticks(user)) * 100 } else { 0 }
    }
    #[cfg(not(windows))]
    0
}

/// The title's present is running on this thread: the frame ends here for the previous one.
pub(crate) struct CritPresent(Option<Instant>);

impl CritPresent {
    #[inline]
    pub(crate) fn new() -> CritPresent {
        if !crit_on() {
            return CritPresent(None);
        }
        let me = this_thread();
        let _ = CRIT_THREAD.compare_exchange(0, me, Relaxed, Relaxed);
        if CRIT_THREAD.load(Relaxed) != me {
            return CritPresent(None);
        }
        let now = Instant::now();
        let cpu = thread_cpu_ns();
        if let Some((last, cpu_last)) = LAST.with(std::cell::Cell::get) {
            CRIT_FRAME_NS.fetch_add(now.saturating_duration_since(last).as_nanos() as u64, Relaxed);
            CRIT_CPU_NS.fetch_add(cpu.saturating_sub(cpu_last), Relaxed);
            CRIT_FRAMES.fetch_add(1, Relaxed);
        }
        LAST.with(|l| l.set(Some((now, cpu))));
        IN_PRESENT.with(|p| p.set(true));
        CritPresent(Some(now))
    }
}

impl Drop for CritPresent {
    fn drop(&mut self) {
        if let Some(began) = self.0 {
            CRIT_PRESENT_NS.fetch_add(began.elapsed().as_nanos() as u64, Relaxed);
            IN_PRESENT.with(|p| p.set(false));
        }
    }
}

/// Recording time (exclusive, `tprof`) of `ns` on this thread.
#[inline]
pub(crate) fn crit_record(ns: u64) {
    if is_crit() {
        CRIT_RECORD_NS.fetch_add(ns, Relaxed);
    }
}

/// Adds `us` to `stage`, and to the presenting thread's own sums if this is that thread.
#[inline]
fn bump(stage: S, us: u64) {
    US[stage as usize].fetch_add(us, Relaxed);
    if is_crit() {
        CRIT_US[stage as usize].fetch_add(us, Relaxed);
        if matches!(stage, S::Lines) && IN_PRESENT.with(std::cell::Cell::get) {
            CRIT_LINES_IN_PRESENT_US.fetch_add(us, Relaxed);
        }
    }
}

/// What of a frame the title's own code took on the presenting thread: the frame less
/// `present`, the `lines` run outside it and the recording (all in the same unit).
pub(crate) fn crit_title_time(frame: u64, present: u64, lines: u64, lines_in_present: u64, record: u64) -> u64 {
    frame
        .saturating_sub(present)
        .saturating_sub(lines.saturating_sub(lines_in_present))
        .saturating_sub(record)
}

/// The presenting thread's line, over the presents it made; takes the sums.
fn crit_line() {
    if !crit_on() {
        return;
    }
    let frames = CRIT_FRAMES.swap(0, Relaxed);
    let take = |a: &AtomicU64| a.swap(0, Relaxed);
    let (frame_ns, present_ns, cpu_ns, record_ns, lip_us) = (
        take(&CRIT_FRAME_NS),
        take(&CRIT_PRESENT_NS),
        take(&CRIT_CPU_NS),
        take(&CRIT_RECORD_NS),
        take(&CRIT_LINES_IN_PRESENT_US),
    );
    let us: Vec<u64> = CRIT_US.iter().map(take).collect();
    if frames == 0 {
        return;
    }
    let f = frames as f64;
    let ms_ns = |ns: u64| ns as f64 / 1_000_000.0 / f;
    let ms_us = |us: u64| us as f64 / 1000.0 / f;
    let g = |s: S| us[s as usize];
    let title = crit_title_time(frame_ns, present_ns, g(S::Lines) * 1000, lip_us * 1000, record_ns);
    let fence = g(S::SyncWait) + g(S::LateCardWait);
    let lock = g(S::SubmitLock) + g(S::LinesGate) + g(S::LinesHeldLock);
    let steps = g(S::StepCopy) + g(S::StepFill) + g(S::StepRun) + g(S::StepHost) + g(S::StepGpuWrite) + g(S::StepTexture) + g(S::StepLoop);
    tracing::info!(
        "M1LK Crit Path ms/frame (presents={frames}): present_wait={:.2} record={:.2} settle={:.2} sweep={:.2} \
         refresh={:.2} texture={:.2} build={:.2} submit={:.2} gpu_wait={:.2} present={:.2} | \
         thread_running={:.2} thread_blocked_fence={:.2} thread_blocked_lock={:.2} frame_total={:.2} | \
         lines={:.2} sync_heaps={:.2} make_waiting={:.2} settle_mid={:.2} step_total={:.2}",
        ms_ns(title),
        ms_ns(record_ns),
        ms_us(g(S::SettleBefore)),
        ms_us(g(S::Sweep)),
        ms_us(g(S::Refresh)),
        ms_us(g(S::StepTexture)),
        ms_us(g(S::SubmitBuild)),
        ms_us(g(S::Execute) + g(S::Tail) + g(S::LateTail)),
        ms_us(fence),
        ms_ns(present_ns),
        ms_ns(cpu_ns),
        ms_us(fence),
        ms_us(lock),
        ms_ns(frame_ns),
        ms_us(g(S::Lines)),
        ms_us(g(S::SyncHeaps)),
        ms_us(g(S::MakeWaiting)),
        ms_us(g(S::SettleMid)),
        ms_us(steps),
    );
}

/// Adds `n` to count `c`.
#[inline]
pub(crate) fn count(c: C, n: u64) {
    if !on() {
        return;
    }
    N[c as usize].fetch_add(n, Relaxed);
}

thread_local! {
    /// Microseconds of refreshes run on this thread so far ([`S::Refresh`]): a step that
    /// flushes refreshes has their time taken out of its own.
    static REFRESHED: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Refreshes took `micros` on this thread ([`S::Refresh`]).
#[inline]
pub(crate) fn refreshed(micros: u64) {
    nested(S::Refresh, micros);
}

/// Work of `stage` that ran inside a deferred step took `micros` on this thread (the
/// refreshes, the texture checks a write of the title's flushes): the step's own time has
/// it taken out ([`refreshed_so_far`]).
#[inline]
pub(crate) fn nested(stage: S, micros: u64) {
    bump(stage, micros);
    REFRESHED.with(|r| r.set(r.get() + micros));
}

/// (M1LK Lactometer) Times a list's deferred steps by kind: each step's time, less what
/// nested work ([`nested`]) took inside it, goes to its kind's stage; the loop's own time
/// beyond them to [`S::StepLoop`]. Inert (no clock read) with the profiler off.
pub(crate) struct Steps {
    began: Option<Instant>,
    nested_at_start: u64,
    /// The step being timed: its stage, its start, the nested time at its start.
    current: Option<(S, Instant, u64)>,
    parts: u64,
    steps: u64,
}

impl Steps {
    #[inline]
    pub(crate) fn new() -> Steps {
        let began = on().then(Instant::now);
        let nested_at_start = if began.is_some() { refreshed_so_far() } else { 0 };
        Steps { began, nested_at_start, current: None, parts: 0, steps: 0 }
    }

    /// A step of `stage` starts (the one before it ends).
    #[inline]
    pub(crate) fn step(&mut self, stage: S) {
        if self.began.is_none() {
            return;
        }
        let now = Instant::now();
        self.close(now);
        self.current = Some((stage, now, refreshed_so_far()));
        self.steps += 1;
    }

    fn close(&mut self, now: Instant) {
        if let Some((stage, since, nested_then)) = self.current.take() {
            let nested = refreshed_so_far().saturating_sub(nested_then);
            let us = (now.saturating_duration_since(since).as_micros() as u64).saturating_sub(nested);
            bump(stage, us);
            self.parts += us;
        }
    }
}

impl Drop for Steps {
    fn drop(&mut self) {
        let Some(began) = self.began else {
            return;
        };
        let now = Instant::now();
        self.close(now);
        let nested = refreshed_so_far().saturating_sub(self.nested_at_start);
        let whole = (now.saturating_duration_since(began).as_micros() as u64).saturating_sub(nested);
        bump(S::StepLoop, whole.saturating_sub(self.parts));
        N[C::Steps as usize].fetch_add(self.steps, Relaxed);
    }
}

/// The refresh microseconds this thread has run so far.
#[inline]
pub(crate) fn refreshed_so_far() -> u64 {
    REFRESHED.with(std::cell::Cell::get)
}

/// The names in the line, by stage.
const NAMES: [&str; STAGES] = [
    "settle",
    "sync_heaps",
    "sync_watch",
    "sync_pages",
    "sync_mirror",
    "sweep",
    "make_waiting",
    "pre_misc",
    "ask_once",
    "step_copy",
    "step_fill",
    "step_run",
    "step_host",
    "step_gpu_write",
    "step_texture",
    "refresh",
    "step_loop",
    "snapshots",
    "settle_mid",
    "card_before",
    "execute",
    "tail",
    "late_tail",
    "sync_wait",
    "wait_ahead",
    "wait_translations",
    "present",
    "idle",
    "lines",
    "ahead_plan",
    "ahead_slots",
    "ahead_spans",
    "ahead_textures",
    "lines_victory",
    "lines_gate",
    "lines_held_lock",
    "lines_check",
    "lines_run",
    "lines_signal",
    "lines_recycle",
    "lines_other",
    "submit_lock",
    "submit_build",
    "late_card_wait",
    "late_land",
    "preludes",
];

const COUNT_NAMES: [&str; COUNTS] = [
    "submissions",
    "steps",
    "ahead_due",
    "sync_mirrored",
    "lines_calls",
    "lines_runs",
    "skim_passes",
    "pour_chunks",
    "rennet_protect_calls",
    "rennet_blocks",
];

/// The line, over `frames` frames: every stage in ms a frame, every count a frame.
pub(crate) fn line(frames: f64) {
    if frames <= 0.0 || !on() {
        return;
    }
    use std::fmt::Write;
    let mut text = String::with_capacity(768);
    for (name, us) in NAMES.iter().zip(US.iter()) {
        let _ = write!(text, " {name}={:.2}", us.swap(0, Relaxed) as f64 / 1000.0 / frames);
    }
    for (name, n) in COUNT_NAMES.iter().zip(N.iter()) {
        let _ = write!(text, " {name}={:.1}", n.swap(0, Relaxed) as f64 / frames);
    }
    tracing::info!("submit profile ms/frame:{text}");
    crit_line();
}

/// Adds the time from its making to its drop to a stage.
pub(crate) struct Scope(S, Option<Instant>);

impl Scope {
    #[inline]
    pub(crate) fn new(stage: S) -> Scope {
        Scope(stage, on().then(Instant::now))
    }
}

impl Drop for Scope {
    fn drop(&mut self) {
        if let Some(since) = self.1 {
            lap(self.0, since);
        }
    }
}

/// A span cut into named parts: [`Split::mark`] gives the time since the last mark (or
/// [`Split::skip`]) to a stage; at the drop the whole goes to `whole`, what no mark took to
/// `rest`, and with `victory` the whole also to it on a thread named `victory-*`.
pub(crate) struct Split {
    whole: S,
    rest: S,
    victory: Option<S>,
    start: Option<Instant>,
    last: Option<Instant>,
    parts: u64,
}

impl Split {
    #[inline]
    pub(crate) fn new(whole: S, rest: S, victory: Option<S>) -> Split {
        let start = on().then(Instant::now);
        Split { whole, rest, victory, start, last: start, parts: 0 }
    }

    /// The time since the last mark goes to `stage`.
    #[inline]
    pub(crate) fn mark(&mut self, stage: S) {
        if let Some(last) = self.last {
            let now = Instant::now();
            let us = now.saturating_duration_since(last).as_micros() as u64;
            bump(stage, us);
            self.parts += us;
            self.last = Some(now);
        }
    }

    /// The time since the last mark is left to `rest`.
    #[inline]
    pub(crate) fn skip(&mut self) {
        if self.last.is_some() {
            self.last = Some(Instant::now());
        }
    }
}

impl Drop for Split {
    fn drop(&mut self) {
        let Some(start) = self.start else {
            return;
        };
        let whole = start.elapsed().as_micros() as u64;
        bump(self.whole, whole);
        bump(self.rest, whole.saturating_sub(self.parts));
        if let Some(v) = self.victory
            && on_victory_thread()
        {
            bump(v, whole);
        }
    }
}

/// Whether this thread is one of ours (named `victory-*`).
fn on_victory_thread() -> bool {
    thread_local! {
        static OURS: bool = std::thread::current().name().is_some_and(|n| n.starts_with("victory-"));
    }
    OURS.with(|o| *o)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_time_is_what_the_rest_of_the_frame_leaves() {
        // 100 frame, 20 present, lines 50 (30 of them inside present), record 10.
        assert_eq!(crit_title_time(100, 20, 50, 30, 10), 100 - 20 - 20 - 10);
        assert_eq!(crit_title_time(10, 20, 50, 30, 10), 0);
        assert_eq!(crit_title_time(100, 0, 5, 9, 0), 100);
    }

    #[test]
    fn every_stage_and_count_has_a_name() {
        assert_eq!(NAMES.len(), STAGES);
        assert_eq!(COUNT_NAMES.len(), COUNTS);
        let mut all: Vec<&str> = NAMES.iter().chain(COUNT_NAMES.iter()).copied().collect();
        all.sort_unstable();
        all.dedup();
        assert_eq!(all.len(), STAGES + COUNTS, "names are unique");
    }

    #[test]
    fn steps_take_nested_time_out_and_are_inert_when_off() {
        // Off (the variable is not set in tests): no clock, nothing added.
        if !on() {
            let mut steps = Steps::new();
            steps.step(S::StepCopy);
            assert!(steps.began.is_none() && steps.current.is_none() && steps.steps == 0);
        }
        // The nested counter is per thread and only grows.
        let before = refreshed_so_far();
        nested(S::StepTexture, 7);
        refreshed(5);
        assert_eq!(refreshed_so_far() - before, 12);
    }
}
