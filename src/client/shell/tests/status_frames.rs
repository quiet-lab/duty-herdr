use super::*;
use crate::client::endpoint::{
    ClientEndpointId, ClientEndpointStatus, ProfileId, SavedSshEndpoint,
};
use crate::config::{AgentSidebarToken, StatusIconFrames};
use std::time::{Duration, Instant};

const MOON: [&str; 4] = ["🌑", "🌒", "🌓", "🌔"];
const NARROW: [&str; 4] = ["◐", "◓", "◑", "◒"];
const INTERVAL: Duration = Duration::from_millis(400);

fn moon_config() -> Config {
    frames_config(MOON)
}

fn frames_config(frames: [&str; 4]) -> Config {
    let mut config = Config::default();
    config.ui.status_icons.working = Some(
        StatusIconFrames::new(frames.into_iter().map(str::to_owned).collect())
            .expect("status frames"),
    );
    config.ui.status_icons.frame_interval_ms = 400;
    config.ui.sidebar.agents.rows =
        vec![vec![AgentSidebarToken::StateIcon, AgentSidebarToken::Agent]];
    config
}

fn working_agent(pane_id: &str, name: &str) -> ClientShellAgent {
    ClientShellAgent {
        pane_id: pane_id.into(),
        workspace_id: "ws_1".into(),
        tab_id: "tab_1".into(),
        name: Some(name.into()),
        display_agent: None,
        agent: Some("pi".into()),
        title: None,
        terminal_title: None,
        terminal_title_stripped: None,
        agent_status: AgentStatus::Working,
        state_change_seq: 1,
        state_labels: Vec::new(),
        tokens: Vec::new(),
        focused: true,
    }
}

fn working_snapshot() -> ClientShellSnapshot {
    let mut projected = snapshot();
    projected.workspaces[0].agent_status = AgentStatus::Working;
    projected.tabs[0].agent_status = AgentStatus::Working;
    projected.agents = vec![
        working_agent("pane_1", "alpha"),
        working_agent("pane_2", "beta"),
    ];
    projected
}

fn moon_state(projected: ClientShellSnapshot) -> ClientShellState {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&moon_config()));
    state.set_snapshot(Box::new(projected));
    state.set_pane_surface(surface());
    state
}

fn frame_at(state: &ClientShellState, tick: u32) -> Instant {
    state.config.status_icons.epoch() + INTERVAL * tick
}

/// Counts the frame symbols of one composition, in the order of `frames`.
fn frame_counts(
    state: &mut ClientShellState,
    frames: [&str; 4],
    (cols, rows): (u16, u16),
    tick: u32,
) -> [usize; 4] {
    let now = frame_at(state, tick);
    let frame = state.compose_at(cols, rows, now).expect("frame");
    frames.map(|symbol| {
        frame
            .cells
            .iter()
            .filter(|cell| cell.symbol == symbol)
            .count()
    })
}

/// Every drawn icon of a composition uses the frame of its tick, and the number of
/// drawn icons does not depend on the frame.
fn assert_one_frame_per_tick(state: &mut ClientShellState, cols: u16, rows: u16, minimum: usize) {
    assert_frames_per_tick(state, MOON, (cols, rows), minimum);
}

fn assert_frames_per_tick(
    state: &mut ClientShellState,
    frames: [&str; 4],
    size: (u16, u16),
    minimum: usize,
) {
    let mut drawn = None;
    for tick in 0..9 {
        let counts = frame_counts(state, frames, size, tick);
        let shown = tick as usize % frames.len();
        for (index, count) in counts.iter().enumerate() {
            if index != shown {
                assert_eq!(*count, 0, "tick {tick} drew {}: {counts:?}", frames[index]);
            }
        }
        assert!(
            counts[shown] >= minimum,
            "tick {tick}: {counts:?}, at least {minimum} icons expected"
        );
        assert_eq!(*drawn.get_or_insert(counts[shown]), counts[shown]);
    }
}

