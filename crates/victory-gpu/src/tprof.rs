//! (M1LK Translate Prof) CPU time of the translation steps on the recording threads, off by
//! default (`VICTORY_TRANSLATE_PROF=1`, and only with the submit profiler on,
//! `VICTORY_SUBMIT_PROF=1`).
//!
//! The submit profiler ([`crate::prof`]) covers the thread that sends the lists, not the
//! per-draw work of the threads that record them. Here every step of that work (binding the
//! pipeline and root signature, copying descriptor tables, barriers, the indirect argument
//! clamp, ...) sums its CPU time, over all recording threads, into a named bucket; the frame
//! line says them in ms a frame.
//!
//! Buckets are EXCLUSIVE: a [`Scope`] inside another takes its time out of the outer one, so
//! the buckets add up to `total` without double counting. Time in a list call that no scope
//! covers is not in any bucket (calls of the title that we only forward, ...).
//!
//! Cost: with the switch off, one cached bool test per scope. On: two `Instant::now()` and
//! one relaxed atomic add per scope (some 25 ns each), a few dozen scopes per draw at most.
//! The profile itself adds to the draws' cost; read ratios between buckets, not the sum.

use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::time::Instant;

/// `VICTORY_TRANSLATE_PROF=1` with the submit profiler on.
pub(crate) fn on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| crate::prof::on() && std::env::var("VICTORY_TRANSLATE_PROF").is_ok_and(|v| v.trim() == "1"))
}

/// A step whose time is summed.
#[derive(Clone, Copy)]
#[repr(usize)]
pub(crate) enum B {
    /// `SetPipelineState` and the list's bookkeeping of the pipeline.
    PsoBind,
    /// `SetDescriptorHeaps`, the root signature, root constants, root views of a draw.
    RootSet,
    /// `State::tables`: the descriptor tables a draw reads, copied and made.
    DescriptorCopy,
    /// The list's `ResourceBarrier` call of the title (`list::barrier`).
    Barrier,
    /// The enhanced barriers (`list::barrier_groups`).
    BarrierGroups,
    /// The indirect argument clamp (`indirect::bound*`, `record_clamp_as`).
    IndirectClamp,
    /// `State::execute_indirect`, less the clamp and the binds it runs.
    ExecuteIndirect,
    /// `State::draw` / `draw_indexed` / `dispatch_mesh`, less what the other buckets take.
    DrawOther,
    /// `State::dispatch` (compute), less what the other buckets take.
    DispatchOther,
    /// `State::heap_buffers`: the table of the heap's buffer views.
    HeapBuffers,
    /// `list::reset`.
    ListReset,
    /// `list::detach`.
    ListDetach,
    /// `State::bind`, less its parts (the rest of the per-draw bind).
    BindOther,
    /// `State::early_textures`.
    EarlyTextures,
    /// `State::memory_ready` (title memory by address).
    MemoryReady,
    /// Vertex and index buffer lookups, refreshes and sets of `prepare`.
    VertexBuffers,
    /// `list::ensure_state`: transitions of title textures for a draw.
    EnsureState,
    /// `State::root_uav`.
    RootUav,
    /// `State::root_srv_table`.
    RootSrvTable,
}

const BUCKETS: usize = B::RootSrvTable as usize + 1;

/// Counts summed with the buckets.
#[derive(Clone, Copy)]
#[repr(usize)]
pub(crate) enum C {
    Draws,
    Dispatches,
}

const COUNTS: usize = C::Dispatches as usize + 1;

const NAMES: [&str; BUCKETS] = [
    "pso_bind",
    "root_set",
    "descriptor_copy",
    "barrier",
    "barrier_groups",
    "indirect_clamp",
    "execute_indirect",
    "draw_other",
    "dispatch_other",
    "heap_buffers",
    "list_reset",
    "list_detach",
    "bind_other",
    "early_textures",
    "memory_ready",
    "vertex_buffers",
    "ensure_state",
    "root_uav",
    "root_srv_table",
];

static NS: [AtomicU64; BUCKETS] = [const { AtomicU64::new(0) }; BUCKETS];
static N: [AtomicU64; COUNTS] = [const { AtomicU64::new(0) }; COUNTS];
/// Threads that ran a scope since the last line.
static THREADS: AtomicU64 = AtomicU64::new(0);
/// The number of the last line: a thread counts itself once for each.
static EPOCH: AtomicU64 = AtomicU64::new(1);

thread_local! {
    /// Nanoseconds the scopes closed inside the open one took, on this thread.
    static CHILD: Cell<u64> = const { Cell::new(0) };
    /// The epoch this thread last counted itself in.
    static SEEN: Cell<u64> = const { Cell::new(0) };
}

