use super::*;
use crate::terminal_theme::{DefaultColorKind, RgbColor};
use ratatui::style::Color;

const RED: Color = Color::Rgb(255, 0, 0);

/// Workspace `ws_1` with color token red, label facts in two tabs, and one
/// unfocused agent in the first tab.
fn two_tab_snapshot(second_active: bool) -> ClientShellSnapshot {
    let mut projected = snapshot();
    let workspace = &mut projected.workspaces[0];
    workspace.tokens = vec![
        ("symbol".into(), "@".into()),
        ("color".into(), "#ff0000".into()),
    ];
    projected.tabs[0].place = Some("herdr".into());
    projected.tabs[0].foreground_process = Some("nvim".into());
    let mut second = projected.tabs[0].clone();
    second.tab_id = "tab_2".into();
    second.number = 2;
    second.label = "2".into();
    second.focused = second_active;
    second.place = Some("notes".into());
    second.foreground_process = Some("htop".into());
    projected.tabs[0].focused = !second_active;
    projected.tabs.push(second);
    if second_active {
        projected.workspaces[0].active_tab_id = "tab_2".into();
        projected.focused_tab_id = Some("tab_2".into());
    }
    projected.agents.push(ClientShellAgent {
        pane_id: "pane_1".into(),
        workspace_id: "ws_1".into(),
        tab_id: "tab_1".into(),
        name: None,
        display_agent: Some("opus".into()),
        agent: Some("claude".into()),
        title: None,
        terminal_title: None,
        terminal_title_stripped: None,
        agent_status: AgentStatus::Working,
        state_change_seq: 1,
        state_labels: Vec::new(),
        tokens: Vec::new(),
        focused: false,
    });
    projected
}

fn tab_style_config(spaces: &str) -> Config {
    toml::from_str(&format!(
        r#"
[ui.sidebar.agents]
rows = [["state_icon", {{ token = "tab", fg = "$panel", bg = "$color", rules = [{{ active = true, fg = "$color", bg = "$panel" }}] }}, "agent"]]
[ui.sidebar.spaces]
rows = {spaces}
"#
    ))
    .expect("config")
}

fn compose_buffer(
    config: &Config,
    projected: ClientShellSnapshot,
) -> (ClientShellState, ratatui::buffer::Buffer) {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(config));
    state.set_snapshot(Box::new(projected));
    state.set_pane_surface(surface());
    let buffer = state
        .compose(106, 24)
        .expect("composed frame")
        .to_ratatui_buffer()
        .expect("buffer");
    (state, buffer)
}

fn row_text(buffer: &ratatui::buffer::Buffer, rect: Rect) -> String {
    (rect.x..rect.right())
        .map(|x| buffer[(x, rect.y)].symbol())
        .collect()
}

/// Column of the first cell of `needle` in the first row of `rect`.
fn column_of(buffer: &ratatui::buffer::Buffer, rect: Rect, needle: &str) -> u16 {
    let text = row_text(buffer, rect);
    let byte = text
        .find(needle)
        .unwrap_or_else(|| panic!("missing {needle} in {text:?}"));
    rect.x + text[..byte].chars().count() as u16
}

const PLAIN_SPACES: &str = r#"[["state_icon", "workspace"]]"#;

#[test]
fn active_agent_tab_keeps_panel_background_and_takes_workspace_color() {
    let config = tab_style_config(PLAIN_SPACES);
    let (state, buffer) = compose_buffer(&config, two_tab_snapshot(false));
    let rect = state.hits.agents[0].0;
    let tab = column_of(&buffer, rect, "@ herdr");
    assert_eq!(buffer[(tab, rect.y)].fg, RED);
    for x in rect.x..rect.right() {
        assert_eq!(
            buffer[(x, rect.y)].bg,
            state.config.palette.sidebar_bg,
            "{x}"
        );
    }
}

