//! (M1LK Churn Clock) GPU timestamps around the host queue's submissions, off by default
//! (`VICTORY_CHURN_CLOCK=1`, and only with the submit profiler on, `VICTORY_SUBMIT_PROF=1`).
//!
//! The card's clock ([`crate::host`]) says how long our queue had work outstanding; this says
//! what that time was. Around every `ExecuteCommandLists` of the host queue a tiny list stamps
//! the card's clock before, and another stamps it after and resolves both stamps into a
//! read-back ring, all in the same call, so no extra submission is made and the order of the
//! lists is untouched. Per frame it sums, in ms:
//!
//! * `exec`: from the first stamp to the second (the lists running, with whatever the card
//!   overlaps or waits for inside them),
//! * `queue_lat`: from the CPU sending the lists to the first stamp (the lists waiting behind
//!   other work of the card, ours or another process's),
//! * `gpu_idle`: from one submission's second stamp to the next one's first (the card with
//!   none of our lists to run).
//!
//! With `VICTORY_CHURN_LISTS=1` as well, a tiny stamp list also goes between every two lists of the
//! call (`[stamp0, L1, stamp1, ... Ln, stampN]`, resolved once), so each command list of the title
//! gets its own card duration. They are summed by what the host knows of the list (its place in
//! the submission, its render target, its draw, dispatch and copy counts) and the top eight and a
//! histogram are said at the frame line.
//!
//! It does not use `VICTORY_GPU_TIMING` (that serialises lists). The other queues of a run are
//! the copy queue of the texture worker (a copy queue cannot resolve stamps, so only its wall
//! time from send to fence is counted) and the rigs of the tests.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering::Relaxed};
use std::sync::{Mutex, OnceLock};

use windows::Win32::Graphics::Direct3D12::*;
use windows::core::Interface;

/// Submissions the ring holds stamps for at once (two stamps each).
const SLOTS: usize = 1024;

/// Stamps (one more than the lists) the per-list ring holds at once.
const QUERIES: usize = 8192;

/// `VICTORY_CHURN_LISTS=1` as well as the clock: a stamp between every two lists.
pub(crate) fn lists() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| on() && std::env::var("VICTORY_CHURN_LISTS").is_ok_and(|v| v.trim() == "1"))
}

/// An env number, or `default` when unset or not a number.
fn env_number(name: &str, default: u64) -> u64 {
    std::env::var(name).ok().and_then(|v| v.trim().parse().ok()).unwrap_or(default)
}

/// `VICTORY_CHURN_LISTS_EVERY=N`: one frame in N is stamped per list (default 30; 1 is every frame).
fn every() -> u64 {
    static N: OnceLock<u64> = OnceLock::new();
    *N.get_or_init(|| env_number("VICTORY_CHURN_LISTS_EVERY", 30))
}

/// `VICTORY_CHURN_LISTS_MAX=K`: at most K lists of a submission are stamped (default 64).
fn most() -> usize {
    static N: OnceLock<usize> = OnceLock::new();
    *N.get_or_init(|| env_number("VICTORY_CHURN_LISTS_MAX", 64).clamp(1, 4096) as usize)
}

/// Whether `frame` is one of the sampled ones (one in `every`; 0 and 1 mean all).
pub(crate) fn sampled_frame(frame: u64, every: u64) -> bool {
    every <= 1 || frame.is_multiple_of(every)
}

/// `VICTORY_CHURN_CLOCK=1` with the submit profiler on.
pub(crate) fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| crate::prof::on() && std::env::var("VICTORY_CHURN_CLOCK").is_ok_and(|v| v.trim() == "1"))
}

/// What a host list recorded, counted as the title records it.
#[derive(Default)]
pub(crate) struct ListStats {
    draws: AtomicU32,
    dispatches: AtomicU32,
    copies: AtomicU32,
    /// The first render target's width and height, 16 bits each (0: none set).
    target: AtomicU32,
}

impl ListStats {
    pub(crate) fn draw(&self) {
        self.draws.fetch_add(1, Relaxed);
    }
    pub(crate) fn dispatch(&self) {
        self.dispatches.fetch_add(1, Relaxed);
    }
    pub(crate) fn copy(&self) {
        self.copies.fetch_add(1, Relaxed);
    }
    /// A render target set on the list (the first set stays, until the list is reset).
    pub(crate) fn target(&self, width: u64, height: u32) {
        let _ = self.target.compare_exchange(0, pack_target(width, height), Relaxed, Relaxed);
    }
    pub(crate) fn clear(&self) {
        for counter in [&self.draws, &self.dispatches, &self.copies, &self.target] {
            counter.store(0, Relaxed);
        }
    }
}