fn remote_profile() -> SavedSshEndpoint {
    SavedSshEndpoint {
        id: ProfileId::parse("0123456789abcdef0123456789abcdef").expect("profile id"),
        label: "Build".into(),
        target: "dev@build.example".into(),
        session: "agents".into(),
        enabled: true,
    }
}

fn with_remote(state: &mut ClientShellState) {
    let profile = remote_profile();
    let endpoint_id = ClientEndpointId::Ssh(profile.id.clone());
    state.set_endpoint_catalog(&[profile]);
    state.set_endpoint_status(&endpoint_id, ClientEndpointStatus::Online);
    let mut remote = working_snapshot();
    remote.boot_id = "remote-boot".into();
    state.set_endpoint_snapshot(&endpoint_id, Box::new(remote));
}

#[test]
fn expanded_sidebar_rows_share_one_frame() {
    let mut state = moon_state(working_snapshot());
    // A workspace row and two agent rows.
    assert_one_frame_per_tick(&mut state, 106, 30, 3);
}

#[test]
fn collapsed_sidebar_rows_share_one_frame() {
    // The compact sidebar leaves one cell for the icon, so its frames are narrow.
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&frames_config(NARROW)));
    state.set_snapshot(Box::new(working_snapshot()));
    state.set_pane_surface(surface());
    state.sidebar_collapsed = true;
    assert_frames_per_tick(&mut state, NARROW, (106, 30), 1);
}

#[test]
fn mobile_header_and_switcher_share_one_frame() {
    let mut state = moon_state(working_snapshot());
    assert_one_frame_per_tick(&mut state, 44, 30, 1);
    state.mode = ClientShellMode::Navigate;
    assert_one_frame_per_tick(&mut state, 44, 30, 2);
}

#[test]
fn endpoint_sidebar_rows_share_one_frame() {
    let mut state = moon_state(working_snapshot());
    with_remote(&mut state);
    assert_one_frame_per_tick(&mut state, 106, 30, 4);
}

#[test]
fn collapsed_endpoint_sidebar_rows_share_one_frame() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&frames_config(NARROW)));
    state.set_snapshot(Box::new(working_snapshot()));
    state.set_pane_surface(surface());
    with_remote(&mut state);
    state.sidebar_collapsed = true;
    assert_frames_per_tick(&mut state, NARROW, (106, 30), 1);
}

#[test]
fn navigator_overlay_shares_one_frame() {
    let mut state = moon_state(working_snapshot());
    state.open_navigator_overlay();
    assert_one_frame_per_tick(&mut state, 106, 30, 1);
}

#[test]
fn frame_changes_on_the_period_boundary_and_not_inside_it() {
    let mut state = moon_state(working_snapshot());
    let epoch = state.config.status_icons.epoch();
    let shown = |state: &mut ClientShellState, offset_ms: u64| {
        let now = epoch + Duration::from_millis(offset_ms);
        let frame = state.compose_at(106, 30, now).expect("frame");
        MOON.iter()
            .position(|moon| frame.cells.iter().any(|cell| cell.symbol == *moon))
            .expect("a moon frame")
    };
    assert_eq!(shown(&mut state, 0), 0);
    assert_eq!(shown(&mut state, 399), 0);
    assert_eq!(shown(&mut state, 400), 1);
    assert_eq!(shown(&mut state, 1599), 3);
    assert_eq!(shown(&mut state, 1600), 0);
}

#[test]
fn visible_animated_icon_arms_the_timer_and_repaint() {
    let mut state = moon_state(working_snapshot());
    let epoch = state.config.status_icons.epoch();
    state.compose_at(106, 30, epoch).expect("frame");

    let now = epoch + Duration::from_millis(350);
    assert_eq!(state.timer_delay(now), Duration::from_millis(50));
    assert!(!state.tick_status_frame(now, false));
    assert!(state.tick_status_frame(epoch + INTERVAL, false));

    state.compose_at(106, 30, epoch + INTERVAL).expect("frame");
    assert!(!state.tick_status_frame(epoch + INTERVAL, false));
    assert!(state.tick_status_frame(epoch + INTERVAL * 2, false));
}

