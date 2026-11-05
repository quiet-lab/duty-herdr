//! Selection of the status icon frame shown by the client.
//!
//! The frame is a pure function of the client's monotonic clock and of the row's
//! own clock: a period (the state's `frame_interval_ms` scaled by the row's speed
//! factor) and a start offset in time. Every row changes its frame on its own
//! boundaries `k * period - offset`, so the gaps between its changes are equal.
//! It is presentation state: the server, the API and the protocol know nothing
//! about it.

use std::time::{Duration, Instant};

use crate::api::schema::AgentStatus;
use crate::config::{StatusFramePhase, StatusIconFrames, StatusIcons};

/// Boundaries of different rows closer than this are served by one tick.
const BOUNDARY_MERGE_WINDOW_US: u64 = 2000;

/// Stable phase seed of a row: FNV-1a over the endpoint key and the row
/// identifier. Deterministic across restarts (no `RandomState`) and free of
/// allocation. The endpoint key keeps rows of different machines with the same
/// identifier apart.
pub(super) fn row_phase(endpoint: &str, id: &str) -> Option<u64> {
    const BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0100_0000_01b3;
    let mut hash = BASIS;
    for byte in endpoint
        .bytes()
        .chain(std::iter::once(0xff))
        .chain(id.bytes())
    {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(PRIME);
    }
    // Final mix (splitmix64) so that the high and low halves, used for the
    // speed and the offset, are well distributed even for short similar ids.
    hash ^= hash >> 30;
    hash = hash.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    hash ^= hash >> 27;
    hash = hash.wrapping_mul(0x94d0_49bb_1331_11eb);
    hash ^= hash >> 31;
    Some(hash)
}

/// Period multiplier of a row in thousandths, uniform in
/// `1000 - spread..=1000 + spread` and fixed by the row's phase seed. The high
/// half of the seed is used. Spread 0 gives 1000.
pub(super) fn row_speed_permille(phase: u64, spread_permille: u16) -> u64 {
    let spread = u64::from(spread_permille);
    1000 - spread + (phase >> 32) % (2 * spread + 1)
}

/// The clock of one row: its frame period and the shift of its start, both in
/// microseconds. The frame shown at `elapsed` is
/// `((elapsed + offset) / period) % frames`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct RowClock {
    period_us: u64,
    offset_us: u64,
}

impl RowClock {
    /// `interval` is the state's frame period; `speed_permille` scales it
    /// (1000 keeps it). `offset_seed` is the row's phase seed, `None` starts the
    /// row at the epoch. The offset is uniform in `0..period * frames`.
    pub(super) fn new(
        interval: Duration,
        speed_permille: u64,
        offset_seed: Option<u64>,
        frames: usize,
    ) -> Self {
        let interval_us = u64::try_from(interval.as_micros()).unwrap_or(u64::MAX);
        // A zero period is rejected by the configuration; the guard keeps the
        // arithmetic total.
        let period_us = (interval_us.saturating_mul(speed_permille) / 1000).max(1);
        let span = period_us.saturating_mul(u64::try_from(frames).unwrap_or(u64::MAX).max(1));
        Self {
            period_us,
            offset_us: offset_seed.map_or(0, |seed| seed % span),
        }
    }

    /// Whole periods of the row's clock at `elapsed_us`; a frame change is a
    /// change of this number.
    fn tick(self, elapsed_us: u64) -> u64 {
        elapsed_us.saturating_add(self.offset_us) / self.period_us
    }

    fn frame_index(self, elapsed_us: u64, frames: usize) -> usize {
        match u64::try_from(frames) {
            Ok(0) | Err(_) => 0,
            Ok(frames) => usize::try_from(self.tick(elapsed_us) % frames).unwrap_or(0),
        }
    }

    /// The first boundary strictly after `after_us`, in microseconds since the epoch.
    fn next_boundary_us(self, after_us: u64) -> u64 {
        self.tick(after_us)
            .saturating_add(1)
            .saturating_mul(self.period_us)
            .saturating_sub(self.offset_us)
    }
}