/// Adds the time from its making to its drop to a bucket, less the scopes inside it.
pub(crate) struct Scope {
    bucket: B,
    since: Option<Instant>,
    outer_child: u64,
}

impl Scope {
    #[inline]
    pub(crate) fn new(bucket: B) -> Scope {
        if !on() {
            return Scope { bucket, since: None, outer_child: 0 };
        }
        let epoch = EPOCH.load(Relaxed);
        SEEN.with(|s| {
            if s.get() != epoch {
                s.set(epoch);
                THREADS.fetch_add(1, Relaxed);
            }
        });
        let outer_child = CHILD.with(|c| c.replace(0));
        Scope { bucket, since: Some(Instant::now()), outer_child }
    }
}

impl Drop for Scope {
    #[inline]
    fn drop(&mut self) {
        let Some(since) = self.since else {
            return;
        };
        let whole = since.elapsed().as_nanos() as u64;
        let inner = CHILD.with(Cell::get);
        let own = exclusive(whole, inner);
        NS[self.bucket as usize].fetch_add(own, Relaxed);
        crate::prof::crit_record(own);
        crate::waitlog::ours_add(own);
        // For the scope outside this one, all of this is a child's time.
        CHILD.with(|c| c.set(self.outer_child.saturating_add(whole)));
    }
}

/// What a scope of `whole` ns keeps of it when scopes inside it took `inner`.
pub(crate) fn exclusive(whole: u64, inner: u64) -> u64 {
    whole.saturating_sub(inner)
}

/// Adds `n` to count `c`.
#[inline]
pub(crate) fn count(c: C, n: u64) {
    if on() {
        N[c as usize].fetch_add(n, Relaxed);
    }
}

/// `ns` over `frames` frames as ms a frame.
pub(crate) fn ms_per_frame(ns: u64, frames: f64) -> f64 {
    if frames <= 0.0 { 0.0 } else { ns as f64 / 1_000_000.0 / frames }
}

/// Merges two sets of bucket sums (per thread sums into the total).
#[cfg(test)]
fn merge(into: &mut [u64], from: &[u64]) {
    for (a, b) in into.iter_mut().zip(from) {
        *a = a.saturating_add(*b);
    }
}

/// The frame line, over `frames` frames; takes the sums.
pub(crate) fn line(frames: f64) {
    if frames <= 0.0 || !on() {
        return;
    }
    use std::fmt::Write;
    let threads = THREADS.swap(0, Relaxed);
    EPOCH.fetch_add(1, Relaxed);
    let mut text = String::with_capacity(512);
    let mut total = 0u64;
    for (name, ns) in NAMES.iter().zip(NS.iter()) {
        let ns = ns.swap(0, Relaxed);
        total = total.saturating_add(ns);
        let _ = write!(text, " {name}={:.2}", ms_per_frame(ns, frames));
    }
    let draws = N[C::Draws as usize].swap(0, Relaxed);
    let dispatches = N[C::Dispatches as usize].swap(0, Relaxed);
    let calls = (draws + dispatches).max(1) as f64;
    let _ = write!(
        text,
        " total={:.2} draws={:.0} dispatches={:.0} us_per_call={:.2}",
        ms_per_frame(total, frames),
        draws as f64 / frames,
        dispatches as f64 / frames,
        total as f64 / 1000.0 / calls,
    );
    tracing::info!("M1LK Translate Prof ms/frame (threads={threads}):{text}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_bucket_and_count_has_a_name() {
        assert_eq!(NAMES.len(), BUCKETS);
        assert_eq!(COUNTS, 2);
        let mut all = NAMES.to_vec();
        all.sort_unstable();
        all.dedup();
        assert_eq!(all.len(), BUCKETS, "names are unique");
    }

    #[test]
    fn exclusive_time_takes_the_inner_scopes_out() {
        assert_eq!(exclusive(100, 30), 70);
        assert_eq!(exclusive(10, 30), 0);
    }

    #[test]
    fn ns_become_ms_a_frame() {
        assert_eq!(ms_per_frame(2_000_000, 2.0), 1.0);
        assert_eq!(ms_per_frame(5, 0.0), 0.0);
    }

    #[test]
    fn sums_merge_and_saturate() {
        let mut a = [1, u64::MAX, 3];
        merge(&mut a, &[2, 5, 0]);
        assert_eq!(a, [3, u64::MAX, 3]);
    }

    #[test]
    fn scopes_are_inert_when_off() {
        if !on() {
            let s = Scope::new(B::PsoBind);
            assert!(s.since.is_none());
        }
    }
}