/// A render target size in one word.
pub(crate) fn pack_target(width: u64, height: u32) -> u32 {
    ((width.min(0xFFFF) as u32) << 16) | height.min(0xFFFF)
}

/// A packed render target size as text.
pub(crate) fn target_text(packed: u32) -> String {
    if packed == 0 { "none".into() } else { format!("{}x{}", packed >> 16, packed & 0xFFFF) }
}

/// What one of the call's lists is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ListMeta {
    /// 0 a prelude of ours (state transitions, uploads), 1 a list of the title, 2 a card check.
    pub(crate) kind: u8,
    /// The title list's place in its submission and the submission's lists.
    pub(crate) pos: u16,
    pub(crate) count: u16,
    /// The submission's number.
    pub(crate) sub: u32,
    pub(crate) draws: u32,
    pub(crate) dispatches: u32,
    pub(crate) copies: u32,
    pub(crate) target: u32,
}

impl ListMeta {
    pub(crate) fn prelude(pos: usize, count: usize, sub: u64) -> ListMeta {
        ListMeta { kind: 0, pos: pos.min(0xFFFF) as u16, count: count.min(0xFFFF) as u16, sub: sub as u32, ..ListMeta::default() }
    }
    pub(crate) fn check(count: usize, sub: u64) -> ListMeta {
        ListMeta { kind: 2, count: count.min(0xFFFF) as u16, sub: sub as u32, ..ListMeta::default() }
    }
    pub(crate) fn title(pos: usize, count: usize, sub: u64, stats: &ListStats) -> ListMeta {
        ListMeta {
            kind: 1,
            draws: stats.draws.load(Relaxed),
            dispatches: stats.dispatches.load(Relaxed),
            copies: stats.copies.load(Relaxed),
            target: stats.target.load(Relaxed),
            ..ListMeta::prelude(pos, count, sub)
        }
    }
    /// The histogram bucket: 0 draws, 1 dispatch-only, 2 copy-only, 3 nothing counted, 4 preludes, 5 checks.
    pub(crate) fn bucket(&self) -> usize {
        match self.kind {
            0 => 4,
            2 => 5,
            _ if self.draws > 0 => 0,
            _ if self.dispatches > 0 => 1,
            _ if self.copies > 0 => 2,
            _ => 3,
        }
    }
}

const BUCKETS: [&str; 6] = ["draws", "dispatch_only", "copy_only", "uncounted", "prelude", "check"];

/// What lists are summed by.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct Key {
    kind: u8,
    pos: u16,
    count: u16,
    target: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Agg {
    ticks: u64,
    lists: u64,
    draws: u64,
    dispatches: u64,
    copies: u64,
    sub: u32,
}

/// One list of `meta` that took `ticks`, into the sums.
pub(crate) fn add_list(sums: &mut HashMap<Key, Agg>, buckets: &mut [u64; 6], meta: &ListMeta, ticks: u64) {
    let agg = sums.entry(Key { kind: meta.kind, pos: meta.pos, count: meta.count, target: meta.target }).or_default();
    agg.ticks += ticks;
    agg.lists += 1;
    agg.draws += u64::from(meta.draws);
    agg.dispatches += u64::from(meta.dispatches);
    agg.copies += u64::from(meta.copies);
    agg.sub = meta.sub;
    buckets[meta.bucket()] += ticks;
}

/// The `n` sums with most ticks (ties: by key).
pub(crate) fn top(sums: &HashMap<Key, Agg>, n: usize) -> Vec<(Key, Agg)> {
    let mut all: Vec<(Key, Agg)> = sums.iter().map(|(k, a)| (*k, *a)).collect();
    all.sort_by(|a, b| b.1.ticks.cmp(&a.1.ticks).then(a.0.cmp(&b.0)));
    all.truncate(n);
    all
}

/// Where a submission of `need` stamps goes in a ring of `cap`, whose stamps from `tail` on are
/// still to be read and the next free one is `head` (all counting up): a run never wraps (it
/// starts at the ring's start instead, the rest of the ring left), and it must not reach what is
/// not read. `(start, end)` in the counting.
pub(crate) fn alloc_range(head: u64, tail: u64, cap: u64, need: u64) -> Option<(u64, u64)> {
    if need == 0 || need > cap {
        return None;
    }
    let start = if head % cap + need > cap { (head / cap + 1) * cap } else { head };
    let end = start + need;
    (end - tail <= cap).then_some((start, end))
}

