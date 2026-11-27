//! M1LK Skim Milk (`VICTORY_HEAP_LAZY=1`, alias `M1LK_HEAP_LAZY`; off by
//! default): the title's big committed heaps are reserved only and
//! committed on first touch, a chunk at a time.
//!
//! Plainly: the title commits ~2.9 GB of heaps up front but touches well
//! under half; untouched chunks now cost no commit charge or RAM.
//!
//! - `xmem` reserves a fresh `XMemVirtualAlloc(MEM_COMMIT)` of at least
//!   `VICTORY_HEAP_LAZY_MIN_MB` (default 4) MiB, read-write, not 32-bit,
//!   not a placeholder, and notes it here ([`add`]). Write watching is a
//!   reservation attribute, so it is kept.
//! - A first touch faults into the GPU side's vectored handler
//!   (`crate::watch`), which asks [`on_fault`]: the chunk
//!   (`VICTORY_HEAP_LAZY_CHUNK_KB`, default 64) is committed and the access
//!   goes on. A fresh commit is zeros, as the title expects.
//! - Kernel and runtime writes into title memory already pass
//!   `crate::lazy::ensure`, which calls [`ensure`] (a kernel write fails on
//!   a reservation instead of faulting); so do the in-place heap openers.
//! - Host reads (`ReadProcessMemory`) of an untouched chunk fail and the
//!   copy paths read zeros there: what the memory holds.
//! - Each chunk is committed once (a bit per chunk): a commit never runs
//!   over committed pages, whose protection (write guards, the title's own
//!   `VirtualProtect`) it would reset.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering::Relaxed};
use std::sync::{LazyLock, RwLock};
use std::time::Instant;

/// A lazily committed heap.
#[derive(Debug)]
struct Heap {
    end: usize,
    protect: u32,
    /// A bit per chunk, set once committed.
    done: Vec<u64>,
}

impl Heap {
    fn is_done(&self, chunk: usize) -> bool {
        (self.done[chunk / 64] >> (chunk % 64)) & 1 != 0
    }
    fn set_done(&mut self, chunk: usize) {
        self.done[chunk / 64] |= 1 << (chunk % 64);
    }
}

/// The lazy heaps by start address, and the chunk size. Pure: no host
/// calls, so it is tested alone.
#[derive(Debug)]
struct Set {
    chunk: usize,
    heaps: BTreeMap<usize, Heap>,
}

impl Set {
    fn new(chunk: usize) -> Self {
        Self { chunk, heaps: BTreeMap::new() }
    }

    /// Notes `[at, at + len)` lazy; any heap it overlaps is gone (its
    /// address space was released and reserved again).
    fn add(&mut self, at: usize, len: usize, protect: u32) {
        self.remove(at, len);
        let chunks = len.div_ceil(self.chunk);
        let done = vec![0; chunks.div_ceil(64)];
        self.heaps.insert(at, Heap { end: at + len, protect, done });
    }

    /// Forgets every heap overlapping `[at, at + len)`.
    fn remove(&mut self, at: usize, len: usize) {
        let end = at.saturating_add(len);
        let gone: Vec<usize> = self
            .heaps
            .range(..end)
            .rev()
            .take_while(|(_, h)| h.end > at)
            .map(|(&s, _)| s)
            .collect();
        for s in gone {
            self.heaps.remove(&s);
        }
    }

    /// The heap holding `at`, with its start.
    fn find(&self, at: usize) -> Option<(usize, &Heap)> {
        let (&s, h) = self.heaps.range(..=at).next_back()?;
        (at < h.end).then_some((s, h))
    }

    /// The protection of the lazy heap holding `at`, if any.
    fn protect_at(&self, at: usize) -> Option<u32> {
        self.find(at).map(|(_, h)| h.protect)
    }

    /// Runs `(address, bytes, protect)` of the chunks of `[at, at + len)`
    /// in lazy heaps not yet committed, adjacent chunks merged.
    fn pending(&self, at: usize, len: usize) -> Vec<(usize, usize, u32)> {
        let end = at.saturating_add(len);
        let mut runs: Vec<(usize, usize, u32)> = Vec::new();
        let first = self.heaps.range(..=at).next_back().map_or(at, |(&s, _)| s);
        for (&s, h) in self.heaps.range(first..end) {
            if h.end <= at {
                continue;
            }
            let lo = (at.max(s) - s) / self.chunk;
            let hi = (end.min(h.end) - s).div_ceil(self.chunk);
            for c in lo..hi {
                if h.is_done(c) {
                    continue;
                }
                let a = s + c * self.chunk;
                let n = (a + self.chunk).min(h.end) - a;
                match runs.last_mut() {
                    Some(r) if r.0 + r.1 == a && r.2 == h.protect => r.1 += n,
                    _ => runs.push((a, n, h.protect)),
                }
            }
        }
        runs
    }

