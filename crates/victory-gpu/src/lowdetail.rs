//! Optional lower-detail preset: skips CPU-side draw preparation for work
//! that costs a lot and adds little. Everything here is off by default and
//! changes nothing unless a switch is set. It lowers picture quality.
//!
//! * `M1LK_LOW_SHADOWS=<n>` (`1` keeps the nearest cascade; a larger number
//!   keeps that many): draws and mesh dispatches that only write depth into
//!   one slice of a depth texture array are not run for slices from `n` on.
//!   The shape is generic (no render targets, a depth view on one slice of
//!   an array of 2 or more), so no shader or title data is involved. The
//!   title's own clears of the array still run, so a skipped slice is left
//!   cleared, not garbage.
//! * `M1LK_DRAW_RULES=<file>`: a rule file of your own. One rule a line, `#`
//!   starts a note:
//!
//! ```text
//! <16 hex digits> skip              # never run
//! <16 hex digits> every <n>         # run in one frame of n
//! <16 hex digits> every <n> at <k>  # run in frames whose number % n == k
//! ```
//!
//! The hash is the one the log prints for a translated shader (any stage of
//! a pipeline matches). The repository ships no real hashes; reflections,
//! fog and the like can only be found this way, by a user who has measured
//! their own copy.
//!
//! The rule format and the idea of frame-thinned rules follow Kitty6's mod
//! files (GPL-3.0, credit to its authors).

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

/// What a rule does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Action {
    Skip,
    /// Run when the frame's number % `n` == `at`.
    Every { n: u64, at: u64 },
}

/// A rule of a rule file.
#[derive(Debug)]
pub(crate) struct Rule {
    pub(crate) hash: String,
    pub(crate) action: Action,
    /// The file and line it came from.
    pub(crate) from: String,
}

/// The rules of one file's text; lines that are not rules are said and left
/// out.
pub(crate) fn parse(text: &str, name: &str) -> Vec<Rule> {
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let words: Vec<&str> = line.split_whitespace().collect();
        let hash = words[0].to_ascii_lowercase();
        let action = match words[1..] {
            ["skip"] => Some(Action::Skip),
            ["every", n] => n.parse().ok().filter(|&n| n > 0).map(|n| Action::Every { n, at: 0 }),
            ["every", n, "at", k] => match (n.parse::<u64>(), k.parse::<u64>()) {
                (Ok(n), Ok(at)) if n > 0 && at < n => Some(Action::Every { n, at }),
                _ => None,
            },
            _ => None,
        };
        match action {
            Some(action) if hash.len() == 16 && hash.bytes().all(|c| c.is_ascii_hexdigit()) => {
                out.push(Rule { hash, action, from: format!("{name}:{}", i + 1) });
            }
            _ => tracing::warn!(line, at = %format!("{name}:{}", i + 1), "not a draw rule"),
        }
    }
    out
}

/// How many cascades `M1LK_LOW_SHADOWS`'s text keeps (`None`: off).
pub(crate) fn parse_keep(value: &str) -> Option<u32> {
    match value.trim().to_ascii_lowercase().as_str() {
        "" | "0" | "off" | "false" | "no" => None,
        other => Some(other.parse::<u32>().ok().filter(|&n| n > 0).unwrap_or(1).min(16)),
    }
}

/// Slices of a shadow array kept, if the switch is on.
fn keep() -> Option<u32> {
    static KEEP: OnceLock<Option<u32>> = OnceLock::new();
    *KEEP.get_or_init(|| {
        let keep = std::env::var("VICTORY_LOW_SHADOWS").ok().and_then(|v| parse_keep(&v));
        if let Some(n) = keep {
            tracing::warn!(keep = n, "M1LK_LOW_SHADOWS: shadow slices from {n} on are not drawn (lower picture quality)");
        }
        keep
    })
}