/// A counter that went up from `last` to `now`, or was cleared on the way (then `now`).
pub(crate) fn delta(now: u64, last: u64) -> u64 {
    if now >= last { now - last } else { now }
}

/// Card ticks to microseconds.
pub(crate) fn ticks_to_us(ticks: u64, frequency: u64) -> u64 {
    if frequency == 0 {
        return 0;
    }
    (u128::from(ticks) * 1_000_000 / u128::from(frequency)) as u64
}

/// One reading of both clocks at once: the card's ticks and the system's counter.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub(crate) struct Calibration {
    pub(crate) gpu: u64,
    pub(crate) cpu: u64,
}

/// A system counter value in the card's ticks, by a reading of both clocks.
pub(crate) fn counter_to_card(counter: u64, cal: Calibration, counter_frequency: u64, card_frequency: u64) -> i128 {
    let delta = i128::from(counter) - i128::from(cal.cpu);
    i128::from(cal.gpu) + delta * i128::from(card_frequency) / i128::from(counter_frequency.max(1))
}

/// Per-frame sums in card ticks, by kind of submission (the title's, ours).
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub(crate) struct Tally {
    pub(crate) exec: [u64; 2],
    pub(crate) latency: u64,
    pub(crate) idle: u64,
    pub(crate) spans: u64,
    last_end: Option<u64>,
}

impl Tally {
    /// A submission whose stamps read `start` and `end`, sent when the card's clock read `sent`.
    pub(crate) fn add(&mut self, kind: usize, start: u64, end: u64, sent: i128) {
        self.exec[kind & 1] += end.saturating_sub(start);
        self.latency += u64::try_from((i128::from(start) - sent).max(0)).unwrap_or(0);
        if let Some(previous) = self.last_end {
            self.idle += start.saturating_sub(previous);
        }
        self.last_end = Some(end);
        self.spans += 1;
    }

    /// The sums so far, with the sums cleared (the last end is kept, for the next gap).
    pub(crate) fn take(&mut self) -> Tally {
        let taken = *self;
        *self = Tally { last_end: self.last_end, ..Tally::default() };
        taken
    }
}

struct Pair {
    allocator: ID3D12CommandAllocator,
    first: ID3D12GraphicsCommandList,
    second: ID3D12GraphicsCommandList,
}

struct StampList {
    _allocator: ID3D12CommandAllocator,
    list: ID3D12CommandList,
}

/// A submission's stamps of the per-list clock.
struct Listed {
    first: usize,
    end: u64,
    metas: Vec<ListMeta>,
}

/// The per-list clock's ring and sums.
struct PerList {
    heap: ID3D12QueryHeap,
    readback: ID3D12Resource,
    mapped: usize,
    /// Stamps allocated and read so far, counting up (`QUERIES` a lap).
    head: u64,
    tail: u64,
    /// Per query: the list that stamps it (made once, run again and again).
    stamps: Vec<Option<StampList>>,
    /// Per submission slot: the allocator and list that resolve its stamps.
    resolvers: Vec<Option<(ID3D12CommandAllocator, ID3D12GraphicsCommandList)>>,
    listed: Vec<Option<Listed>>,
    sums: HashMap<Key, Agg>,
    buckets: [u64; 6],
    /// Submissions without per-list stamps (ring full, lists not made).
    skipped: u64,
    /// Frames that had lists stamped, and the last of them.
    sampled: u64,
    last_frame: Option<u64>,
}

struct Ring {
    /// Submissions stamped so far; the next one has this number.
    head: u64,
    /// Submissions read so far.
    tail: u64,
    /// Per submission, by slot: the system counter when sent, and its kind.
    sent: Vec<(u64, usize)>,
    pairs: Vec<Option<Pair>>,
    tally: Tally,
    /// Submissions sent without stamps (ring full or lists not made).
    dropped: u64,
    calibration: Calibration,
    per_list: Option<PerList>,
}

/// The host queue's stamps.
pub(crate) struct Clock {
    heap: ID3D12QueryHeap,
    readback: ID3D12Resource,
    /// The read-back buffer, mapped for good (an address, so the clock is `Send`).
    mapped: usize,
    fence: ID3D12Fence,
    device: ID3D12Device,
    card_frequency: u64,
    counter_frequency: u64,
    ring: Mutex<Ring>,
}

fn counter() -> u64 {
    let mut count = 0;
    // SAFETY: a valid out pointer.
    unsafe { windows_sys::Win32::System::Performance::QueryPerformanceCounter(&mut count) };
    u64::try_from(count).unwrap_or(0)
}

