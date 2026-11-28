//! (M1LK Texture Use) How much host-texture memory no draw ever reads, off by default
//! (`VICTORY_TEXTURE_USE_PROF=1`). Read-only measurement: it changes nothing the host does.
//!
//! A title texture is "read" when a draw's shader view of it (an SRV, or an unordered access
//! view) is made ready (`draw::readable_view`, the one path every table, early texture pass
//! and slot pass goes through). That stamps the frame number on the texture. About every two
//! seconds the line walks the live textures and sums, by the stamp, the host bytes read this
//! frame, read in the last [`WINDOW`] frames, read earlier (stale), and never read since the
//! title made the texture. A texture whose host copy exists but that no draw ever read is a
//! copy that could have been left out (or let go).
//!
//! Cost: off, one cached bool test per view. On, one relaxed atomic store per view made ready.
//! The walk takes each texture's lock with `try_lock` (a busy one is counted as skipped).
//! Limits: a table copied whole stamps every texture in it, so "read" is an upper bound on
//! what the shaders sample; depth/colour twins' bytes are not counted.

#![allow(dead_code)]

use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

/// `VICTORY_TEXTURE_USE_PROF=1`.
pub(crate) fn on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("VICTORY_TEXTURE_USE_PROF").is_ok_and(|v| v.trim() == "1"))
}

/// Frames "recent" covers.
pub(crate) const WINDOW: u64 = 300;

static FRAME: AtomicU64 = AtomicU64::new(0);

/// A frame was presented.
pub(crate) fn frame() {
    if on() {
        FRAME.fetch_add(1, Relaxed);
    }
}

/// The stamp a read now leaves: the frame plus one, so 0 means never read.
pub(crate) fn stamp() -> u64 {
    FRAME.load(Relaxed) + 1
}

/// The frame the stamp counts toward the line.
pub(crate) fn now() -> u64 {
    FRAME.load(Relaxed) + 1
}

/// Which age a stamp has at `now` (both from [`stamp`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Age {
    Never,
    /// Read in the current frame (the last one presented or the one in progress).
    Now,
    /// Read within [`WINDOW`] frames, but not now.
    Recent,
    /// Read, longer ago.
    Stale,
}

pub(crate) fn age(stamp: u64, now: u64) -> Age {
    if stamp == 0 {
        Age::Never
    } else {
        let ago = now.saturating_sub(stamp);
        if ago <= 1 {
            Age::Now
        } else if ago <= WINDOW {
            Age::Recent
        } else {
            Age::Stale
        }
    }
}

/// Kinds of host copy the sums split by.
#[derive(Clone, Copy)]
#[repr(usize)]
pub(crate) enum Kind {
    /// Contents come from title memory.
    Fill,
    /// The host renders to or writes it.
    Written,
}

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub(crate) struct Sum {
    pub(crate) count: u64,
    pub(crate) bytes: u64,
}

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub(crate) struct Row {
    pub(crate) all: Sum,
    pub(crate) now: Sum,
    pub(crate) recent: Sum,
    pub(crate) stale: Sum,
    pub(crate) never: Sum,
}

impl Row {
    pub(crate) fn add(&mut self, bytes: u64, age: Age) {
        let part = |s: &mut Sum| {
            s.count += 1;
            s.bytes += bytes;
        };
        part(&mut self.all);
        match age {
            Age::Never => part(&mut self.never),
            Age::Now => part(&mut self.now),
            Age::Recent => part(&mut self.recent),
            Age::Stale => part(&mut self.stale),
        }
    }

    /// Read in the last frame / within the window (the frame's included).
    fn read_recent(&self) -> Sum {
        Sum { count: self.now.count + self.recent.count, bytes: self.now.bytes + self.recent.bytes }
    }
}

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub(crate) struct Census {
    pub(crate) rows: [Row; 2],
    /// Textures whose lock was busy.
    pub(crate) skipped: u64,
}

impl Census {
    pub(crate) fn add(&mut self, kind: Kind, bytes: u64, stamp: u64, now: u64) {
        self.rows[kind as usize].add(bytes, age(stamp, now));
    }

    fn total(&self) -> Row {
        let mut t = Row::default();
        for r in &self.rows {
            for (a, b) in [
                (&mut t.all, r.all),
                (&mut t.now, r.now),
                (&mut t.recent, r.recent),
                (&mut t.stale, r.stale),
                (&mut t.never, r.never),
            ] {
                a.count += b.count;
                a.bytes += b.bytes;
            }
        }
        t
    }

    pub(crate) fn text(&self, now: u64) -> String {
        let mb = |s: Sum| s.bytes as f64 / (1 << 20) as f64;
        let t = self.total();
        let one = |name: &str, r: &Row| {
            format!(
                "{name}: total={:.0} MB read_1f={:.0} read_recent={:.0} never_read={:.0} (count never={} of {})",
                mb(r.all),
                mb(r.now),
                mb(r.read_recent()),
                mb(r.never),
                r.never.count,
                r.all.count,
            )
        };
        format!(
            "M1LK Texture Use: total={:.0} MB read_1f={:.0} read_recent={:.0} stale={:.0} never_read={:.0} \
             (count never={} of {}) | {} | {} | frame={} window={} skipped={}",
            mb(t.all),
            mb(t.now),
            mb(t.read_recent()),
            mb(t.stale),
            mb(t.never),
            t.never.count,
            t.all.count,
            one("fills", &self.rows[Kind::Fill as usize]),
            one("written", &self.rows[Kind::Written as usize]),
            now,
            WINDOW,
            self.skipped,
        )
    }
}

/// The line, about every two seconds (called with the frame line each second).
pub(crate) fn line() {
    if !on() {
        return;
    }
    static CALLS: AtomicU64 = AtomicU64::new(0);
    if CALLS.fetch_add(1, Relaxed) % 2 != 1 {
        return;
    }
    #[cfg(windows)]
    {
        let now = now();
        let census = crate::resource::use_census(now);
        tracing::info!("{}", census.text(now));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ages() {
        assert_eq!(age(0, 1000), Age::Never);
        assert_eq!(age(1000, 1000), Age::Now);
        assert_eq!(age(999, 1000), Age::Now);
        assert_eq!(age(998, 1000), Age::Recent);
        assert_eq!(age(1000 - WINDOW, 1000), Age::Recent);
        assert_eq!(age(1000 - WINDOW - 1, 1000), Age::Stale);
        // A stamp from a frame after `now` (a race) is current.
        assert_eq!(age(1002, 1000), Age::Now);
    }

    #[test]
    fn sums() {
        let mb = 1u64 << 20;
        let mut c = Census::default();
        c.add(Kind::Fill, 10 * mb, 0, 500);
        c.add(Kind::Fill, 20 * mb, 500, 500);
        c.add(Kind::Fill, 30 * mb, 400, 500);
        c.add(Kind::Fill, 40 * mb, 10, 500);
        c.add(Kind::Written, 50 * mb, 0, 500);
        let f = c.rows[0];
        assert_eq!((f.all.count, f.all.bytes), (4, 100 * mb));
        assert_eq!(f.never, Sum { count: 1, bytes: 10 * mb });
        assert_eq!(f.read_recent().bytes, 50 * mb);
        assert_eq!(f.stale.bytes, 40 * mb);
        let t = c.total();
        assert_eq!((t.all.count, t.all.bytes), (5, 150 * mb));
        assert_eq!(t.never, Sum { count: 2, bytes: 60 * mb });
        let text = c.text(500);
        assert!(text.starts_with("M1LK Texture Use: total=150 MB read_1f=20 read_recent=50 stale=40 never_read=60 (count never=2 of 5)"));
    }
}