    /// Notes the chunks of `[at, at + len)` (in one heap) committed.
    fn mark(&mut self, at: usize, len: usize) {
        let chunk = self.chunk;
        let Some((&s, h)) = self.heaps.range_mut(..=at).next_back() else {
            return;
        };
        if at >= h.end {
            return;
        }
        let lo = (at - s) / chunk;
        let hi = ((at + len).min(h.end) - s).div_ceil(chunk);
        for c in lo..hi {
            h.set_done(c);
        }
    }
}

static HEAPS: AtomicUsize = AtomicUsize::new(0);
static RESERVED: AtomicU64 = AtomicU64::new(0);
static COMMITTED: AtomicU64 = AtomicU64::new(0);
static FAULTS: AtomicU64 = AtomicU64::new(0);
static LAST_LOG: AtomicU64 = AtomicU64::new(0);
static START: LazyLock<Instant> = LazyLock::new(Instant::now);

fn chunk_bytes() -> usize {
    let kb = std::env::var("VICTORY_HEAP_LAZY_CHUNK_KB")
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .unwrap_or(64);
    // Whole 64 KiB allocation-granularity pieces, a power of two.
    (kb.max(64) << 10).next_power_of_two()
}

static SET: LazyLock<RwLock<Set>> = LazyLock::new(|| RwLock::new(Set::new(chunk_bytes())));

/// The smallest heap left lazy, if M1LK Skim Milk is on.
pub fn min_bytes() -> Option<usize> {
    static MIN: std::sync::OnceLock<Option<usize>> = std::sync::OnceLock::new();
    *MIN.get_or_init(|| {
        if !std::env::var("VICTORY_HEAP_LAZY").is_ok_and(|v| v.trim() == "1") {
            return None;
        }
        let mb = std::env::var("VICTORY_HEAP_LAZY_MIN_MB")
            .ok()
            .and_then(|v| v.trim().parse::<usize>().ok())
            .unwrap_or(4)
            .max(1);
        LazyLock::force(&START);
        tracing::info!("M1LK Skim Milk: on (min {mb} MB, chunk {} KB)", chunk_bytes() >> 10);
        Some(mb << 20)
    })
}

/// `xmem` reserved `[at, at + len)` for a commit the title asked for: it is
/// committed on first touch with `protect`.
pub fn add(at: usize, len: usize, protect: u32) {
    crate::watch::install();
    let mut s = SET.write().unwrap_or_else(|e| e.into_inner());
    s.add(at, len, protect);
    HEAPS.store(s.heaps.len(), Relaxed);
    drop(s);
    RESERVED.fetch_add(len as u64, Relaxed);
    log_now();
}

/// The protection the title's query reports at `at` if it is in a lazy
/// heap (a reserved page there reads as committed to the title).
pub fn protect_at(at: usize) -> Option<u32> {
    if HEAPS.load(Relaxed) == 0 {
        return None;
    }
    SET.read().unwrap_or_else(|e| e.into_inner()).protect_at(at)
}

/// Commits what is not yet committed of `[at, at + len)`; false if the host
/// refused a piece.
fn commit(at: usize, len: usize) -> bool {
    if HEAPS.load(Relaxed) == 0 || len == 0 {
        return true;
    }
    if SET.read().unwrap_or_else(|e| e.into_inner()).pending(at, len).is_empty() {
        return true;
    }
    use windows_sys::Win32::System::Memory::{MEM_COMMIT, VirtualAlloc};
    let mut s = SET.write().unwrap_or_else(|e| e.into_inner());
    let mut ok = true;
    for (a, n, protect) in s.pending(at, len) {
        // SAFETY: chunks of a reservation xmem made for the title and noted
        // here, none committed before (their bits are clear).
        if unsafe { VirtualAlloc(a as *const _, n, MEM_COMMIT, protect) }.is_null() {
            ok = false;
            continue;
        }
        s.mark(a, n);
        COMMITTED.fetch_add(n as u64, Relaxed);
    }
    drop(s);
    log_now();
    ok
}

