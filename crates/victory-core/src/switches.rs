//! Runtime switches: named on/off settings read with one atomic load, and a
//! timed schedule that flips some of them while the title runs.
//!
//! Each switch starts with the value its environment variable gives
//! (`M1LK_<NAME>`, or the older `VICTORY_<NAME>`; `M1LK_` wins), exactly as the
//! one-time reads before it did. A switch is either
//!
//! * RUNTIME-SAFE: flipping it between two frames cannot corrupt state, or
//! * START-ONLY: it changes a memory layout, an allocation made once, a
//!   cached translation or a start-up behaviour; only its start value counts.
//!
//! | switch | kind | why |
//! |---|---|---|
//! | `COPY_PLAIN` | runtime | chosen per host copy; the fault handler is installed the first time it is on |
//! | `SHORT_REMAP_LOCK` | runtime | chosen per texture read |
//! | `WIDE_REFRESH` | runtime | chosen per flush; a flush finishes its own jobs |
//! | `PARALLEL_REFRESH` | runtime | chosen per texture step; the batch runs whatever was gathered |
//! | `INDIRECT_WINDOW` | runtime | recording state only; a window ends when the switch is off (default on) |
//! | `LOOKUP_CACHE` | runtime | the tables' change count is bumped at a flip, so no thread keeps an old answer |
//! | `RANGE_MEMO` | runtime | the memo is emptied at a flip (and already dropped each frame) |
//! | `LOW_SHADOWS` | runtime | decided per draw; a skipped slice is only left cleared |
//! | `WATCH_SCAN` | start-only | changes how write-watch askings are laid out and journalled for the whole run |
//! | `LOWCOMMIT_BACKOFF` | start-only | commit behaviour of uploads and staging is set up at start |
//! | `INPLACE_CONSTANTS` | start-only | regions and buffer forms are placed once; a title buffer cannot change form |
//! | `GPU_DETILE` | start-only | textures kept detiled on the card or the PC stay so; the shader is made at start |
//! | `LEGACY_FMA` | start-only | changes translated code (cached shaders); read by the translator once, part of the shader cache key |
//! | `NAN_GUARD` | start-only | changes translated code (cached shaders); read by the translator once, part of the shader cache key |
//!
//! The schedule (`M1LK_TOGGLE_PLAN`, names joined by `,` or `+` (the test queue splits its env list on commas); see [`parse_plan`] and [`schedule`])
//! names runtime-safe switches only. [`on_frame`] is called once a frame by
//! the present path, so a flip is always between two frames.

use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering::Relaxed};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// One named switch.
pub struct Switch {
    pub name: &'static str,
    /// Whether it may be flipped while the title runs.
    pub runtime_safe: bool,
    /// The start value, from the text of its variable (`None`: not set).
    parse: fn(Option<&str>) -> bool,
    /// 0 not read yet, 1 off, 2 on.
    state: AtomicU8,
    /// The start value: 0 not read yet, 1 off, 2 on.
    base: AtomicU8,
}

impl Switch {
    const fn new(name: &'static str, runtime_safe: bool, parse: fn(Option<&str>) -> bool) -> Self {
        Self { name, runtime_safe, parse, state: AtomicU8::new(0), base: AtomicU8::new(0) }
    }

    /// Whether it is on now. One atomic load once it has been read.
    #[inline]
    pub fn is_on(&self) -> bool {
        match self.state.load(Relaxed) {
            2 => true,
            1 => false,
            _ => self.init(),
        }
    }

    #[cold]
    fn init(&self) -> bool {
        let text = env_text(self.name);
        let value = if (self.parse)(text.as_deref()) { 2 } else { 1 };
        // The first reader's value stands (every reader computes the same).
        let _ = self.base.compare_exchange(0, value, Relaxed, Relaxed);
        let _ = self.state.compare_exchange(0, value, Relaxed, Relaxed);
        self.state.load(Relaxed) == 2
    }

    /// The value it started with.
    pub fn base(&self) -> bool {
        self.is_on();
        self.base.load(Relaxed) == 2
    }

    /// Sets it. Only the schedule (and tests) call this; returns whether the
    /// value changed.
    pub fn set(&self, on: bool) -> bool {
        let before = self.is_on();
        self.state.store(if on { 2 } else { 1 }, Relaxed);
        before != on
    }
}

/// The text of `M1LK_<name>` or `VICTORY_<name>`.
fn env_text(name: &str) -> Option<String> {
    std::env::var(format!("M1LK_{name}")).or_else(|_| std::env::var(format!("VICTORY_{name}"))).ok()
}

fn is_one(v: Option<&str>) -> bool {
    v == Some("1")
}

/// On unless the text says `0`.
fn not_zero(v: Option<&str>) -> bool {
    v != Some("0")
}