/// Every rule of the file, if `M1LK_DRAW_RULES` names one. Logs how many.
fn rules() -> &'static [Rule] {
    static RULES: OnceLock<Vec<Rule>> = OnceLock::new();
    RULES.get_or_init(|| {
        let Some(file) = std::env::var("VICTORY_DRAW_RULES").ok().filter(|f| !f.trim().is_empty()) else {
            return Vec::new();
        };
        match std::fs::read_to_string(file.trim()) {
            Ok(text) => {
                let name = std::path::Path::new(file.trim()).file_name().map_or_else(|| "rules".into(), |n| n.to_string_lossy().into_owned());
                let rules = parse(&text, &name);
                tracing::warn!(rules = rules.len(), "M1LK_DRAW_RULES: draw rules loaded (lower picture quality)");
                rules
            }
            Err(e) => {
                tracing::warn!(error = %e, "M1LK_DRAW_RULES: the rule file was not read");
                Vec::new()
            }
        }
    })
}

/// Loads the switches and logs what is on. Safe to call more than once.
pub(crate) fn init() {
    let _ = keep();
    let _ = rules();
}

/// The rule for a pipeline of shaders with these hashes, if one names any.
pub(crate) fn rule_for<'a>(hashes: impl IntoIterator<Item = &'a str>) -> Option<u16> {
    let rules = rules();
    if rules.is_empty() {
        return None;
    }
    let hashes: Vec<&str> = hashes.into_iter().collect();
    let i = rules.iter().position(|r| hashes.contains(&r.hash.as_str()))?;
    tracing::info!(rule = %rules[i].from, "a pipeline a draw rule applies to");
    u16::try_from(i).ok()
}

/// Whether `action` holds work back in frame `frame`.
pub(crate) fn holds_in(action: Action, frame: u64) -> bool {
    match action {
        Action::Skip => true,
        Action::Every { n, at } => frame % n != at,
    }
}

/// Whether rule `i` holds this frame's draw or dispatch back.
pub(crate) fn holds(i: u16) -> bool {
    rules().get(usize::from(i)).is_some_and(|r| holds_in(r.action, crate::queue::frame_number()))
}

/// Whether a draw into depth slice `slice` of a shadow array is left out.
pub(crate) fn shadow_slice_held(slice: u32) -> bool {
    keep().is_some_and(|n| slice >= n)
}

/// Whether the switch for shadow slices is on.
pub(crate) fn shadows_on() -> bool {
    keep().is_some()
}

/// Draws and dispatches left out since the last frame line.
static SKIPPED: AtomicU64 = AtomicU64::new(0);

/// Counts one left out.
pub(crate) fn note_skipped() {
    SKIPPED.fetch_add(1, Ordering::Relaxed);
}

/// Left out since the last call.
pub(crate) fn take_skipped() -> u64 {
    SKIPPED.swap(0, Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules_are_read_and_bad_lines_left_out() {
        let text = "# a note\n\
                    0123456789ABCDEF skip   # loud\n\
                    fedcba9876543210 every 2\n\
                    00000000000000aa every 3 at 1\n\
                    00000000000000ab every 3 at 3\n\
                    00000000000000ad every 0\n\
                    short skip\n\
                    00000000000000ac sometimes\n";
        let rules = parse(text, "t.rules");
        let got: Vec<(&str, Action)> = rules.iter().map(|r| (r.hash.as_str(), r.action)).collect();
        assert_eq!(
            got,
            vec![
                ("0123456789abcdef", Action::Skip),
                ("fedcba9876543210", Action::Every { n: 2, at: 0 }),
                ("00000000000000aa", Action::Every { n: 3, at: 1 }),
            ]
        );
        assert_eq!(rules[0].from, "t.rules:2");
    }

    #[test]
    fn every_runs_in_one_frame_of_n() {
        let every = Action::Every { n: 4, at: 1 };
        let held: Vec<bool> = (0..8).map(|f| holds_in(every, f)).collect();
        assert_eq!(held, [true, false, true, true, true, false, true, true]);
        assert!(holds_in(Action::Skip, 7));
    }

    #[test]
    fn keep_parses_off_on_and_counts() {
        for off in ["", "0", "off", " FALSE ", "no"] {
            assert_eq!(parse_keep(off), None, "{off:?}");
        }
        assert_eq!(parse_keep("1"), Some(1));
        assert_eq!(parse_keep("on"), Some(1));
        assert_eq!(parse_keep("3"), Some(3));
        assert_eq!(parse_keep("99"), Some(16));
    }
}
