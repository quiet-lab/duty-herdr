//! Selection of the status icon frame shown by the client.
//!
//! The frame is a pure function of the client's monotonic clock, so every row in
//! one state shows the same frame. It is presentation state: the server, the API
//! and the protocol know nothing about it.

use std::time::{Duration, Instant};

use crate::api::schema::AgentStatus;
use crate::config::{StatusFramePhase, StatusIconFrames, StatusIcons};

/// Number of whole frame periods in `elapsed`. A zero period stays on tick 0;
/// the configuration rejects it, the guard only keeps the function total.
pub(super) fn frame_tick(elapsed: Duration, interval: Duration) -> u64 {
    let interval = interval.as_millis();
    if interval == 0 {
        return 0;
    }
    u64::try_from(elapsed.as_millis() / interval).unwrap_or(u64::MAX)
}

/// Index of the frame shown at `tick` shifted by `offset` frames. Lists of
/// different length stay on the same tick and wrap independently; the sum
/// cannot overflow because both terms are reduced first.
pub(super) fn frame_index(tick: u64, offset: u64, frames: usize) -> usize {
    match u64::try_from(frames) {
        Ok(0) | Err(_) => 0,
        Ok(frames) => usize::try_from((tick % frames + offset % frames) % frames).unwrap_or(0),
    }
}

/// Stable phase seed of a row: FNV-1a over the endpoint key and the row
/// identifier. Deterministic across restarts (no `RandomState`) and free of
/// allocation. The endpoint key keeps rows of different machines with the same
/// identifier apart.
pub(super) fn row_phase(endpoint: &str, id: &str) -> u64 {
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
    hash
}

/// Period multiplier of a row in thousandths, uniform in
/// `1000 - spread..=1000 + spread` and fixed by the row's phase seed. The high
/// half of the seed is used, the offset uses the low bits. Spread 0 gives 1000.
pub(super) fn row_speed_permille(phase: u64, spread_permille: u16) -> u64 {
    let spread = u64::from(spread_permille);
    1000 - spread + (phase >> 32) % (2 * spread + 1)
}

/// Frame of a row: the number of whole row periods in `elapsed` (the period is
/// `interval` scaled by `speed_permille`) shifted by `offset`.
pub(super) fn row_frame_index(
    elapsed: Duration,
    interval: Duration,
    speed_permille: u64,
    offset: u64,
    frames: usize,
) -> usize {
    let micros = interval
        .as_micros()
        .saturating_mul(u128::from(speed_permille))
        / 1000;
    let tick =
        u64::try_from(elapsed.as_micros().checked_div(micros).unwrap_or(0)).unwrap_or(u64::MAX);
    frame_index(tick, offset, frames)
}

/// `[ui.status_icons]` together with the frame of the composition being drawn.
#[derive(Debug)]
pub(crate) struct ShellStatusIcons {
    configured: StatusIcons,
    epoch: Instant,
    /// Time since the epoch at the last composition.
    elapsed: Duration,
    /// Whole base periods in `elapsed`; only staleness is judged by it.
    tick: u64,
    has_animation: bool,
    /// Whether a shown endpoint has a row in a state with an animated icon.
    animated_row_present: bool,
}

impl ShellStatusIcons {
    pub(super) fn new(configured: StatusIcons) -> Self {
        Self {
            has_animation: has_animation(&configured),
            configured,
            epoch: Instant::now(),
            elapsed: Duration::ZERO,
            tick: 0,
            animated_row_present: false,
        }
    }

    /// Replaces the configuration, keeping the epoch so that a reload does not
    /// restart the animation.
    pub(super) fn set_configured(&mut self, configured: StatusIcons) {
        self.has_animation = has_animation(&configured);
        self.configured = configured;
        self.animated_row_present = false;
    }

    #[cfg(test)]
    pub(super) fn epoch(&self) -> Instant {
        self.epoch
    }

    #[cfg(test)]
    pub(super) fn configured(&self) -> &StatusIcons {
        &self.configured
    }