fn counter_frequency() -> u64 {
    let mut frequency = 0;
    // SAFETY: a valid out pointer.
    unsafe { windows_sys::Win32::System::Performance::QueryPerformanceFrequency(&mut frequency) };
    u64::try_from(frequency).unwrap_or(1).max(1)
}

fn calibrate(queue: &ID3D12CommandQueue) -> Option<Calibration> {
    let (mut gpu, mut cpu) = (0u64, 0u64);
    // SAFETY: valid out pointers.
    unsafe { queue.GetClockCalibration(&mut gpu, &mut cpu) }.ok()?;
    Some(Calibration { gpu, cpu })
}

/// The host queue's clock, made at the first call; `None` with the switch off or if the card
/// would not give one.
pub(crate) fn host(host: &crate::host::Host) -> Option<&'static Clock> {
    if !on() {
        return None;
    }
    static CLOCK: OnceLock<Option<Clock>> = OnceLock::new();
    CLOCK
        .get_or_init(|| {
            let made = Clock::new(host);
            match &made {
                Some(clock) => tracing::info!(
                    "M1LK Churn Clock: stamping the host queue (card clock {} Hz, {} submissions in flight at most)",
                    clock.card_frequency,
                    SLOTS
                ),
                None => tracing::warn!("M1LK Churn Clock: no stamps for the host queue (the card gave no query heap or clock)"),
            }
            made
        })
        .as_ref()
}

impl PerList {
    fn new(host: &crate::host::Host) -> Option<PerList> {
        let mut heap: Option<ID3D12QueryHeap> = None;
        // SAFETY: the description and output belong to this call.
        unsafe {
            host.device
                .CreateQueryHeap(
                    &D3D12_QUERY_HEAP_DESC { Type: D3D12_QUERY_HEAP_TYPE_TIMESTAMP, Count: QUERIES as u32, NodeMask: 0 },
                    &mut heap,
                )
                .ok()?;
        }
        let readback = host.read_back_buffer((8 * QUERIES) as u64)?;
        let mut data = std::ptr::null_mut();
        // SAFETY: mapped for reading for good, as the clock's own buffer.
        unsafe { readback.Map(0, None, Some(&mut data)) }.ok()?;
        if data.is_null() {
            return None;
        }
        Some(PerList {
            heap: heap?,
            readback,
            mapped: data as usize,
            head: 0,
            tail: 0,
            stamps: (0..QUERIES).map(|_| None).collect(),
            resolvers: (0..SLOTS).map(|_| None).collect(),
            listed: (0..SLOTS).map(|_| None).collect(),
            sums: HashMap::new(),
            buckets: [0; 6],
            skipped: 0,
            sampled: 0,
            last_frame: None,
        })
    }
}

impl Clock {
    fn new(host: &crate::host::Host) -> Option<Clock> {
        let mut heap: Option<ID3D12QueryHeap> = None;
        // SAFETY: the description and outputs belong to this call.
        let card_frequency = unsafe {
            let frequency = host.queue.GetTimestampFrequency().ok()?;
            host.device
                .CreateQueryHeap(
                    &D3D12_QUERY_HEAP_DESC {
                        Type: D3D12_QUERY_HEAP_TYPE_TIMESTAMP,
                        Count: (2 * SLOTS) as u32,
                        NodeMask: 0,
                    },
                    &mut heap,
                )
                .ok()?;
            frequency
        };
        let readback = host.read_back_buffer((16 * SLOTS) as u64)?;
        let mut data = std::ptr::null_mut();
        // SAFETY: a read-back buffer is mapped for reading, and stays mapped as long as the
        // clock (which is never dropped).
        unsafe { readback.Map(0, None, Some(&mut data)) }.ok()?;
        if data.is_null() {
            return None;
        }
        // SAFETY: a plain object creation.
        let fence: ID3D12Fence = unsafe { host.device.CreateFence(0, D3D12_FENCE_FLAG_NONE) }.ok()?;
        let calibration = calibrate(&host.queue)?;
        let per_list = if lists() { PerList::new(host) } else { None };
        Some(Clock {
            heap: heap?,
            readback,
            mapped: data as usize,
            fence,
            device: host.device.clone(),
            card_frequency,
            counter_frequency: counter_frequency(),
            ring: Mutex::new(Ring {
                head: 0,
                tail: 0,
                sent: vec![(0, 0); SLOTS],
                pairs: (0..SLOTS).map(|_| None).collect(),
                tally: Tally::default(),
                dropped: 0,
                calibration,
                per_list,
            }),
        })
    }