/// On for any text that is not an off word (the low-shadows value is a count).
fn not_off_word(v: Option<&str>) -> bool {
    v.is_some_and(|s| !matches!(s.trim().to_ascii_lowercase().as_str(), "" | "0" | "off" | "false" | "no"))
}

pub static COPY_PLAIN: Switch = Switch::new("COPY_PLAIN", true, is_one);
pub static SHORT_REMAP_LOCK: Switch = Switch::new("SHORT_REMAP_LOCK", true, is_one);
pub static WIDE_REFRESH: Switch = Switch::new("WIDE_REFRESH", true, is_one);
pub static PARALLEL_REFRESH: Switch = Switch::new("PARALLEL_REFRESH", true, is_one);
pub static INDIRECT_WINDOW: Switch = Switch::new("INDIRECT_WINDOW", true, not_zero);
pub static LOOKUP_CACHE: Switch = Switch::new("LOOKUP_CACHE", true, is_one);
pub static RANGE_MEMO: Switch = Switch::new("RANGE_MEMO", true, is_one);
pub static LOW_SHADOWS: Switch = Switch::new("LOW_SHADOWS", true, not_off_word);
// Start-only: listed so a plan naming them is refused with a reason. Their
// modules still read their own variables once.
pub static WATCH_SCAN: Switch = Switch::new("WATCH_SCAN", false, is_one);
pub static LOWCOMMIT_BACKOFF: Switch = Switch::new("LOWCOMMIT_BACKOFF", false, is_one);
pub static INPLACE_CONSTANTS: Switch = Switch::new("INPLACE_CONSTANTS", false, is_one);
pub static GPU_DETILE: Switch = Switch::new("GPU_DETILE", false, is_one);
// Read by victory-shader itself (`emit::PICTURE_SWITCHES`); listed here so a plan naming them is refused.
pub static LEGACY_FMA: Switch = Switch::new("LEGACY_FMA", false, is_one);
pub static NAN_GUARD: Switch = Switch::new("NAN_GUARD", false, is_one);

/// Every switch, runtime-safe and start-only.
pub static ALL: [&Switch; 14] = [
    &COPY_PLAIN,
    &SHORT_REMAP_LOCK,
    &WIDE_REFRESH,
    &PARALLEL_REFRESH,
    &INDIRECT_WINDOW,
    &LOOKUP_CACHE,
    &RANGE_MEMO,
    &LOW_SHADOWS,
    &WATCH_SCAN,
    &LOWCOMMIT_BACKOFF,
    &INPLACE_CONSTANTS,
    &GPU_DETILE,
    &LEGACY_FMA,
    &NAN_GUARD,
];

/// The switch of a name (with or without `M1LK_`/`VICTORY_`, any case).
pub fn find(name: &str) -> Option<&'static Switch> {
    let upper = name.trim().to_ascii_uppercase();
    let bare = upper.strip_prefix("M1LK_").or_else(|| upper.strip_prefix("VICTORY_")).unwrap_or(&upper);
    ALL.iter().copied().find(|s| s.name == bare)
}

/// The switches a plan text names, and those it names that are refused with
/// the reason.
pub fn parse_plan(text: &str) -> (Vec<&'static Switch>, Vec<(String, &'static str)>) {
    let mut keep: Vec<&'static Switch> = Vec::new();
    let mut refused = Vec::new();
    for word in text.split([',', '+', '|']).map(str::trim).filter(|w| !w.is_empty()) {
        match find(word) {
            None => refused.push((word.to_string(), "not a switch")),
            Some(s) if !s.runtime_safe => refused.push((word.to_string(), "start-only: it cannot change while the title runs")),
            Some(s) if keep.iter().any(|k| k.name == s.name) => refused.push((word.to_string(), "named twice")),
            Some(s) => keep.push(s),
        }
    }
    (keep, refused)
}

/// One stretch of the schedule: `switch` is `on` (or off) from `start_s` to
/// `end_s`, in seconds from the start of the schedule.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    pub switch: String,
    pub on: bool,
    pub start_s: u64,
    pub end_s: u64,
}

/// The schedule: in each pass every switch of the plan is on for `secs`, then
/// off for `secs`; the passes alternate, so the second runs off, then on (ABBA),
/// the third on, then off again, and so on.
pub fn schedule<S: AsRef<str>>(plan: &[S], secs: u64, repeats: u32) -> Vec<Step> {
    let mut steps = Vec::new();
    let mut at = 0;
    for pass in 0..repeats {
        for name in plan {
            let first_on = pass % 2 == 0;
            for on in [first_on, !first_on] {
                steps.push(Step { switch: name.as_ref().to_string(), on, start_s: at, end_s: at + secs });
                at += secs;
            }
        }
    }
    steps
}