/// The earliest boundary among `clocks` after `after_us`, moved to the latest
/// boundary within the merge window so that rows changing within 2 ms share a
/// tick.
fn merged_boundary_us(clocks: &[RowClock], after_us: u64) -> Option<u64> {
    let first = clocks
        .iter()
        .map(|clock| clock.next_boundary_us(after_us))
        .min()?;
    let limit = first.saturating_add(BOUNDARY_MERGE_WINDOW_US);
    clocks
        .iter()
        .map(|clock| clock.next_boundary_us(after_us))
        .filter(|boundary| *boundary <= limit)
        .max()
}

fn micros(elapsed: Duration) -> u64 {
    u64::try_from(elapsed.as_micros()).unwrap_or(u64::MAX)
}

/// `[ui.status_icons]` together with the frame of the composition being drawn.
#[derive(Debug)]
pub(crate) struct ShellStatusIcons {
    configured: StatusIcons,
    epoch: Instant,
    /// Time since the epoch at the last composition.
    elapsed: Duration,
    has_animation: bool,
    /// Clocks of the rows of the last composition that are in a state with an
    /// animated icon. Empty means nothing animates.
    row_clocks: Vec<RowClock>,
}

/// A row shown by a composition: its state and the phase seed of its identifier,
/// `None` for an aggregate without a row of its own.
pub(super) type ShownRow = (AgentStatus, Option<u64>);

impl ShellStatusIcons {
    pub(super) fn new(configured: StatusIcons) -> Self {
        Self {
            has_animation: has_animation(&configured),
            configured,
            epoch: Instant::now(),
            elapsed: Duration::ZERO,
            row_clocks: Vec::new(),
        }
    }

    /// Replaces the configuration, keeping the epoch so that a reload does not
    /// restart the animation.
    pub(super) fn set_configured(&mut self, configured: StatusIcons) {
        self.has_animation = has_animation(&configured);
        self.configured = configured;
        self.row_clocks.clear();
    }

    #[cfg(test)]
    pub(super) fn epoch(&self) -> Instant {
        self.epoch
    }

    #[cfg(test)]
    pub(super) fn configured(&self) -> &StatusIcons {
        &self.configured
    }

    /// Frame period of `status`.
    fn interval(&self, status: AgentStatus) -> Duration {
        let intervals = &self.configured.frame_interval_ms;
        let ms = match status {
            AgentStatus::Idle => intervals.idle,
            AgentStatus::Working => intervals.working,
            AgentStatus::Blocked => intervals.blocked,
            AgentStatus::Done => intervals.done,
            AgentStatus::Unknown => intervals.unknown,
        }
        .ms();
        Duration::from_millis(u64::from(ms))
    }

    /// Spread of the period for `status`; 0 without `per_row`.
    fn spread_permille(&self, status: AgentStatus) -> u16 {
        let spreads = &self.configured.frame_speed_spread;
        match (self.configured.frame_phase, status) {
            (StatusFramePhase::Synced, _) => 0,
            (StatusFramePhase::PerRow, AgentStatus::Idle) => spreads.idle.permille(),
            (StatusFramePhase::PerRow, AgentStatus::Working) => spreads.working.permille(),
            (StatusFramePhase::PerRow, AgentStatus::Blocked) => spreads.blocked.permille(),
            (StatusFramePhase::PerRow, AgentStatus::Done) => spreads.done.permille(),
            (StatusFramePhase::PerRow, AgentStatus::Unknown) => spreads.unknown.permille(),
        }
    }

    /// The clock of a row in `status` with the phase seed `phase`. The seed is
    /// ignored unless `frame_phase = "per_row"`, and `None` (an aggregate
    /// without a row) always runs on the state's own period from the epoch.
    fn clock_for(&self, status: AgentStatus, phase: Option<u64>, frames: usize) -> RowClock {
        let (seed, speed) = match (self.configured.frame_phase, phase) {
            (StatusFramePhase::Synced, _) | (StatusFramePhase::PerRow, None) => (None, 1000),
            (StatusFramePhase::PerRow, Some(phase)) => (
                Some(phase),
                row_speed_permille(phase, self.spread_permille(status)),
            ),
        };
        RowClock::new(self.interval(status), speed, seed, frames)
    }