    fn interval(&self) -> Duration {
        Duration::from_millis(u64::from(self.configured.frame_interval_ms))
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

    /// The largest spread of any state: it sets the animation clock.
    fn max_spread_permille(&self) -> u16 {
        match self.configured.frame_phase {
            StatusFramePhase::Synced => 0,
            StatusFramePhase::PerRow => self.configured.frame_speed_spread.max_permille(),
        }
    }

    /// The period of the animation clock: with a spread the fastest row has the
    /// period `interval * (1 - spread)` and must not skip a frame.
    fn base_interval(&self) -> Duration {
        self.interval() * (1000 - u32::from(self.max_spread_permille())) / 1000
    }

    fn tick_at(&self, now: Instant) -> u64 {
        frame_tick(
            now.saturating_duration_since(self.epoch),
            self.base_interval(),
        )
    }

    /// Fixes the frame for one composition and records whether a row is in an animated
    /// state. The statuses are the rows' data, so nothing depends on how a frame is laid
    /// out in cells. A row that shows no icon, for lack of a token, of width or of space
    /// on screen, costs a repaint without a visible change; a missed row would stop the
    /// animation. Without animated icons in the configuration no row is read.
    pub(super) fn begin_composition(
        &mut self,
        now: Instant,
        mut statuses: impl Iterator<Item = AgentStatus>,
    ) {
        self.elapsed = now.saturating_duration_since(self.epoch);
        self.tick = self.tick_at(now);
        self.animated_row_present = self.has_animation
            && statuses.any(|status| {
                self.frames_for(status)
                    .is_some_and(StatusIconFrames::is_animated)
            });
    }

    /// WARNING: the frame period is a timer, an exception to the station's ban on
    /// timers. Reason: frames change with time and no event source exists for them.
    /// Removal condition: animated status icons are dropped from the fork.
    ///
    /// The moment the shown frame goes stale, while a row is in an animated state.
    /// `None` keeps the client loop's wake-ups as they are without animation.
    pub(super) fn next_frame_at(&self, now: Instant) -> Option<Instant> {
        if !self.animated_row_present {
            return None;
        }
        let next = u32::try_from(self.tick_at(now).saturating_add(1)).ok()?;
        self.epoch
            .checked_add(self.base_interval().checked_mul(next)?)
    }

    /// Whether `now` belongs to another frame than the last composition drew.
    pub(super) fn frame_stale(&self, now: Instant) -> bool {
        self.animated_row_present && self.tick_at(now) != self.tick
    }

    /// The frame of `status` for a row with the phase seed `phase`; the seed is
    /// ignored unless `frame_phase = "per_row"`.
    pub(super) fn override_for(&self, status: AgentStatus, phase: u64) -> Option<&str> {
        let frames = self.frames_for(status)?;
        let (offset, speed) = match self.configured.frame_phase {
            StatusFramePhase::Synced => (0, 1000),
            StatusFramePhase::PerRow => (
                phase,
                row_speed_permille(phase, self.spread_permille(status)),
            ),
        };
        frames
            .frames()
            .get(row_frame_index(
                self.elapsed,
                self.interval(),
                speed,
                offset,
                frames.frames().len(),
            ))
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

    fn moon() -> StatusIconFrames {
        StatusIconFrames::new(
            ["🌑", "🌒", "🌓", "🌔"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
        )
        .unwrap()
    }

    fn animated_icons() -> ShellStatusIcons {
        ShellStatusIcons::new(StatusIcons {
            working: Some(moon()),
            idle: Some("z".into()),
            ..StatusIcons::default()
        })
    }

    #[test]
    fn frame_tick_counts_whole_periods() {
        let interval = Duration::from_millis(400);
        assert_eq!(frame_tick(Duration::ZERO, interval), 0);
        assert_eq!(frame_tick(Duration::from_millis(399), interval), 0);
        assert_eq!(frame_tick(Duration::from_millis(400), interval), 1);
        assert_eq!(frame_tick(Duration::from_millis(1200), interval), 3);
    }

    #[test]
    fn frame_tick_survives_zero_period_and_overflow() {
        assert_eq!(frame_tick(Duration::MAX, Duration::ZERO), 0);
        assert_eq!(
            frame_tick(Duration::MAX, Duration::from_millis(1)),
            u64::MAX
        );
    }

    #[test]
    fn frame_index_wraps_and_handles_degenerate_lists() {
        assert_eq!(frame_index(0, 0, 8), 0);
        assert_eq!(frame_index(7, 0, 8), 7);
        assert_eq!(frame_index(8, 0, 8), 0);
        assert_eq!(frame_index(u64::MAX, 0, 8), 7);
        assert_eq!(frame_index(u64::MAX, 0, 1), 0);
        assert_eq!(frame_index(u64::MAX, 0, 0), 0);
    }

    #[test]
    fn frame_index_applies_offset_without_overflow() {
        assert_eq!(frame_index(0, 3, 8), 3);
        assert_eq!(frame_index(6, 3, 8), 1);
        assert_eq!(frame_index(0, 11, 8), 3);
        assert_eq!(frame_index(u64::MAX, u64::MAX, 8), 6);
        assert_eq!(frame_index(5, u64::MAX, 1), 0);
        assert_eq!(frame_index(5, 5, 0), 0);
    }

    #[test]
    fn row_phase_is_deterministic_and_separates_endpoints() {
        // Fixed values: the offset must not change between client restarts.
        assert_eq!(row_phase("", "w4"), row_phase("", "w4"));
        assert_ne!(row_phase("", "w4"), row_phase("", "w5"));
        assert_ne!(row_phase("", "w4"), row_phase("ssh-a", "w4"));
        assert_ne!(row_phase("ssh-a", "w4"), row_phase("ssh-b", "w4"));
    }

    #[test]
    fn deadline_exists_only_while_a_row_is_in_an_animated_state() {
        let mut icons = animated_icons();
        let start = icons.epoch;
        assert_eq!(icons.next_frame_at(start), None);
        assert!(!icons.frame_stale(start + Duration::from_secs(10)));

        // A static override and a status without an override do not animate.
        icons.begin_composition(start, [AgentStatus::Idle, AgentStatus::Done].into_iter());
        assert_eq!(icons.next_frame_at(start), None);
        assert!(!icons.frame_stale(start + Duration::from_secs(10)));

        icons.begin_composition(start, [AgentStatus::Idle, AgentStatus::Working].into_iter());
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
            [AgentStatus::Idle].into_iter(),
        );
        assert_eq!(icons.next_frame_at(start), None);
    }

    #[test]
    fn frame_is_stale_only_after_the_period_ends() {
        let mut icons = animated_icons();
        let start = icons.epoch;
        icons.begin_composition(start, [AgentStatus::Working].into_iter());
        assert_eq!(icons.override_for(AgentStatus::Working, 0), Some("🌑"));
        assert!(!icons.frame_stale(start + Duration::from_millis(399)));
        assert!(icons.frame_stale(start + Duration::from_millis(400)));

        icons.begin_composition(
            start + Duration::from_millis(400),
            [AgentStatus::Working].into_iter(),
        );
        assert_eq!(icons.override_for(AgentStatus::Working, 0), Some("🌒"));
        assert!(!icons.frame_stale(start + Duration::from_millis(799)));
    }

    #[test]
    fn single_frame_lists_never_arm_the_deadline() {
        let mut icons = ShellStatusIcons::new(StatusIcons {
            working: Some("x".into()),
            ..StatusIcons::default()
        });
        let start = icons.epoch;
        icons.begin_composition(start, [AgentStatus::Working].into_iter());
        assert_eq!(icons.override_for(AgentStatus::Working, 0), Some("x"));
        assert_eq!(icons.next_frame_at(start), None);
    }

    #[test]
    fn reload_keeps_the_epoch_and_drops_visibility() {
        let mut icons = animated_icons();
        let start = icons.epoch;
        icons.begin_composition(start, [AgentStatus::Working].into_iter());
        assert!(icons.animated_row_present);
        icons.set_configured(StatusIcons::default());
        assert_eq!(icons.epoch, start);
        assert!(!icons.animated_row_present);
        assert_eq!(icons.override_for(AgentStatus::Working, 0), None);
    }

    #[test]
    fn row_speed_is_deterministic_and_within_the_spread() {
        assert_eq!(row_speed_permille(u64::MAX, 0), 1000);
        for spread in [0, 1, 250, 500] {
            let mut distinct = std::collections::BTreeSet::new();
            for id in 0..200 {
                let phase = row_phase("", &format!("w{id}:p1"));
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
    fn row_frame_index_scales_the_period() {
        let interval = Duration::from_millis(400);
        let at = |ms, speed, offset| {
            row_frame_index(Duration::from_millis(ms), interval, speed, offset, 4)
        };
        assert_eq!(at(0, 1000, 0), 0);
        assert_eq!(at(400, 1000, 0), 1);
        // A faster row (period 200 ms) and a slower one (period 600 ms).
        assert_eq!(at(400, 500, 0), 2);
        assert_eq!(at(400, 1500, 0), 0);
        assert_eq!(at(600, 1500, 2), 3);
        assert_eq!(at(400, 1000, 3), 0);
        // A degenerate period stays total.
        assert_eq!(at(400, 0, 1), 1);
        assert_eq!(
            row_frame_index(Duration::MAX, interval, 1000, u64::MAX, 1),
            0
        );
    }

    #[test]
    fn per_row_offsets_and_spreads_follow_the_configuration() {
        let mut icons = StatusIcons {
            working: Some(moon()),
            frame_phase: StatusFramePhase::PerRow,
            ..StatusIcons::default()
        };
        let mut shown = ShellStatusIcons::new(icons.clone());
        shown.begin_composition(shown.epoch, [AgentStatus::Working].into_iter());
        let seen: std::collections::BTreeSet<_> = (0..16)
            .map(|id| shown.override_for(AgentStatus::Working, row_phase("", &format!("w{id}"))))
            .collect();
        assert!(seen.len() > 1);
        icons.frame_phase = StatusFramePhase::Synced;
        let mut shown = ShellStatusIcons::new(icons);
        shown.begin_composition(shown.epoch, [AgentStatus::Working].into_iter());
        assert_eq!(shown.override_for(AgentStatus::Working, 12345), Some("🌑"));
    }
}