/// The plan variable's text, if the schedule is in use.
fn plan_text() -> Option<String> {
    env_text("TOGGLE_PLAN").filter(|t| !t.trim().is_empty())
}

/// Whether a schedule was asked for (log lines then carry a `phase=` tag).
pub fn plan_active() -> bool {
    static ACTIVE: OnceLock<bool> = OnceLock::new();
    *ACTIVE.get_or_init(|| plan_text().is_some())
}

/// What the schedule is doing now: 0 nothing, else 1 + 2 * (index in [`ALL`]) + on.
static PHASE: AtomicUsize = AtomicUsize::new(0);

/// The tag for log lines: `<SWITCH>:<0|1>`, or `-` outside the plan.
pub fn phase_tag() -> String {
    match PHASE.load(Relaxed) {
        0 => "-".to_string(),
        p => format!("{}:{}", ALL[(p - 1) / 2].name, (p - 1) % 2),
    }
}

fn env_u64(name: &str, default: u64) -> u64 {
    env_text(name).and_then(|v| v.trim().parse().ok()).unwrap_or(default)
}

/// Draws a frame needs before it counts as the world (so loading is not measured).
const WORLD_DRAWS: u64 = 200;

struct Runner {
    steps: Vec<Step>,
    delay: Duration,
    last_draws: u64,
    /// When the schedule starts, once the world has been seen.
    t0: Option<Instant>,
    current: Option<usize>,
    done: bool,
}

impl Runner {
    fn new() -> Self {
        let text = plan_text().unwrap_or_default();
        let (switches, refused) = parse_plan(&text);
        for (word, why) in &refused {
            tracing::warn!("toggle plan: {word} left out ({why})");
        }
        let names: Vec<&str> = switches.iter().map(|s| s.name).collect();
        let secs = env_u64("TOGGLE_SECS", 60).max(1);
        let repeats = u32::try_from(env_u64("TOGGLE_REPEATS", 2)).unwrap_or(2);
        let delay = env_u64("TOGGLE_DELAY", 240);
        let steps = schedule(&names, secs, repeats);
        tracing::info!(
            plan = %names.join(","),
            secs,
            repeats,
            delay,
            steps = steps.len(),
            total_s = steps.last().map_or(0, |s| s.end_s),
            "toggle plan: starts {delay} s after the first frame with more than {WORLD_DRAWS} draws"
        );
        let done = steps.is_empty();
        Self { steps, delay: Duration::from_secs(delay), last_draws: 0, t0: None, current: None, done }
    }

    /// One frame. Returns the switches whose value changed.
    fn tick(&mut self, now: Instant, frame: u64, draws_total: u64) -> Vec<&'static str> {
        let mut changed = Vec::new();
        if self.done {
            return changed;
        }
        let drawn = draws_total.saturating_sub(self.last_draws);
        self.last_draws = draws_total;
        let Some(t0) = self.t0 else {
            if drawn > WORLD_DRAWS {
                self.t0 = Some(now + self.delay);
                tracing::info!(frame, "toggle plan armed: the schedule starts in {} s", self.delay.as_secs());
            }
            return changed;
        };
        if now < t0 {
            return changed;
        }
        let elapsed = now.duration_since(t0).as_secs();
        let index = self.steps.iter().position(|s| elapsed >= s.start_s && elapsed < s.end_s);
        if index == self.current && index.is_some() {
            return changed;
        }
        // The switch of the step that ended goes back to its start value
        // unless the next step is of the same switch.
        let previous = self.current.map(|i| self.steps[i].switch.clone());
        let next = index.map(|i| self.steps[i].clone());
        if let Some(prev) = previous
            && next.as_ref().is_none_or(|n| n.switch != prev)
            && let Some(s) = find(&prev)
        {
            if s.set(s.base()) {
                changed.push(s.name);
            }
            tracing::info!("toggle {} -> {} at frame {frame} (back to its start value)", s.name, if s.base() { "on" } else { "off" });
        }
        match next {
            Some(step) => {
                if let Some(s) = find(&step.switch) {
                    if s.set(step.on) {
                        changed.push(s.name);
                    }
                    tracing::info!("toggle {} -> {} at frame {frame}", s.name, if step.on { "on" } else { "off" });
                    let at = ALL.iter().position(|a| a.name == s.name).unwrap_or(0);
                    PHASE.store(1 + 2 * at + usize::from(step.on), Relaxed);
                }
            }
            None => {
                PHASE.store(0, Relaxed);
                self.done = true;
                tracing::info!(frame, "toggle plan finished");
            }
        }
        self.current = index;
        changed
    }
}

static RUNNER: Mutex<Option<Runner>> = Mutex::new(None);