#[test]
fn no_visible_animated_icon_leaves_the_timer_alone() {
    let idle_default = Duration::from_millis(100);

    // Frames are configured, but no row is in the animated state.
    let mut state = moon_state(snapshot());
    let epoch = state.config.status_icons.epoch();
    state.compose_at(106, 30, epoch).expect("frame");
    assert_eq!(
        state.timer_delay(epoch + Duration::from_millis(350)),
        idle_default
    );
    assert!(!state.tick_status_frame(epoch + INTERVAL * 5, false));

    // A row is in the state, but its icon is static.
    let mut config = moon_config();
    config.ui.status_icons.working = Some("x".into());
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&config));
    state.set_snapshot(Box::new(working_snapshot()));
    state.set_pane_surface(surface());
    let epoch = state.config.status_icons.epoch();
    state.compose_at(106, 30, epoch).expect("frame");
    assert_eq!(
        state.timer_delay(epoch + Duration::from_millis(350)),
        idle_default
    );
    assert!(!state.tick_status_frame(epoch + INTERVAL * 5, false));

    // Nothing was composed yet.
    let state = ClientShellState::new(ClientShellConfig::from_config(&moon_config()));
    let epoch = state.config.status_icons.epoch();
    assert_eq!(
        state.timer_delay(epoch + Duration::from_millis(350)),
        idle_default
    );
    assert!(!state.tick_status_frame(epoch + INTERVAL * 5, false));
}

#[test]
fn animated_state_leaving_the_view_disarms_the_timer() {
    let mut state = moon_state(working_snapshot());
    let epoch = state.config.status_icons.epoch();
    state.compose_at(106, 30, epoch).expect("frame");
    assert!(state.tick_status_frame(epoch + INTERVAL, false));

    let mut idle = snapshot();
    idle.revision = 1;
    state.set_snapshot(Box::new(idle));
    state.compose_at(106, 30, epoch + INTERVAL).expect("frame");
    assert!(!state.tick_status_frame(epoch + INTERVAL * 2, false));
}

#[test]
fn wide_frames_cut_off_by_the_collapsed_sidebar_do_not_arm_the_timer() {
    let mut state = moon_state(working_snapshot());
    state.sidebar_collapsed = true;
    let epoch = state.config.status_icons.epoch();
    let frame = state.compose_at(106, 30, epoch).expect("frame");
    assert!(frame
        .cells
        .iter()
        .all(|cell| !MOON.contains(&cell.symbol.as_str())));
    assert!(!state.tick_status_frame(epoch + INTERVAL, false));
    assert_eq!(state.timer_delay(epoch), Duration::from_millis(100));
}

#[test]
fn rows_without_an_icon_token_do_not_arm_the_timer() {
    let mut config = moon_config();
    config.ui.sidebar.agents.rows = vec![vec![AgentSidebarToken::Agent]];
    config.ui.sidebar.spaces.rows = vec![vec![crate::config::SpaceSidebarToken::Workspace]];
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&config));
    state.set_snapshot(Box::new(working_snapshot()));
    state.set_pane_surface(surface());
    let epoch = state.config.status_icons.epoch();
    let frame = state.compose_at(106, 30, epoch).expect("frame");
    let text = frame_rows(&frame).join("\n");
    assert!(
        frame
            .cells
            .iter()
            .all(|cell| !MOON.contains(&cell.symbol.as_str())),
        "{text}"
    );
    assert!(!state.tick_status_frame(epoch + INTERVAL, false));
}

#[test]
fn frozen_presentation_and_incoherent_surface_hold_the_repaint() {
    let mut state = moon_state(working_snapshot());
    let epoch = state.config.status_icons.epoch();
    state.compose_at(106, 30, epoch).expect("frame");
    let stale = epoch + INTERVAL;
    assert!(state.tick_status_frame(stale, false));
    assert!(!state.tick_status_frame(stale, true));

    let mut newer = working_snapshot();
    newer.revision = 2;
    state.set_snapshot(Box::new(newer));
    assert!(state.compose_at(106, 30, stale).is_none());
    assert!(!state.tick_status_frame(stale, false));
}