#[test]
fn inactive_agent_tab_fills_the_row_with_workspace_color_and_panel_text() {
    let config = tab_style_config(PLAIN_SPACES);
    let (state, buffer) = compose_buffer(&config, two_tab_snapshot(true));
    let rect = state.hits.agents[0].0;
    let tab = column_of(&buffer, rect, "@ herdr");
    assert_eq!(buffer[(tab, rect.y)].fg, state.config.palette.panel_bg);
    // The fill covers the whole row width: indent, separators and the tail.
    let separator = column_of(&buffer, rect, " · opus");
    for x in rect.x..rect.right() {
        assert_eq!(buffer[(x, rect.y)].bg, RED, "{x}");
    }
    assert_eq!(
        buffer[(separator + 1, rect.y)].fg,
        state.config.palette.overlay0
    );
}

#[test]
fn panel_text_follows_host_background_when_theme_keeps_terminal_background() {
    let config = tab_style_config(PLAIN_SPACES);
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&config));
    state.set_snapshot(Box::new(two_tab_snapshot(true)));
    state.set_pane_surface(surface());
    state.handle_raw_events(vec![RawInputEvent::HostDefaultColor {
        kind: DefaultColorKind::Background,
        color: RgbColor { r: 1, g: 2, b: 3 },
    }]);
    let buffer = state
        .compose(106, 24)
        .expect("composed frame")
        .to_ratatui_buffer()
        .expect("buffer");
    let rect = state.hits.agents[0].0;
    let tab = column_of(&buffer, rect, "@ herdr");
    assert_eq!(buffer[(tab, rect.y)].fg, Color::Rgb(1, 2, 3));
}

#[test]
fn filled_space_row_spans_the_block_and_yields_to_the_navigation_cursor() {
    let config = tab_style_config(
        r#"[["state_icon", { token = "workspace", fg = "$panel", bg = "$color" }, "branch"]]"#,
    );
    let mut projected = two_tab_snapshot(false);
    projected.workspaces[0].focused = false;
    let (mut state, buffer) = compose_buffer(&config, projected);
    let rect = state.hits.workspaces[0].rect;
    for x in rect.x..rect.right() {
        assert_eq!(buffer[(x, rect.y)].bg, RED, "{x}");
    }
    let label = column_of(&buffer, rect, "@ herdr");
    assert_eq!(buffer[(label, rect.y)].fg, state.config.palette.panel_bg);

    // Navigation cursor: its background wins and `$panel` text takes the token
    // background, so the label stays readable.
    state.handle_input_bytes(&[0x02]);
    state.handle_input_bytes(b"w");
    assert_eq!(state.mode, ClientShellMode::Navigate);
    let buffer = state
        .compose(106, 24)
        .expect("composed frame")
        .to_ratatui_buffer()
        .expect("buffer");
    let rect = state.hits.workspaces[0].rect;
    let cursor_bg = state.config.palette.selection_bg;
    for x in rect.x..rect.right() {
        assert_eq!(buffer[(x, rect.y)].bg, cursor_bg, "{x}");
    }
    let label = column_of(&buffer, rect, "@ herdr");
    assert_eq!(buffer[(label, rect.y)].fg, RED);
}

#[test]
fn spaces_header_and_active_tab_row_follow_the_active_rule() {
    let config = tab_style_config(
        r#"[["state_icon", { token = "workspace", fg = "$panel", bg = "$color", rules = [{ active = true, fg = "$color", bg = "$panel" }] }]]"#,
    );
    let unfocused = |second_active| {
        let mut projected = two_tab_snapshot(second_active);
        projected.workspaces[0].focused = false;
        projected
    };
    // First tab active: the header row has the panel background and red text.
    let (state, buffer) = compose_buffer(&config, unfocused(false));
    let rect = state.hits.workspaces[0].rect;
    assert_eq!(rect.height, 1);
    let label = column_of(&buffer, rect, "@ herdr");
    assert_eq!(buffer[(label, rect.y)].fg, RED);
    assert_eq!(buffer[(rect.x, rect.y)].bg, state.config.palette.sidebar_bg);

    // Later tab active: the header (first tab) is inactive and inverted, the
    // tab row below it is active.
    let (state, buffer) = compose_buffer(&config, unfocused(true));
    let rect = state.hits.workspaces[0].rect;
    assert_eq!(rect.height, 2);
    for x in rect.x..rect.right() {
        assert_eq!(buffer[(x, rect.y)].bg, RED, "{x}");
    }
    let label = column_of(&buffer, rect, "@ herdr");
    assert_eq!(buffer[(label, rect.y)].fg, state.config.palette.panel_bg);
    let tab_row = Rect::new(rect.x, rect.y + 1, rect.width, 1);
    let tab = column_of(&buffer, tab_row, "@ notes");
    assert_eq!(buffer[(tab, tab_row.y)].fg, RED);
    assert_eq!(
        buffer[(tab_row.x, tab_row.y)].bg,
        state.config.palette.sidebar_bg
    );
}