/// Called once a frame, between two frames, by the present path. Does nothing
/// unless `M1LK_TOGGLE_PLAN` is set. `draws_total` is the count of draws run
/// so far. Returns the names of the switches whose value changed, so the
/// caller can drop what it cached under the old value.
pub fn on_frame(frame: u64, draws_total: u64) -> Vec<&'static str> {
    if !plan_active() {
        return Vec::new();
    }
    let mut guard = RUNNER.lock().unwrap_or_else(|e| e.into_inner());
    let runner = guard.get_or_insert_with(Runner::new);
    runner.tick(Instant::now(), frame, draws_total)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(steps: &[Step]) -> Vec<(&str, bool, u64, u64)> {
        steps.iter().map(|s| (s.switch.as_str(), s.on, s.start_s, s.end_s)).collect()
    }

    #[test]
    fn schedule_runs_on_then_off_and_swaps_on_the_second_pass() {
        let steps = schedule(&["A", "B"], 60, 2);
        assert_eq!(
            flat(&steps),
            vec![
                ("A", true, 0, 60),
                ("A", false, 60, 120),
                ("B", true, 120, 180),
                ("B", false, 180, 240),
                ("A", false, 240, 300),
                ("A", true, 300, 360),
                ("B", false, 360, 420),
                ("B", true, 420, 480),
            ]
        );
    }

    #[test]
    fn schedule_of_nothing_is_empty() {
        assert!(schedule::<&str>(&[], 60, 2).is_empty());
        assert!(schedule(&["A"], 60, 0).is_empty());
        let three = schedule(&["A"], 10, 3);
        assert_eq!(flat(&three)[4], ("A", true, 40, 50));
        assert_eq!(three.last().map(|s| s.end_s), Some(60));
    }

    #[test]
    fn start_only_switches_and_strangers_are_refused() {
        let (keep, refused) = parse_plan("LOOKUP_CACHE, m1lk_range_memo +WATCH_SCAN|GPU_DETILE,NOPE,LOOKUP_CACHE,,LOW_SHADOWS");
        let names: Vec<&str> = keep.iter().map(|s| s.name).collect();
        assert_eq!(names, vec!["LOOKUP_CACHE", "RANGE_MEMO", "LOW_SHADOWS"]);
        let bad: Vec<&str> = refused.iter().map(|(w, _)| w.as_str()).collect();
        assert_eq!(bad, vec!["WATCH_SCAN", "GPU_DETILE", "NOPE", "LOOKUP_CACHE"]);
        assert!(refused[0].1.starts_with("start-only"));
    }

    #[test]
    fn every_switch_has_one_name_and_the_start_only_are_marked() {
        let mut names: Vec<&str> = ALL.iter().map(|s| s.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), ALL.len());
        let start_only: Vec<&str> = ALL.iter().filter(|s| !s.runtime_safe).map(|s| s.name).collect();
        assert_eq!(start_only, vec!["WATCH_SCAN", "LOWCOMMIT_BACKOFF", "INPLACE_CONSTANTS", "GPU_DETILE", "LEGACY_FMA", "NAN_GUARD"]);
    }

    #[test]
    fn start_values_follow_the_text() {
        assert!(is_one(Some("1")) && !is_one(Some("0")) && !is_one(None));
        assert!(not_zero(None) && not_zero(Some("1")) && !not_zero(Some("0")));
        assert!(not_off_word(Some("2")) && not_off_word(Some("1")) && !not_off_word(Some("off")) && !not_off_word(None));
    }

    #[test]
    fn runner_waits_for_the_world_then_flips_in_step_order() {
        let mut r = Runner {
            steps: schedule(&["COPY_PLAIN"], 10, 1),
            delay: Duration::from_secs(5),
            last_draws: 0,
            t0: None,
            current: None,
            done: false,
        };
        let t = Instant::now();
        COPY_PLAIN.set(false);
        // Loading: few draws, nothing armed.
        assert!(r.tick(t, 1, 10).is_empty());
        assert!(r.t0.is_none());
        // The world: armed, nothing flips during the delay.
        assert!(r.tick(t, 2, 1000).is_empty());
        assert!(r.t0.is_some());
        assert!(r.tick(t + Duration::from_secs(4), 3, 2000).is_empty());
        // On at the start of the first step.
        assert_eq!(r.tick(t + Duration::from_secs(5), 4, 3000), vec!["COPY_PLAIN"]);
        assert!(COPY_PLAIN.is_on());
        // Same step: nothing.
        assert!(r.tick(t + Duration::from_secs(9), 5, 4000).is_empty());
        // Off in the second step.
        assert_eq!(r.tick(t + Duration::from_secs(15), 6, 5000), vec!["COPY_PLAIN"]);
        assert!(!COPY_PLAIN.is_on());
        // Past the end: finished, back to the start value.
        r.tick(t + Duration::from_secs(40), 7, 6000);
        assert!(r.done);
        assert_eq!(COPY_PLAIN.is_on(), COPY_PLAIN.base());
    }
}