/// Before a kernel or runtime write into `[at, at + len)`, or the title's
/// own commit or protection change there.
pub fn ensure(at: usize, len: usize) {
    commit(at, len);
}

/// From the access-violation handler: true if `at` is in a lazy heap and
/// its chunk is committed now (the access goes on). `access`: 0 read,
/// 1 write, 8 execute.
pub(crate) fn on_fault(access: usize, at: usize) -> bool {
    if HEAPS.load(Relaxed) == 0 || access == 8 {
        return false;
    }
    let pending = {
        let s = SET.read().unwrap_or_else(|e| e.into_inner());
        if s.find(at).is_none() {
            return false;
        }
        !s.pending(at, 1).is_empty()
    };
    if pending {
        if !commit(at, 1) {
            return false;
        }
        FAULTS.fetch_add(1, Relaxed);
        return true;
    }
    // Committed already: by another thread since this one faulted (go on),
    // or decommitted or guarded since (the fault is not ours).
    use windows_sys::Win32::System::Memory::{
        MEM_COMMIT, MEMORY_BASIC_INFORMATION, PAGE_EXECUTE_READWRITE, PAGE_READWRITE, VirtualQuery,
    };
    // SAFETY: plain data filled in by the query.
    let mut info: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: a valid out pointer of the right size.
    let n = unsafe { VirtualQuery(at as *const _, &mut info, size_of_val(&info)) };
    n != 0
        && info.State == MEM_COMMIT
        && (info.Protect == PAGE_READWRITE || info.Protect == PAGE_EXECUTE_READWRITE)
}

/// At most one line every ~10 s, when something changed.
fn log_now() {
    let now = START.elapsed().as_millis() as u64;
    let last = LAST_LOG.load(Relaxed);
    if now < last + 10_000 || LAST_LOG.compare_exchange(last, now, Relaxed, Relaxed).is_err() {
        return;
    }
    tracing::info!(
        "M1LK Skim Milk: {} MB reserved lazily, {} MB committed on touch, {} faults",
        RESERVED.load(Relaxed) >> 20,
        COMMITTED.load(Relaxed) >> 20,
        FAULTS.load(Relaxed)
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    const C: usize = 64 << 10;

    #[test]
    fn finds_heaps_and_pending_chunks() {
        let mut s = Set::new(C);
        s.add(0x10_0000, 4 * C, 4);
        assert_eq!(s.protect_at(0x10_0000 + C), Some(4));
        assert_eq!(s.protect_at(0x10_0000 + 4 * C), None);
        assert_eq!(s.protect_at(0x0f_ffff), None);
        // Two whole chunks, from inside the first.
        assert_eq!(s.pending(0x10_0000 + 10, C), vec![(0x10_0000, 2 * C, 4)]);
        s.mark(0x10_0000 + C, C);
        assert_eq!(
            s.pending(0x10_0000, 4 * C),
            vec![(0x10_0000, C, 4), (0x10_0000 + 2 * C, 2 * C, 4)]
        );
        // Wider than the heap: only the heap's chunks.
        s.mark(0x10_0000, C);
        assert_eq!(s.pending(0, 1 << 30), vec![(0x10_0000 + 2 * C, 2 * C, 4)]);
    }

    #[test]
    fn a_short_tail_chunk_and_reuse_of_the_address() {
        let mut s = Set::new(C);
        s.add(0x20_0000, C + 0x1000, 4);
        assert_eq!(s.pending(0x20_0000 + C, 1), vec![(0x20_0000 + C, 0x1000, 4)]);
        s.mark(0x20_0000, C + 0x1000);
        assert!(s.pending(0x20_0000, C + 0x1000).is_empty());
        // Released and reserved again, overlapping: the old heap is gone.
        s.add(0x20_0000 + C, 2 * C, 0x40);
        assert_eq!(s.protect_at(0x20_0000), None);
        assert_eq!(s.pending(0x20_0000, 4 * C), vec![(0x20_0000 + C, 2 * C, 0x40)]);
    }

    #[test]
    fn neighbouring_heaps_stay_apart() {
        let mut s = Set::new(C);
        s.add(0x40_0000, 2 * C, 4);
        s.add(0x40_0000 + 2 * C, 2 * C, 4);
        assert_eq!(s.heaps.len(), 2);
        assert_eq!(s.pending(0x40_0000 + C, 2 * C), vec![(0x40_0000 + C, 2 * C, 4)]);
        s.remove(0x40_0000 + 3 * C, 1);
        assert_eq!(s.heaps.len(), 1);
    }
}