    /// Makes or resets the two small lists of `slot` and records the stamps into them.
    fn record(&self, ring: &mut Ring, slot: usize) -> Option<(ID3D12CommandList, ID3D12CommandList)> {
        let first_stamp = (2 * slot) as u32;
        // SAFETY: lists and allocator of this slot, reset only after the card finished the
        // slot's last use (the ring reuses a slot once it has been read).
        unsafe {
            if ring.pairs[slot].is_none() {
                let allocator: ID3D12CommandAllocator =
                    self.device.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT).ok()?;
                let first: ID3D12GraphicsCommandList =
                    self.device.CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &allocator, None).ok()?;
                first.Close().ok()?;
                let second: ID3D12GraphicsCommandList =
                    self.device.CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &allocator, None).ok()?;
                second.Close().ok()?;
                ring.pairs[slot] = Some(Pair { allocator, first, second });
            }
            let pair = ring.pairs[slot].as_ref()?;
            pair.allocator.Reset().ok()?;
            pair.first.Reset(&pair.allocator, None).ok()?;
            pair.first.EndQuery(&self.heap, D3D12_QUERY_TYPE_TIMESTAMP, first_stamp);
            pair.first.Close().ok()?;
            pair.second.Reset(&pair.allocator, None).ok()?;
            pair.second.EndQuery(&self.heap, D3D12_QUERY_TYPE_TIMESTAMP, first_stamp + 1);
            pair.second.ResolveQueryData(
                &self.heap,
                D3D12_QUERY_TYPE_TIMESTAMP,
                first_stamp,
                2,
                &self.readback,
                (16 * slot) as u64,
            );
            pair.second.Close().ok()?;
            Some((pair.first.cast().ok()?, pair.second.cast().ok()?))
        }
    }

    /// Reads the stamps of submissions the card has finished.
    fn drain(&self, ring: &mut Ring) {
        // SAFETY: a plain query.
        let done = unsafe { self.fence.GetCompletedValue() };
        if done == u64::MAX {
            return;
        }
        while ring.tail < ring.head && ring.tail < done {
            let slot = (ring.tail % SLOTS as u64) as usize;
            let stamps = self.mapped as *const u64;
            // SAFETY: the slot's 16 bytes were resolved before the fence passed it, and the
            // buffer stays mapped.
            let (start, end) = unsafe { (stamps.add(2 * slot).read_volatile(), stamps.add(2 * slot + 1).read_volatile()) };
            let (counted, kind) = ring.sent[slot];
            let sent = counter_to_card(counted, ring.calibration, self.counter_frequency, self.card_frequency);
            ring.tally.add(kind, start, end, sent);
            if let Some(per) = ring.per_list.as_mut()
                && let Some(listed) = per.listed[slot].take()
            {
                let stamps = per.mapped as *const u64;
                for (k, meta) in listed.metas.iter().enumerate() {
                    // SAFETY: resolved before the fence passed the slot; the buffer stays mapped.
                    let (a, b) = unsafe {
                        (stamps.add(listed.first + k).read_volatile(), stamps.add(listed.first + k + 1).read_volatile())
                    };
                    let PerList { sums, buckets, .. } = &mut *per;
                    add_list(sums, buckets, meta, b.saturating_sub(a));
                }
                per.tail = listed.end;
            }
            ring.tail += 1;
        }
    }

    /// The stamps of the per-list clock for `lists` (`meta`, in step with them) of `slot`: the
    /// list for each stamp and the one that resolves them, with the stamps' place in the ring.
    fn per_list_stamps(
        &self,
        ring: &mut Ring,
        slot: usize,
        lists: &[Option<ID3D12CommandList>],
        meta: &[ListMeta],
    ) -> Option<(Vec<ID3D12CommandList>, ID3D12CommandList, usize)> {
        let per = ring.per_list.as_mut()?;
        let frame = crate::frame::current();
        if !sampled_frame(frame, every()) || meta.len() != lists.len() || lists.iter().any(Option::is_none) {
            return None;
        }
        // At most `most()` lists are stamped; the rest of the call runs after, unstamped.
        let lists = &lists[..lists.len().min(most())];
        let meta = &meta[..lists.len()];
        let need = lists.len() as u64 + 1;
        let Some((start, end)) = alloc_range(per.head, per.tail, QUERIES as u64, need) else {
            per.skipped += 1;
            return None;
        };
        let first = (start % QUERIES as u64) as usize;
        let mut stamps = Vec::with_capacity(need as usize);
        // SAFETY: lists and allocators made here; a stamp list is closed for good and may run
        // again while an earlier run is still queued; the resolver's allocator is reset only
        // after the slot was read (the ring reuses a slot once it has been).
        unsafe {
            for query in first..first + need as usize {
                if per.stamps[query].is_none() {
                    let allocator: ID3D12CommandAllocator =
                        self.device.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT).ok()?;
                    let list: ID3D12GraphicsCommandList =
                        self.device.CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &allocator, None).ok()?;
                    list.EndQuery(&per.heap, D3D12_QUERY_TYPE_TIMESTAMP, query as u32);
                    list.Close().ok()?;
                    per.stamps[query] = Some(StampList { _allocator: allocator, list: list.cast().ok()? });
                }
                stamps.push(per.stamps[query].as_ref()?.list.clone());
            }
            if per.resolvers[slot].is_none() {
                let allocator: ID3D12CommandAllocator =
                    self.device.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT).ok()?;
                let list: ID3D12GraphicsCommandList =
                    self.device.CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &allocator, None).ok()?;
                list.Close().ok()?;
                per.resolvers[slot] = Some((allocator, list));
            }
            let (allocator, list) = per.resolvers[slot].as_ref()?;
            allocator.Reset().ok()?;
            list.Reset(allocator, None).ok()?;
            list.ResolveQueryData(
                &per.heap,
                D3D12_QUERY_TYPE_TIMESTAMP,
                first as u32,
                need as u32,
                &per.readback,
                (8 * first) as u64,
            );
            list.Close().ok()?;
            let resolve: ID3D12CommandList = list.cast().ok()?;
            per.head = end;
            if per.last_frame != Some(frame) {
                per.last_frame = Some(frame);
                per.sampled += 1;
            }
            per.listed[slot] = Some(Listed { first, end, metas: meta.to_vec() });
            Some((stamps, resolve, lists.len()))
        }
    }

    /// `ExecuteCommandLists(lists)` on `queue` with a stamp before and one after (and, with
    /// `meta` for every list and `VICTORY_CHURN_LISTS=1`, one between every two lists).
    pub(crate) fn execute(&self, queue: &ID3D12CommandQueue, lists: &[Option<ID3D12CommandList>], title: bool, meta: &[ListMeta]) {
        let mut ring = self.ring.lock().unwrap_or_else(|e| e.into_inner());
        self.drain(&mut ring);
        let slot = (ring.head % SLOTS as u64) as usize;
        let stamps = if lists.is_empty() || ring.head - ring.tail >= SLOTS as u64 {
            None
        } else {
            self.record(&mut ring, slot)
        };
        let Some((first, second)) = stamps else {
            ring.dropped += 1;
            drop(ring);
            // SAFETY: as the caller of `execute_lists` guarantees.
            unsafe { queue.ExecuteCommandLists(lists) };
            return;
        };
        let between = if title { self.per_list_stamps(&mut ring, slot, lists, meta) } else { None };
        let mut all = Vec::with_capacity(2 * lists.len() + 4);
        all.push(Some(first));
        match between {
            Some((stamps, resolve, stamped)) => {
                for (stamp, list) in stamps.iter().zip(&lists[..stamped]) {
                    all.push(Some(stamp.clone()));
                    all.push(list.clone());
                }
                all.push(stamps.last().cloned());
                all.push(Some(resolve));
                all.extend_from_slice(&lists[stamped..]);
            }
            None => all.extend_from_slice(lists),
        }
        all.push(Some(second));
        ring.sent[slot] = (counter(), usize::from(!title));
        // SAFETY: the caller's lists, closed, with our own; the fence value orders the read.
        unsafe {
            queue.ExecuteCommandLists(&all);
            let _ = queue.Signal(&self.fence, ring.head + 1);
        }
        ring.head += 1;
    }

    /// The per-list sums since the last call, with the submissions the ring had no stamps for.
    fn take_lists(&self) -> Option<(HashMap<Key, Agg>, [u64; 6], u64, u64)> {
        let mut ring = self.ring.lock().unwrap_or_else(|e| e.into_inner());
        self.drain(&mut ring);
        let per = ring.per_list.as_mut()?;
        Some((std::mem::take(&mut per.sums), std::mem::take(&mut per.buckets), std::mem::take(&mut per.skipped), std::mem::take(&mut per.sampled)))
    }

    /// The sums since the last call, and how many submissions went unstamped.
    fn take(&self, queue: &ID3D12CommandQueue) -> (Tally, u64) {
        let mut ring = self.ring.lock().unwrap_or_else(|e| e.into_inner());
        self.drain(&mut ring);
        let taken = (ring.tally.take(), std::mem::take(&mut ring.dropped));
        // The two clocks drift apart slowly: read both again for the next frame.
        if let Some(calibration) = calibrate(queue) {
            ring.calibration = calibration;
        }
        taken
    }
}