const FALLBACK_AGENTS: &str = r##"[ui.sidebar.agents]
rows = [["state_icon", { token = "tab", fg = "$panel", bg = "$color|#ffffff", rules = [{ active = true, fg = "$color|#ffffff", bg = "$panel" }] }, "agent"]]
[ui.sidebar.spaces]
rows = [["state_icon", { token = "workspace", fg = "$color|#ffffff" }]]
"##;

const WHITE: Color = Color::Rgb(255, 255, 255);

fn without_color_token(mut projected: ClientShellSnapshot) -> ClientShellSnapshot {
    projected.workspaces[0]
        .tokens
        .retain(|(name, _)| name != "color");
    projected
}

#[test]
fn fallback_color_keeps_labels_visible_without_color_token() {
    let config: Config = toml::from_str(FALLBACK_AGENTS).expect("config");

    // Active tab: panel background, white text.
    let (state, buffer) = compose_buffer(&config, without_color_token(two_tab_snapshot(false)));
    let rect = state.hits.agents[0].0;
    let tab = column_of(&buffer, rect, "@ herdr");
    assert_eq!(buffer[(tab, rect.y)].fg, WHITE);
    assert_eq!(buffer[(rect.x, rect.y)].bg, state.config.palette.sidebar_bg);
    let space = state.hits.workspaces[0].rect;
    let label = column_of(&buffer, space, "@ herdr");
    assert_eq!(buffer[(label, space.y)].fg, WHITE);

    // Inactive tab: white fill with panel text.
    let (state, buffer) = compose_buffer(&config, without_color_token(two_tab_snapshot(true)));
    let rect = state.hits.agents[0].0;
    let tab = column_of(&buffer, rect, "@ herdr");
    assert_eq!(buffer[(tab, rect.y)].fg, state.config.palette.panel_bg);
    for x in rect.x..rect.right() {
        assert_eq!(buffer[(x, rect.y)].bg, WHITE, "{x}");
    }
}

#[test]
fn fallback_color_yields_to_the_color_token() {
    let config: Config = toml::from_str(FALLBACK_AGENTS).expect("config");
    let (state, buffer) = compose_buffer(&config, two_tab_snapshot(false));
    let rect = state.hits.agents[0].0;
    let tab = column_of(&buffer, rect, "@ herdr");
    assert_eq!(buffer[(tab, rect.y)].fg, RED);
    let space = state.hits.workspaces[0].rect;
    let label = column_of(&buffer, space, "@ herdr");
    assert_eq!(buffer[(label, space.y)].fg, RED);

    let (state, buffer) = compose_buffer(&config, two_tab_snapshot(true));
    let rect = state.hits.agents[0].0;
    for x in rect.x..rect.right() {
        assert_eq!(buffer[(x, rect.y)].bg, RED, "{x}");
    }
}

const TAB_BAR_COLORS: &str = r##"
[ui.tab_bar]
active = { fg = "$color|#ffffff", bg = "$panel" }
inactive = { fg = "$panel", bg = "$color|#ffffff" }
"##;

/// Foreground and background of the first cell of the tab-strip tab `index`.
fn tab_cell(
    state: &ClientShellState,
    buffer: &ratatui::buffer::Buffer,
    index: usize,
) -> (Color, Color, Rect) {
    let rect = state.hits.tabs[index].0;
    // The label is centered: the first column is padding with the tab style.
    let cell = &buffer[(rect.x, rect.y)];
    (cell.fg, cell.bg, rect)
}

