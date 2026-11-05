//! Selection of the status icon frame shown by the client.
//!
//! The frame is a pure function of the client's monotonic clock, so every row in
//! one state shows the same frame. It is presentation state: the server, the API
//! and the protocol know nothing about it.

use std::time::{Duration, Instant};

use crate::api::schema::AgentStatus;
use crate::config::{StatusIconFrames, StatusIcons};

/// Number of whole frame periods in `elapsed`. A zero period stays on tick 0;
/// the configuration rejects it, the guard only keeps the function total.
pub(super) fn frame_tick(elapsed: Duration, interval: Duration) -> u64 {
    let interval = interval.as_millis();
    if interval == 0 {
        return 0;
    }
    u64::try_from(elapsed.as_millis() / interval).unwrap_or(u64::MAX)
}

/// Index of the frame shown at `tick`. Lists of different length stay on the
/// same tick and wrap independently.
pub(super) fn frame_index(tick: u64, frames: usize) -> usize {
    match u64::try_from(frames) {
        Ok(0) | Err(_) => 0,
        Ok(frames) => usize::try_from(tick % frames).unwrap_or(0),
    }
}

/// `[ui.status_icons]` together with the frame of the composition being drawn.
#[derive(Debug)]
pub(crate) struct ShellStatusIcons {
    configured: StatusIcons,
    epoch: Instant,
    tick: u64,
    /// Whether the last composed frame shows an animated icon.
    animated_visible: bool,
}

impl ShellStatusIcons {
    pub(super) fn new(configured: StatusIcons) -> Self {
        Self {
            configured,
            epoch: Instant::now(),
            tick: 0,
            animated_visible: false,
        }
    }

    /// Replaces the configuration, keeping the epoch so that a reload does not
    /// restart the animation.
    pub(super) fn set_configured(&mut self, configured: StatusIcons) {
        self.configured = configured;
        self.animated_visible = false;
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

    fn tick_at(&self, now: Instant) -> u64 {
        frame_tick(now.saturating_duration_since(self.epoch), self.interval())
    }

    /// Fixes the frame for one composition.
    pub(super) fn begin_composition(&mut self, now: Instant) {
        self.tick = self.tick_at(now);
    }

    /// Records whether the composed cells show the current frame of an animated
    /// icon. Reading the result of the composition covers every reason for an icon
    /// to be absent: no icon token in a row, a cell too narrow, an opaque overlay.
    /// A pane printing the same symbol only costs a repaint without a visible change.
    /// Without animated icons in the configuration no cell is read.
    pub(super) fn observe_composition<'a>(&mut self, mut cells: impl Iterator<Item = &'a str>) {
        let current = [
            AgentStatus::Idle,
            AgentStatus::Working,
            AgentStatus::Blocked,
            AgentStatus::Done,
            AgentStatus::Unknown,
        ]
        .map(|status| {
            self.frames_for(status)
                .filter(|frames| frames.is_animated())
                .and_then(|_| self.override_for(status))
        });
        self.animated_visible = current.iter().any(Option::is_some)
            && cells.any(|cell| current.iter().flatten().any(|frame| *frame == cell));
    }

    /// Forgets visibility when the composition draws no status icons.
    pub(super) fn clear_visibility(&mut self) {
        self.animated_visible = false;
    }

    #[cfg(test)]
    fn animated_visible(&self) -> bool {
        self.animated_visible
    }

    /// WARNING: the frame period is a timer, an exception to the station's ban on
    /// timers. Reason: frames change with time and no event source exists for them.
    /// Removal condition: animated status icons are dropped from the fork.
    ///
    /// The moment the shown frame goes stale, while an animated icon is visible.
    /// `None` keeps the client loop's wake-ups as they are without animation.
    pub(super) fn next_frame_at(&self, now: Instant) -> Option<Instant> {
        if !self.animated_visible {
            return None;
        }
        let next = u32::try_from(self.tick_at(now).saturating_add(1)).ok()?;
        self.epoch.checked_add(self.interval().checked_mul(next)?)
    }

    /// Whether `now` belongs to another frame than the last composition drew.
    pub(super) fn frame_stale(&self, now: Instant) -> bool {
        self.animated_visible && self.tick_at(now) != self.tick
    }

    pub(super) fn override_for(&self, status: AgentStatus) -> Option<&str> {
        let frames = self.frames_for(status)?;
        frames
            .frames()
            .get(frame_index(self.tick, frames.frames().len()))
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
        assert_eq!(frame_index(0, 8), 0);
        assert_eq!(frame_index(7, 8), 7);
        assert_eq!(frame_index(8, 8), 0);
        assert_eq!(frame_index(u64::MAX, 8), 7);
        assert_eq!(frame_index(u64::MAX, 1), 0);
        assert_eq!(frame_index(u64::MAX, 0), 0);
    }

    #[test]
    fn deadline_exists_only_while_a_composition_shows_an_animated_icon() {
        let mut icons = animated_icons();
        let start = icons.epoch;
        icons.begin_composition(start);
        assert_eq!(icons.next_frame_at(start), None);
        assert!(!icons.frame_stale(start + Duration::from_secs(10)));

        // Choosing an icon does not make it visible, and neither do other symbols.
        assert_eq!(icons.override_for(AgentStatus::Working), Some("🌑"));
        icons.observe_composition(["z", "a", "🌒"].into_iter());
        assert_eq!(icons.next_frame_at(start), None);

        icons.observe_composition(["z", "🌑"].into_iter());
        assert_eq!(
            icons.next_frame_at(start + Duration::from_millis(100)),
            Some(start + Duration::from_millis(400))
        );
        assert_eq!(
            icons.next_frame_at(start + Duration::from_millis(400)),
            Some(start + Duration::from_millis(800))
        );

        // The next composition without the icon removes the deadline.
        icons.begin_composition(start + Duration::from_millis(400));
        icons.observe_composition(["z"].into_iter());
        assert_eq!(icons.next_frame_at(start), None);
    }

    #[test]
    fn frame_is_stale_only_after_the_period_ends() {
        let mut icons = animated_icons();
        let start = icons.epoch;
        icons.begin_composition(start);
        icons.observe_composition(["🌑"].into_iter());
        assert!(!icons.frame_stale(start + Duration::from_millis(399)));
        assert!(icons.frame_stale(start + Duration::from_millis(400)));

        icons.begin_composition(start + Duration::from_millis(400));
        assert_eq!(icons.override_for(AgentStatus::Working), Some("🌒"));
        icons.observe_composition(["🌒"].into_iter());
        assert!(!icons.frame_stale(start + Duration::from_millis(799)));
    }

    #[test]
    fn single_frame_lists_never_arm_the_deadline() {
        let mut icons = ShellStatusIcons::new(StatusIcons {
            working: Some("x".into()),
            ..StatusIcons::default()
        });
        let start = icons.epoch;
        icons.begin_composition(start);
        assert_eq!(icons.override_for(AgentStatus::Working), Some("x"));
        icons.observe_composition(["x"].into_iter());
        assert_eq!(icons.next_frame_at(start), None);
    }

    #[test]
    fn reload_keeps_the_epoch_and_drops_visibility() {
        let mut icons = animated_icons();
        let start = icons.epoch;
        icons.begin_composition(start);
        icons.observe_composition(["🌑"].into_iter());
        assert!(icons.animated_visible());
        icons.set_configured(StatusIcons::default());
        assert_eq!(icons.epoch, start);
        assert!(!icons.animated_visible());
        assert_eq!(icons.override_for(AgentStatus::Working), None);
    }
}
