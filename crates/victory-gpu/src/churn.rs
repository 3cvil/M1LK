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
//! It does not use `VICTORY_GPU_TIMING` (that serialises lists). The other queues of a run are
//! the copy queue of the texture worker (a copy queue cannot resolve stamps, so only its wall
//! time from send to fence is counted) and the rigs of the tests.

use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{Mutex, OnceLock};

use windows::Win32::Graphics::Direct3D12::*;
use windows::core::Interface;

/// Submissions the ring holds stamps for at once (two stamps each).
const SLOTS: usize = 1024;

/// `VICTORY_CHURN_CLOCK=1` with the submit profiler on.
pub(crate) fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| crate::prof::on() && std::env::var("VICTORY_CHURN_CLOCK").is_ok_and(|v| v.trim() == "1"))
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
            ring.tail += 1;
        }
    }

    /// `ExecuteCommandLists(lists)` on `queue` with a stamp before and one after.
    pub(crate) fn execute(&self, queue: &ID3D12CommandQueue, lists: &[Option<ID3D12CommandList>], title: bool) {
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
        let mut all = Vec::with_capacity(lists.len() + 2);
        all.push(Some(first));
        all.extend_from_slice(lists);
        all.push(Some(second));
        ring.sent[slot] = (counter(), usize::from(!title));
        // SAFETY: the caller's lists, closed, with our two; the fence value orders the read.
        unsafe {
            queue.ExecuteCommandLists(&all);
            let _ = queue.Signal(&self.fence, ring.head + 1);
        }
        ring.head += 1;
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
    tracing::info!(
        "M1LK Churn Clock ms/frame: host_exec={:.2} host_exec_title={:.2} host_exec_other={:.2} host_queue_lat={:.2} \
         host_gpu_idle={:.2} host_spans={:.1} host_unstamped={:.1} ahead_copy_wall={:.2} ahead_batches={:.1}",
        ms(tally.exec[0] + tally.exec[1]),
        ms(tally.exec[0]),
        ms(tally.exec[1]),
        ms(tally.latency),
        ms(tally.idle),
        tally.spans as f64 / frames,
        dropped as f64 / frames,
        ahead_ms,
        ahead_n,
    );
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
    fn taking_clears_the_sums_but_keeps_the_gap_start() {
        let mut tally = Tally::default();
        tally.add(1, 10, 20, 0);
        let taken = tally.take();
        assert_eq!((taken.spans, taken.exec), (1, [0, 10]));
        tally.add(1, 25, 30, 20);
        assert_eq!((tally.spans, tally.idle, tally.exec), (1, 5, [0, 5]));
    }
}