    /// Fixes the frame for one composition and collects the clocks of the rows in
    /// an animated state. The rows are data, so nothing depends on how a frame is
    /// laid out in cells. A row that shows no icon, for lack of a token, of width
    /// or of space on screen, costs a repaint without a visible change; a missed
    /// row would stop the animation. Without animated icons in the configuration
    /// no row is read.
    pub(super) fn begin_composition(&mut self, now: Instant, rows: impl Iterator<Item = ShownRow>) {
        self.elapsed = now.saturating_duration_since(self.epoch);
        let mut clocks = std::mem::take(&mut self.row_clocks);
        clocks.clear();
        if self.has_animation {
            // Aggregates (a collapsed sidebar, the mobile header) have no row of
            // their own: each state shown gets one clock without a seed.
            let mut aggregates = [false; 5];
            for (status, phase) in rows {
                let Some(frames) = self
                    .frames_for(status)
                    .filter(|frames| frames.is_animated())
                else {
                    continue;
                };
                let count = frames.frames().len();
                if phase.is_some() && self.configured.frame_phase == StatusFramePhase::PerRow {
                    clocks.push(self.clock_for(status, phase, count));
                }
                let seen = &mut aggregates[status_slot(status)];
                if !*seen {
                    *seen = true;
                    clocks.push(self.clock_for(status, None, count));
                }
            }
            clocks.sort_unstable_by_key(|clock| (clock.period_us, clock.offset_us));
            clocks.dedup();
        }
        self.row_clocks = clocks;
    }

    /// WARNING: the frame period is a timer, an exception to the station's ban on
    /// timers. Reason: frames change with time and no event source exists for them.
    /// Removal condition: animated status icons are dropped from the fork.
    ///
    /// The nearest moment a shown frame goes stale, over the rows in an animated
    /// state; boundaries within 2 ms are served at once. `None` keeps the client
    /// loop's wake-ups as they are without animation.
    pub(super) fn next_frame_at(&self, now: Instant) -> Option<Instant> {
        let after = micros(now.saturating_duration_since(self.epoch));
        let boundary = merged_boundary_us(&self.row_clocks, after)?;
        self.epoch.checked_add(Duration::from_micros(boundary))
    }

    /// Whether `now` belongs to another frame than the last composition drew, for
    /// any row in an animated state.
    pub(super) fn frame_stale(&self, now: Instant) -> bool {
        let drawn = micros(self.elapsed);
        let now = micros(now.saturating_duration_since(self.epoch));
        self.row_clocks
            .iter()
            .any(|clock| clock.tick(now) != clock.tick(drawn))
    }

    /// The frame of `status` for a row with the phase seed `phase`; see
    /// [`Self::clock_for`].
    pub(super) fn override_for(&self, status: AgentStatus, phase: Option<u64>) -> Option<&str> {
        let frames = self.frames_for(status)?;
        let count = frames.frames().len();
        let clock = self.clock_for(status, phase, count);
        frames
            .frames()
            .get(clock.frame_index(micros(self.elapsed), count))
            .map(String::as_str)
    }

    fn frames_for(&self, status: AgentStatus) -> Option<&StatusIconFrames> {
        match status {
            AgentStatus::Idle => self.configured.idle.as_ref(),
            AgentStatus::Working => self.configured.working.as_ref(),
            AgentStatus::Blocked => self.configured.blocked.as_ref(),
            AgentStatus::Done => self.configured.done.as_ref(),
            AgentStatus::Unknown => self.configured.unknown.as_ref(),
        }
    }
}

fn status_slot(status: AgentStatus) -> usize {
    match status {
        AgentStatus::Idle => 0,
        AgentStatus::Working => 1,
        AgentStatus::Blocked => 2,
        AgentStatus::Done => 3,
        AgentStatus::Unknown => 4,
    }
}