/// The icon's width is fixed by the configuration check, so a frame change moves no
/// neighbouring cell: only the icon cells differ between two ticks.
#[test]
fn frame_change_touches_only_icon_cells() {
    let mut state = moon_state(working_snapshot());
    let first = state
        .compose_at(106, 30, frame_at(&state, 0))
        .expect("first frame");
    let second = state
        .compose_at(106, 30, frame_at(&state, 1))
        .expect("second frame");
    let changed = first
        .cells
        .iter()
        .zip(&second.cells)
        .filter(|(left, right)| left.symbol != right.symbol)
        .collect::<Vec<_>>();
    assert!(!changed.is_empty());
    for (left, right) in changed {
        assert_eq!(left.symbol, MOON[0]);
        assert_eq!(right.symbol, MOON[1]);
    }
}

fn scale_snapshot(workspaces: usize) -> ClientShellSnapshot {
    let mut projected = snapshot();
    let workspace = projected.workspaces[0].clone();
    let tab = projected.tabs[0].clone();
    projected.workspaces.clear();
    projected.tabs.clear();
    projected.agents.clear();
    for index in 0..workspaces {
        let workspace_id = format!("ws_{}", index + 1);
        let tab_id = format!("tab_{}", index + 1);
        projected.workspaces.push(ClientShellWorkspace {
            workspace_id: workspace_id.clone(),
            active_tab_id: tab_id.clone(),
            number: index + 1,
            label: format!("space {index}"),
            focused: index == 0,
            agent_status: AgentStatus::Working,
            ..workspace.clone()
        });
        projected.tabs.push(ClientShellTab {
            tab_id: tab_id.clone(),
            workspace_id: workspace_id.clone(),
            focused: index == 0,
            agent_status: AgentStatus::Working,
            ..tab.clone()
        });
        projected.agents.push(ClientShellAgent {
            workspace_id,
            tab_id,
            focused: index == 0,
            ..working_agent(&format!("pane_{}", index + 1), &format!("agent {index}"))
        });
    }
    projected
}

/// Cost of one animation tick: the stale check, the composition over the retained
/// surface and the encoding of the changed cells. Part of `just bench-render-scale`.
#[test]
#[ignore = "manual status frame tick scaling profile"]
fn status_frame_tick_render_scale_profile() {
    const TICKS: u32 = 2000;
    for animated in [true, false] {
        for workspaces in [1, 15, 50] {
            let mut config = moon_config();
            if !animated {
                config.ui.status_icons.working = Some("x".into());
            }
            let mut state = ClientShellState::new(ClientShellConfig::from_config(&config));
            state.set_snapshot(Box::new(scale_snapshot(workspaces)));
            state.set_pane_surface(surface());
            let mut encoder = crate::protocol::render_ansi::BlitEncoder::new();
            let epoch = state.config.status_icons.epoch();
            let start = Instant::now();
            for tick in 0..TICKS {
                let now = epoch + INTERVAL * tick;
                std::hint::black_box(state.tick_status_frame(now, false));
                let composed = state.compose_at(106, 40, now).expect("frame");
                let encoded = encoder.encode(&composed.frame, false);
                std::hint::black_box(&encoded);
                encoder.commit(composed.frame, encoded);
            }
            eprintln!(
                "status frame tick: animated={animated}, {workspaces} working workspaces, \
                 {:.1} us/tick",
                start.elapsed().as_secs_f64() * 1e6 / f64::from(TICKS)
            );
        }
    }
    // The stale check alone runs on every client loop timer.
    let mut state = moon_state(scale_snapshot(50));
    let epoch = state.config.status_icons.epoch();
    state.compose_at(106, 40, epoch).expect("frame");
    let start = Instant::now();
    for tick in 0..1_000_000u32 {
        std::hint::black_box(
            state.tick_status_frame(epoch + Duration::from_micros(tick.into()), false),
        );
    }
    eprintln!(
        "status frame stale check: {:.1} ns",
        start.elapsed().as_secs_f64() * 1e9 / 1e6
    );
}