#[test]
fn tab_strip_keeps_theme_colors_without_tab_bar_config() {
    let config: Config = toml::from_str("").expect("config");
    let (state, buffer) = compose_buffer(&config, two_tab_snapshot(false));
    let palette = &state.config.palette;
    let (fg, bg, _) = tab_cell(&state, &buffer, 0);
    assert_eq!((fg, bg), (panel_contrast_fg(palette), palette.accent));
    let (fg, bg, _) = tab_cell(&state, &buffer, 1);
    assert_eq!((fg, bg), (palette.overlay0, palette.surface0));
}

#[test]
fn tab_strip_takes_workspace_color_by_focus() {
    let config: Config = toml::from_str(TAB_BAR_COLORS).expect("config");
    let (state, buffer) = compose_buffer(&config, two_tab_snapshot(false));
    let panel = state.config.palette.panel_bg;
    let (fg, bg, rect) = tab_cell(&state, &buffer, 0);
    assert_eq!((fg, bg), (RED, panel));
    for x in rect.x..rect.right() {
        assert_eq!(buffer[(x, rect.y)].bg, panel, "{x}");
    }
    let (fg, bg, rect) = tab_cell(&state, &buffer, 1);
    assert_eq!((fg, bg), (panel, RED));
    for x in rect.x..rect.right() {
        assert_eq!(buffer[(x, rect.y)].bg, RED, "{x}");
    }
    // Focus moves: the colors follow.
    let (state, buffer) = compose_buffer(&config, two_tab_snapshot(true));
    assert_eq!(tab_cell(&state, &buffer, 0).0, panel);
    assert_eq!(tab_cell(&state, &buffer, 0).1, RED);
    assert_eq!(tab_cell(&state, &buffer, 1).1, panel);
}

#[test]
fn tab_strip_terminal_background_resets_the_active_tab_over_the_panel_fill() {
    let config: Config = toml::from_str(
        r##"
[ui.tab_bar]
active = { fg = "$color|#ffffff", bg = "$terminal" }
inactive = { fg = "$panel", bg = "$color|#ffffff" }
"##,
    )
    .expect("config");
    let (state, buffer) = compose_buffer(&config, two_tab_snapshot(false));
    let panel = state.config.palette.panel_bg;
    let (fg, bg, rect) = tab_cell(&state, &buffer, 0);
    assert_eq!((fg, bg), (RED, Color::Reset));
    for x in rect.x..rect.right() {
        assert_eq!(buffer[(x, rect.y)].bg, Color::Reset, "{x}");
    }
    // The strip outside the active tab keeps the panel fill.
    assert_eq!(buffer[(rect.right(), rect.y)].bg, panel);
    let (_, bg, _) = tab_cell(&state, &buffer, 1);
    assert_eq!(bg, RED);
}

#[test]
fn tab_strip_uses_fallback_color_without_workspace_color_token() {
    let config: Config = toml::from_str(TAB_BAR_COLORS).expect("config");
    let mut projected = two_tab_snapshot(false);
    projected.workspaces[0].tokens = vec![("symbol".into(), "@".into())];
    let (state, buffer) = compose_buffer(&config, projected);
    let white = Color::Rgb(255, 255, 255);
    assert_eq!(tab_cell(&state, &buffer, 0).0, white);
    assert_eq!(tab_cell(&state, &buffer, 1).1, white);
}

#[test]
fn tab_strip_unset_fields_keep_theme_colors_and_modifiers_apply() {
    let config: Config = toml::from_str(
        r##"
[ui.tab_bar]
inactive = { bg = "#102030", bold = true }
"##,
    )
    .expect("config");
    let (state, buffer) = compose_buffer(&config, two_tab_snapshot(false));
    let palette = &state.config.palette;
    let (fg, bg, rect) = tab_cell(&state, &buffer, 1);
    assert_eq!((fg, bg), (palette.overlay0, Color::Rgb(16, 32, 48)));
    assert!(buffer[(rect.x, rect.y)]
        .modifier
        .contains(ratatui::style::Modifier::BOLD));
    let (fg, bg, _) = tab_cell(&state, &buffer, 0);
    assert_eq!((fg, bg), (panel_contrast_fg(palette), palette.accent));
}