/// Wall time and count of the texture worker's copy batches, from send to fence (µs, batches).
static AHEAD: [AtomicU64; 2] = [const { AtomicU64::new(0) }; 2];

/// A copy batch of the texture worker took `micros` from send to its fence.
pub(crate) fn ahead(micros: u64) {
    AHEAD[0].fetch_add(micros, Relaxed);
    AHEAD[1].fetch_add(1, Relaxed);
}

/// The frame line: every sum in ms a frame, over `frames` frames.
pub(crate) fn line(host: &crate::host::Host, frames: f64) {
    if frames <= 0.0 {
        return;
    }
    let Some(clock) = self::host(host) else {
        return;
    };
    let (tally, dropped) = clock.take(&host.queue);
    let ms = |ticks: u64| ticks_to_us(ticks, clock.card_frequency) as f64 / 1000.0 / frames;
    let ahead_ms = AHEAD[0].swap(0, Relaxed) as f64 / 1000.0 / frames;
    let ahead_n = AHEAD[1].swap(0, Relaxed) as f64 / frames;
    // The card's other clock (a fence behind every send): the card with nothing of ours queued,
    // and how much of that the submitting thread was working (a gap of ours) or the title had not
    // submitted (the title's wait). Read from the frame line's own sums, which it clears.
    static LAST_GAP: [AtomicU64; 2] = [const { AtomicU64::new(0) }; 2];
    let [idle_now, ours_now] = crate::host::card_idle_us();
    let idle = delta(idle_now, LAST_GAP[0].swap(idle_now, Relaxed));
    let ours = delta(ours_now, LAST_GAP[1].swap(ours_now, Relaxed));
    let (gap_ours, gap_title) = (ours as f64 / 1000.0 / frames, idle.saturating_sub(ours) as f64 / 1000.0 / frames);
    tracing::info!(
        "M1LK Churn Clock ms/frame: host_exec={:.2} host_exec_title={:.2} host_exec_other={:.2} host_queue_lat={:.2} \
         host_gpu_idle={:.2} host_spans={:.1} host_unstamped={:.1} ahead_copy_wall={:.2} ahead_batches={:.1} host_gap_title_wait={:.2} host_gap_ours={:.2}",
        ms(tally.exec[0] + tally.exec[1]),
        ms(tally.exec[0]),
        ms(tally.exec[1]),
        ms(tally.latency),
        ms(tally.idle),
        tally.spans as f64 / frames,
        dropped as f64 / frames,
        ahead_ms,
        ahead_n,
        gap_title,
        gap_ours,
    );
    if let Some((sums, buckets, skipped, sampled)) = clock.take_lists() {
        // Per sampled frame (the stamps of the other frames are not made).
        let frames = sampled.max(1) as f64;
        let ms = |ticks: u64| ticks_to_us(ticks, clock.card_frequency) as f64 / 1000.0 / frames;
        let mut text = String::new();
        for (i, (key, agg)) in top(&sums, 8).iter().enumerate() {
            let lists = agg.lists.max(1) as f64;
            let what = match key.kind {
                0 => "prelude",
                2 => "check",
                _ => "list",
            };
            text += &format!(
                "{} #{} {what} pos={}/{} sub={} rt={} gpu={:.2}ms/f n={:.1}/f draws={:.0} disp={:.0} copies={:.0}",
                if i == 0 { "" } else { " |" },
                i + 1,
                key.pos,
                key.count,
                agg.sub,
                target_text(key.target),
                ms(agg.ticks),
                agg.lists as f64 / frames,
                agg.draws as f64 / lists,
                agg.dispatches as f64 / lists,
                agg.copies as f64 / lists,
            );
        }
        tracing::info!("M1LK Churn Lists top (sampled_frames={sampled} every={} max={}):{text}", every(), most());
        let total: u64 = buckets.iter().sum();
        let hist: Vec<String> = BUCKETS.iter().zip(buckets).map(|(name, ticks)| format!("{name}={:.2}", ms(ticks))).collect();
        tracing::info!(
            "M1LK Churn Lists hist ms/sampled_frame (sampled_frames={}): {} total={:.2} keys={} unlisted_submissions={:.1}",
            sampled,
            hist.join(" "),
            ms(total),
            sums.len(),
            skipped as f64 / frames,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticks_become_microseconds() {
        assert_eq!(ticks_to_us(10_000_000, 10_000_000), 1_000_000);
        assert_eq!(ticks_to_us(25, 1_000_000), 25);
        assert_eq!(ticks_to_us(5, 0), 0);
        assert_eq!(ticks_to_us(u64::MAX / 2, 1_000_000_000), (u64::MAX / 2) / 1000);
    }

    #[test]
    fn counter_values_land_in_card_ticks() {
        let cal = Calibration { gpu: 1_000, cpu: 500 };
        // Counter 10 MHz, card 1 GHz: 100 card ticks to a counter tick.
        assert_eq!(counter_to_card(500, cal, 10_000_000, 1_000_000_000), 1_000);
        assert_eq!(counter_to_card(510, cal, 10_000_000, 1_000_000_000), 2_000);
        assert_eq!(counter_to_card(490, cal, 10_000_000, 1_000_000_000), 0);
    }

    #[test]
    fn spans_sum_execution_latency_and_gaps() {
        let mut tally = Tally::default();
        tally.add(1, 100, 300, 0);
        tally.add(1, 350, 400, 150);
        tally.add(0, 400, 460, 390);
        tally.add(0, 470, 480, 500);
        assert_eq!(tally.exec, [70, 250]);
        assert_eq!(tally.latency, 100 + 200 + 10);
        assert_eq!(tally.idle, 50 + 10);
        assert_eq!(tally.spans, 4);
    }

    #[test]
    fn ranges_never_wrap_and_never_reach_unread_stamps() {
        assert_eq!(alloc_range(0, 0, 100, 10), Some((0, 10)));
        assert_eq!(alloc_range(95, 90, 100, 10), Some((100, 110)), "starts at the ring's start");
        assert_eq!(alloc_range(95, 0, 100, 10), None, "the unread would be reached");
        assert_eq!(alloc_range(100, 100, 100, 100), Some((100, 200)));
        assert_eq!(alloc_range(0, 0, 100, 101), None);
        assert_eq!(alloc_range(0, 0, 100, 0), None);
        assert_eq!(alloc_range(150, 60, 100, 10), Some((150, 160)));
        assert_eq!(alloc_range(150, 40, 100, 10), None);
    }

    #[test]
    fn lists_are_summed_by_place_and_bucketed() {
        let stats = ListStats::default();
        stats.draw();
        stats.draw();
        stats.target(1920, 1080);
        stats.target(64, 64);
        let drawing = ListMeta::title(3, 10, 7, &stats);
        assert_eq!((drawing.draws, drawing.target, drawing.bucket()), (2, pack_target(1920, 1080), 0));
        assert_eq!(target_text(drawing.target), "1920x1080");
        assert_eq!(target_text(0), "none");
        stats.clear();
        stats.dispatch();
        let compute = ListMeta::title(4, 10, 7, &stats);
        assert_eq!(compute.bucket(), 1);
        stats.clear();
        stats.copy();
        assert_eq!(ListMeta::title(5, 10, 7, &stats).bucket(), 2);
        stats.clear();
        assert_eq!(ListMeta::title(6, 10, 7, &stats).bucket(), 3);
        assert_eq!((ListMeta::prelude(0, 10, 7).bucket(), ListMeta::check(10, 7).bucket()), (4, 5));
        let (mut sums, mut buckets) = (HashMap::new(), [0u64; 6]);
        add_list(&mut sums, &mut buckets, &drawing, 30);
        add_list(&mut sums, &mut buckets, &drawing, 20);
        add_list(&mut sums, &mut buckets, &compute, 70);
        assert_eq!(buckets, [50, 70, 0, 0, 0, 0]);
        let best = top(&sums, 1);
        assert_eq!((best[0].1.ticks, best[0].1.lists), (70, 1));
        let both = top(&sums, 8);
        assert_eq!((both.len(), both[1].1.ticks, both[1].1.draws), (2, 50, 4));
    }

    #[test]
    fn one_frame_in_n_is_sampled() {
        assert!(sampled_frame(5, 0) && sampled_frame(5, 1));
        assert!(sampled_frame(60, 30) && !sampled_frame(61, 30));
    }

    #[test]
    fn a_cleared_counter_counts_from_zero() {
        assert_eq!(delta(15, 10), 5);
        assert_eq!(delta(3, 10), 3);
    }

    #[test]
    fn taking_clears_the_sums_but_keeps_the_gap_start() {
        let mut tally = Tally::default();
        tally.add(1, 10, 20, 0);
        let taken = tally.take();
        assert_eq!((taken.spans, taken.exec), (1, [0, 10]));
        tally.add(1, 25, 30, 20);
        assert_eq!((tally.spans, tally.idle, tally.exec), (1, 5, [0, 5]));
    }
}