fn has_animation(icons: &StatusIcons) -> bool {
    [
        &icons.idle,
        &icons.working,
        &icons.blocked,
        &icons.done,
        &icons.unknown,
    ]
    .into_iter()
    .flatten()
    .any(StatusIconFrames::is_animated)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frames(list: &[&str]) -> StatusIconFrames {
        StatusIconFrames::new(list.iter().map(|frame| (*frame).to_owned()).collect()).unwrap()
    }

    fn moon() -> StatusIconFrames {
        frames(&["🌑", "🌒", "🌓", "🌔"])
    }

    fn animated_icons() -> ShellStatusIcons {
        ShellStatusIcons::new(StatusIcons {
            working: Some(moon()),
            idle: Some("z".into()),
            ..StatusIcons::default()
        })
    }

    /// Working 83 ms with a spread of 0.10 and blocked 333 ms without one, rows
    /// started at their own offsets.
    fn timed_icons() -> ShellStatusIcons {
        let timing: StatusIcons = toml::from_str(
            "frame_interval_ms = { working = 83, blocked = 333 }\n\
             frame_phase = \"per_row\"\n\
             frame_speed_spread = { working = 0.10 }",
        )
        .unwrap();
        ShellStatusIcons::new(StatusIcons {
            working: Some(moon()),
            blocked: Some(frames(&["!", "?"])),
            ..timing
        })
    }

    /// A seed whose speed factor is `permille` for a spread of 0.10 and whose
    /// offset is `low` modulo the span.
    fn seed_with_speed(permille: u64, low: u32) -> u64 {
        assert!((900..=1100).contains(&permille));
        ((permille - 900) << 32) | u64::from(low)
    }

    fn at(icons: &ShellStatusIcons, us: u64) -> Instant {
        icons.epoch + Duration::from_micros(us)
    }

    fn shown(icons: &mut ShellStatusIcons, status: AgentStatus, seed: u64, us: u64) -> String {
        icons.begin_composition(at(icons, us), [(status, Some(seed))].into_iter());
        icons.override_for(status, Some(seed)).unwrap().to_owned()
    }

    #[test]
    fn row_clock_counts_periods_from_the_shifted_start() {
        let clock = RowClock {
            period_us: 400_000,
            offset_us: 100_000,
        };
        assert_eq!(clock.tick(0), 0);
        assert_eq!(clock.tick(299_999), 0);
        assert_eq!(clock.tick(300_000), 1);
        assert_eq!(clock.next_boundary_us(0), 300_000);
        assert_eq!(clock.next_boundary_us(300_000), 700_000);
        assert_eq!(clock.frame_index(300_000, 4), 1);
        assert!(clock.frame_index(u64::MAX, 4) < 4);
        assert_eq!(clock.frame_index(5, 0), 0);
    }

    #[test]
    fn row_clock_offset_is_uniform_within_the_cycle_and_total() {
        let interval = Duration::from_millis(83);
        for seed in [0, 1, u64::MAX, 0x1234_5678_9abc_def0] {
            let clock = RowClock::new(interval, 1070, Some(seed), 8);
            assert_eq!(clock.period_us, 88_810);
            assert!(clock.offset_us < 88_810 * 8);
        }
        assert_eq!(RowClock::new(interval, 1000, None, 8).offset_us, 0);
        // A degenerate period and an empty list stay total.
        let clock = RowClock::new(Duration::ZERO, 0, Some(7), 0);
        assert_eq!(clock.frame_index(u64::MAX, 0), 0);
        assert!(clock.next_boundary_us(u64::MAX) >= 1);
    }

    #[test]
    fn row_phase_is_deterministic_and_separates_endpoints() {
        assert_eq!(row_phase("", "w4"), row_phase("", "w4"));
        assert_ne!(row_phase("", "w4"), row_phase("", "w5"));
        assert_ne!(row_phase("", "w4"), row_phase("ssh-a", "w4"));
        assert_ne!(row_phase("ssh-a", "w4"), row_phase("ssh-b", "w4"));
    }

    #[test]
    fn row_speed_is_deterministic_and_within_the_spread() {
        assert_eq!(row_speed_permille(u64::MAX, 0), 1000);
        for spread in [0, 1, 250, 500] {
            let mut distinct = std::collections::BTreeSet::new();
            for id in 0..200 {
                let phase = row_phase("", &format!("w{id}:p1")).unwrap();
                let speed = row_speed_permille(phase, spread);
                assert_eq!(speed, row_speed_permille(phase, spread));
                assert!((1000 - u64::from(spread)..=1000 + u64::from(spread)).contains(&speed));
                distinct.insert(speed);
            }
            if spread >= 250 {
                assert!(distinct.len() > 20, "{} speeds", distinct.len());
            }
        }
        assert_eq!(row_speed_permille(0, 500), 500);
        assert_eq!(
            row_speed_permille(u64::MAX, 500),
            500 + (u64::from(u32::MAX) % 1001)
        );
    }

    #[test]
    fn row_changes_frame_exactly_on_its_shifted_boundaries() {
        let mut icons = timed_icons();
        // Factor 1.07: the period is 83 ms * 1.07 = 88.81 ms.
        let seed = seed_with_speed(1070, 12_345);
        let period = 88_810;
        let offset = seed % (period * 4);
        assert_ne!(offset, 0);
        let moons = ["🌑", "🌒", "🌓", "🌔"];
        let mut previous = None;
        for k in 1..=24u64 {
            let Some(boundary) = (k * period).checked_sub(offset) else {
                continue;
            };
            if boundary < period {
                continue;
            }
            assert_eq!(
                shown(&mut icons, AgentStatus::Working, seed, boundary - 1),
                moons[usize::try_from((k - 1) % 4).unwrap()],
                "just before boundary {k}"
            );
            assert_eq!(
                shown(&mut icons, AgentStatus::Working, seed, boundary),
                moons[usize::try_from(k % 4).unwrap()],
                "at boundary {k}"
            );
            if let Some(previous) = previous {
                assert_eq!(boundary - previous, period, "equal gaps");
            }
            previous = Some(boundary);
        }
    }

    #[test]
    fn next_boundary_of_a_row_is_the_shifted_one() {
        let mut icons = timed_icons();
        let seed = seed_with_speed(1070, 12_345);
        let offset = seed % (88_810 * 4);
        icons.begin_composition(
            at(&icons, 0),
            [(AgentStatus::Working, Some(seed))].into_iter(),
        );
        let first = (offset.div_ceil(88_810).max(1)) * 88_810 - offset;
        // The aggregate clock of the state (no offset, period 83 ms) is another
        // boundary; the row's own is among the candidates and never skipped.
        let own = RowClock::new(Duration::from_millis(83), 1070, Some(seed), 4);
        assert!(icons.row_clocks.contains(&own));
        assert_eq!(own.next_boundary_us(0), first);
        let deadline = icons.next_frame_at(at(&icons, 0)).unwrap();
        let deadline_us = u64::try_from(deadline.duration_since(icons.epoch).as_micros()).unwrap();
        assert!(deadline_us <= first);
    }

    #[test]
    fn rows_with_different_factors_drift_apart_and_blocked_keeps_its_interval() {
        let mut icons = timed_icons();
        let slow = seed_with_speed(1100, 0);
        let fast = seed_with_speed(900, 0);
        let mut difference = std::collections::BTreeSet::new();
        for step in 0..40u64 {
            let us = step * 83_000;
            let now = at(&icons, us);
            icons.begin_composition(
                now,
                [
                    (AgentStatus::Working, Some(slow)),
                    (AgentStatus::Working, Some(fast)),
                ]
                .into_iter(),
            );
            let slow_frame = icons
                .override_for(AgentStatus::Working, Some(slow))
                .map(str::to_owned);
            let fast_frame = icons
                .override_for(AgentStatus::Working, Some(fast))
                .map(str::to_owned);
            difference.insert((slow_frame, fast_frame));
        }
        assert!(difference.len() > 4, "{difference:?}");

        // Blocked has no spread: its period is exactly the interval for any seed.
        let seed = row_phase("", "pane_9").unwrap();
        let offset = seed % (333_000 * 2);
        let expected = ["!", "?"];
        for k in 1..=10u64 {
            let Some(boundary) = (k * 333_000).checked_sub(offset) else {
                continue;
            };
            if boundary < 333_000 {
                continue;
            }
            assert_eq!(
                shown(&mut icons, AgentStatus::Blocked, seed, boundary - 1),
                expected[usize::try_from((k - 1) % 2).unwrap()]
            );
            assert_eq!(
                shown(&mut icons, AgentStatus::Blocked, seed, boundary),
                expected[usize::try_from(k % 2).unwrap()]
            );
        }
    }

    #[test]
    fn deadline_is_the_nearest_boundary_among_the_rows() {
        let clocks = [
            RowClock {
                period_us: 100_000,
                offset_us: 30_000,
            },
            RowClock {
                period_us: 100_000,
                offset_us: 60_000,
            },
            RowClock {
                period_us: 70_000,
                offset_us: 0,
            },
        ];
        // Boundaries after 0: 70_000, 40_000, 70_000.
        assert_eq!(merged_boundary_us(&clocks, 0), Some(40_000));
        assert_eq!(merged_boundary_us(&clocks, 40_000), Some(70_000));
        assert_eq!(merged_boundary_us(&[], 0), None);
    }

    #[test]
    fn boundaries_within_two_milliseconds_share_one_tick() {
        let clock = |offset_us| RowClock {
            period_us: 100_000,
            offset_us,
        };
        // Boundaries at 99_000, 99_500 and 101_000 (the last one 2000 us after the
        // first): all three are served by the latest of them.
        let clocks = [
            clock(1000),
            clock(500),
            RowClock {
                period_us: 101_000,
                offset_us: 0,
            },
        ];
        assert_eq!(merged_boundary_us(&clocks, 0), Some(101_000));
        // A boundary 2001 us after the first one is a tick of its own.
        let clocks = [
            clock(1000),
            RowClock {
                period_us: 101_001,
                offset_us: 0,
            },
        ];
        assert_eq!(merged_boundary_us(&clocks, 0), Some(99_000));
        assert_eq!(merged_boundary_us(&clocks, 99_000), Some(101_001));
    }

    #[test]
    fn deadline_exists_only_while_a_row_is_in_an_animated_state() {
        let mut icons = animated_icons();
        let start = icons.epoch;
        assert_eq!(icons.next_frame_at(start), None);
        assert!(!icons.frame_stale(start + Duration::from_secs(10)));

        // A static override and a status without an override do not animate.
        icons.begin_composition(
            start,
            [(AgentStatus::Idle, None), (AgentStatus::Done, None)].into_iter(),
        );
        assert_eq!(icons.next_frame_at(start), None);
        assert!(!icons.frame_stale(start + Duration::from_secs(10)));

        icons.begin_composition(
            start,
            [(AgentStatus::Idle, None), (AgentStatus::Working, None)].into_iter(),
        );
        assert_eq!(
            icons.next_frame_at(start + Duration::from_millis(100)),
            Some(start + Duration::from_millis(400))
        );
        assert_eq!(
            icons.next_frame_at(start + Duration::from_millis(400)),
            Some(start + Duration::from_millis(800))
        );

        // The next composition without such a row removes the deadline.
        icons.begin_composition(
            start + Duration::from_millis(400),
            [(AgentStatus::Idle, None)].into_iter(),
        );
        assert_eq!(icons.next_frame_at(start), None);
    }

    #[test]
    fn frame_is_stale_only_after_the_period_ends() {
        let mut icons = animated_icons();
        let start = icons.epoch;
        icons.begin_composition(start, [(AgentStatus::Working, None)].into_iter());
        assert_eq!(icons.override_for(AgentStatus::Working, None), Some("🌑"));
        assert!(!icons.frame_stale(start + Duration::from_millis(399)));
        assert!(icons.frame_stale(start + Duration::from_millis(400)));

        icons.begin_composition(
            start + Duration::from_millis(400),
            [(AgentStatus::Working, None)].into_iter(),
        );
        assert_eq!(icons.override_for(AgentStatus::Working, None), Some("🌒"));
        assert!(!icons.frame_stale(start + Duration::from_millis(799)));
    }

    #[test]
    fn single_frame_lists_never_arm_the_deadline() {
        let mut icons = ShellStatusIcons::new(StatusIcons {
            working: Some("x".into()),
            ..StatusIcons::default()
        });
        let start = icons.epoch;
        icons.begin_composition(start, [(AgentStatus::Working, None)].into_iter());
        assert_eq!(icons.override_for(AgentStatus::Working, None), Some("x"));
        assert_eq!(icons.next_frame_at(start), None);
    }

    #[test]
    fn reload_keeps_the_epoch_and_drops_visibility() {
        let mut icons = animated_icons();
        let start = icons.epoch;
        icons.begin_composition(start, [(AgentStatus::Working, None)].into_iter());
        assert!(!icons.row_clocks.is_empty());
        icons.set_configured(StatusIcons::default());
        assert_eq!(icons.epoch, start);
        assert!(icons.row_clocks.is_empty());
        assert_eq!(icons.override_for(AgentStatus::Working, None), None);
    }

    #[test]
    fn per_row_offsets_and_spreads_follow_the_configuration() {
        let mut icons = StatusIcons {
            working: Some(moon()),
            frame_phase: StatusFramePhase::PerRow,
            ..StatusIcons::default()
        };
        let mut shown = ShellStatusIcons::new(icons.clone());
        shown.begin_composition(shown.epoch, std::iter::empty());
        let seen: std::collections::BTreeSet<_> = (0..16)
            .map(|id| shown.override_for(AgentStatus::Working, row_phase("", &format!("w{id}"))))
            .collect();
        assert!(seen.len() > 1);
        icons.frame_phase = StatusFramePhase::Synced;
        let mut shown = ShellStatusIcons::new(icons);
        shown.begin_composition(shown.epoch, std::iter::empty());
        assert_eq!(
            shown.override_for(AgentStatus::Working, Some(12345)),
            Some("🌑")
        );
    }

    #[test]
    fn aggregates_run_on_the_state_period_from_the_epoch() {
        let spread: StatusIcons = toml::from_str("frame_speed_spread = 0.5").unwrap();
        let mut icons = ShellStatusIcons::new(StatusIcons {
            working: Some(moon()),
            frame_phase: StatusFramePhase::PerRow,
            ..spread
        });
        let start = icons.epoch;
        icons.begin_composition(
            start + Duration::from_millis(400),
            [(AgentStatus::Working, None)].into_iter(),
        );
        assert_eq!(icons.override_for(AgentStatus::Working, None), Some("🌒"));
        // Only the aggregate clock runs: the nearest boundary is the next period.
        assert_eq!(
            icons.next_frame_at(start + Duration::from_millis(400)),
            Some(start + Duration::from_millis(800))
        );
    }

    #[test]
    fn per_state_intervals_set_each_period() {
        let mut icons = timed_icons();
        let start = icons.epoch;
        icons.begin_composition(
            start,
            [(AgentStatus::Blocked, None), (AgentStatus::Working, None)].into_iter(),
        );
        assert_eq!(
            icons.next_frame_at(start),
            Some(start + Duration::from_millis(83))
        );
        assert_eq!(icons.override_for(AgentStatus::Blocked, None), Some("!"));
        assert_eq!(icons.override_for(AgentStatus::Working, None), Some("🌑"));
        icons.begin_composition(
            start + Duration::from_millis(333),
            [(AgentStatus::Blocked, None)].into_iter(),
        );
        assert_eq!(icons.override_for(AgentStatus::Blocked, None), Some("?"));
    }
}
