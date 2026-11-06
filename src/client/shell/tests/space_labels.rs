use super::*;

/// One workspace with a manual label, symbol `@`, and label facts in its
/// first tab: Git place `herdr`, foreground process `nvim`.
fn labelled_snapshot() -> ClientShellSnapshot {
    let mut projected = snapshot();
    let workspace = &mut projected.workspaces[0];
    workspace.label = "mine".into();
    workspace.custom_label = true;
    workspace.tokens = vec![("symbol".into(), "@".into())];
    let tab = &mut projected.tabs[0];
    tab.place = Some("herdr".into());
    tab.foreground_process = Some("nvim".into());
    projected
}

fn with_second_tab(mut projected: ClientShellSnapshot, active: bool) -> ClientShellSnapshot {
    let mut second = projected.tabs[0].clone();
    second.tab_id = "tab_2".into();
    second.number = 2;
    second.label = "2".into();
    second.custom_label = false;
    second.focused = active;
    second.place = Some("~/notes".into());
    second.foreground_process = Some("htop".into());
    projected.tabs[0].focused = !active;
    projected.tabs.push(second);
    if active {
        projected.workspaces[0].active_tab_id = "tab_2".into();
        projected.focused_tab_id = Some("tab_2".into());
    }
    projected
}

fn agent(tab_id: &str, display_agent: &str) -> ClientShellAgent {
    ClientShellAgent {
        pane_id: "pane_1".into(),
        workspace_id: "ws_1".into(),
        tab_id: tab_id.into(),
        name: None,
        display_agent: Some(display_agent.into()),
        agent: Some("claude".into()),
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

fn compose_text(
    config: &Config,
    projected: ClientShellSnapshot,
) -> (ClientShellState, Vec<String>) {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(config));
    state.set_snapshot(Box::new(projected));
    state.set_pane_surface(surface());
    let frame = state.compose(106, 24).expect("composed frame");
    let rows = frame_rows(&frame);
    (state, rows)
}

fn line_with<'a>(rows: &'a [String], needle: &str) -> &'a str {
    rows.iter()
        .find(|row| row.contains(needle))
        .unwrap_or_else(|| panic!("missing row with {needle}: {rows:#?}"))
}

#[test]
fn workspace_token_hides_manual_label_without_agent_and_shows_place_and_process() {
    let config = Config::default();
    let (_, rows) = compose_text(&config, labelled_snapshot());
    line_with(&rows, "@ herdr · nvim");
    assert!(!rows.iter().any(|row| row.contains("mine")));
}

#[test]
fn workspace_token_shows_manual_label_while_agent_lives_in_first_tab() {
    let mut projected = labelled_snapshot();
    projected.tabs[0].agent_label = Some("opus".into());
    projected.agents.push(agent("tab_1", "opus"));
    let config = Config::default();
    // The workspace row shows the manual label; tab labels keep the tab rule.
    let (state, rows) = compose_text(&config, projected.clone());
    let block = state.hits.workspaces[0].rect;
    let workspace_row = rows[usize::from(block.y)]
        .chars()
        .take(usize::from(block.width))
        .collect::<String>();
    assert!(workspace_row.contains("mine"), "{workspace_row}");
    assert!(!workspace_row.contains("@ herdr"), "{workspace_row}");

    // Without a manual label the app is the agent, not the foreground process.
    projected.workspaces[0].custom_label = false;
    projected.workspaces[0].label = "herdr".into();
    let (_, rows) = compose_text(&config, projected);
    line_with(&rows, "@ herdr · opus");
}

#[test]
fn space_tokens_compose_symbol_place_app_and_auto_label() {
    let config: Config = toml::from_str(
        r#"
[ui.sidebar.spaces]
rows = [["place", "app"], ["auto_label"]]
"#,
    )
    .expect("config");
    let mut projected = labelled_snapshot();
    projected.tabs[0].place = Some("~/notes".into());
    let (_, rows) = compose_text(&config, projected);
    line_with(&rows, "~/notes · nvim");
    line_with(&rows, "@ ~/notes · nvim");
}

#[test]
fn active_later_tab_adds_branch_row_under_workspace_and_keeps_hits_aligned() {
    let config = Config::default();
    let (state, rows) = compose_text(&config, with_second_tab(labelled_snapshot(), false));
    assert!(!rows.iter().any(|row| row.contains("└─")));
    let single = state.hits.workspaces[0].rect;
    assert!(rows[usize::from(single.y)].contains("@ herdr · nvim"));

    let (mut state, rows) = compose_text(&config, with_second_tab(labelled_snapshot(), true));
    let block = state.hits.workspaces[0].rect;
    assert_eq!(block.y, single.y);
    assert_eq!(block.height, single.height + 1);
    let sidebar = |row: u16| -> String {
        rows[usize::from(row)]
            .chars()
            .take(usize::from(block.width))
            .collect()
    };
    assert!(sidebar(block.y).contains("@ herdr · nvim"));
    let tab_row = sidebar(block.y + 1);
    assert!(tab_row.contains("└─ @ ~/notes · htop"), "{tab_row}");
    assert!(sidebar(block.y + 2).contains("main"));

    // A click on the tab row belongs to the workspace block.
    let click = |kind| {
        RawInputEvent::Mouse(crossterm::event::MouseEvent {
            kind,
            column: 8,
            row: block.y + 1,
            modifiers: KeyModifiers::empty(),
        })
    };
    state.handle_raw_events(vec![click(MouseEventKind::Down(MouseButton::Left))]);
    let up = state.handle_raw_events(vec![click(MouseEventKind::Up(MouseButton::Left))]);
    let [ClientShellAction::Endpoint { request, .. }] = &up.actions[..] else {
        panic!("tab row click should focus the workspace: {:?}", up.actions);
    };
    assert!(matches!(
        &request.method,
        crate::api::schema::Method::WorkspaceFocus(target) if target.workspace_id == "ws_1"
    ));
}

#[test]
fn tab_row_shows_manual_tab_label_only_while_its_agent_lives() {
    let mut projected = with_second_tab(labelled_snapshot(), true);
    projected.tabs[1].label = "review".into();
    projected.tabs[1].custom_label = true;
    let config = Config::default();
    let (_, rows) = compose_text(&config, projected.clone());
    assert!(line_with(&rows, "└─").contains("@ ~/notes · htop"));

    projected.tabs[1].agent_label = Some("opus".into());
    let (_, rows) = compose_text(&config, projected);
    assert!(line_with(&rows, "└─").contains("review"));
}

#[test]
fn tab_bar_labels_follow_the_shared_rule() {
    let mut projected = with_second_tab(labelled_snapshot(), true);
    projected.tabs[0].label = "main".into();
    projected.tabs[0].custom_label = true;
    let config = Config::default();
    let (_, rows) = compose_text(&config, projected.clone());
    let bar = &rows[0];
    assert!(bar.contains("@ herdr · nvim"), "{bar}");
    assert!(bar.contains("@ ~/notes · htop"), "{bar}");
    assert!(!bar.contains("main"), "{bar}");

    projected.tabs[0].agent_label = Some("opus".into());
    let (_, rows) = compose_text(&config, projected);
    assert!(rows[0].contains("main"), "{}", rows[0]);
}

fn agent_tab_tokens(projected: &ClientShellSnapshot) -> Vec<String> {
    let config: Config = toml::from_str(
        r#"
[ui.sidebar.agents]
rows = [["state_icon", "tab"]]
"#,
    )
    .expect("config");
    let shell_config = ClientShellConfig::from_config(&config);
    let labels = super::super::labels::SnapshotLabels::new(projected);
    super::super::agent_sidebar::agent_rows(
        super::super::labels::LabeledSnapshot::new(projected, Some(&labels)),
        &shell_config,
        None,
    )
    .into_iter()
    .flat_map(|row| row.rows.into_iter().flatten())
    .filter_map(|token| match token.kind {
        crate::ui::ResolvedTokenKind::Tab(label) => Some(label),
        _ => None,
    })
    .collect()
}

#[test]
fn agent_panel_tab_token_follows_the_shared_rule() {
    let mut projected = with_second_tab(labelled_snapshot(), false);
    projected.tabs[0].agent_label = Some("opus".into());
    projected.agents.push(agent("tab_1", "opus"));
    assert_eq!(agent_tab_tokens(&projected), vec!["@ herdr · opus"]);

    projected.tabs[0].label = "review".into();
    projected.tabs[0].custom_label = true;
    assert_eq!(agent_tab_tokens(&projected), vec!["review"]);
}

#[test]
fn lone_tab_without_manual_label_shows_no_tab_token() {
    let mut projected = labelled_snapshot();
    projected.tabs[0].agent_label = Some("opus".into());
    projected.agents.push(agent("tab_1", "opus"));
    assert!(agent_tab_tokens(&projected).is_empty());

    // Navigator: the lone pane row names its workspace, not the tab.
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(projected.clone()));
    state.open_navigator_overlay();
    let Some(ClientShellOverlay::Navigator(navigator)) = state.overlay.as_ref() else {
        panic!("expected navigator");
    };
    let found =
        render::client_navigator_rows(&state.endpoints, &state.active_endpoint_id, navigator);
    let pane = found
        .iter()
        .find(|row| matches!(row.target, ClientNavigatorTarget::Pane { .. }))
        .expect("pane row");
    assert_eq!(pane.label, "mine");

    // Mobile switcher: neither the agent detail nor the tab list repeats the
    // workspace through the lone tab's automatic label.
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(projected.clone()));
    state.set_pane_surface(surface());
    state.mode = ClientShellMode::Navigate;
    let rows = frame_rows(&state.compose(44, 20).expect("mobile switcher"));
    let switcher = rows[2..].join("\n");
    assert!(!switcher.contains("@ herdr · opus"), "{switcher}");
    assert!(switcher.contains("tab 1"), "{switcher}");

    // A manual label on the lone tab is still shown.
    projected.tabs[0].label = "review".into();
    projected.tabs[0].custom_label = true;
    assert_eq!(agent_tab_tokens(&projected), vec!["review"]);
}

#[test]
fn snapshot_labels_index_matches_scanning_the_snapshot() {
    let mut projected = with_second_tab(labelled_snapshot(), true);
    let mut other = projected.workspaces[0].clone();
    other.workspace_id = "ws_2".into();
    other.active_tab_id = "tab_3".into();
    other.tokens = vec![("symbol".into(), "%".into())];
    projected.workspaces.push(other);
    let mut third = projected.tabs[0].clone();
    third.tab_id = "tab_3".into();
    third.workspace_id = "ws_2".into();
    third.agent_label = Some("opus".into());
    projected.tabs.push(third);
    let labels = super::super::labels::SnapshotLabels::new(&projected);
    let indexed = super::super::labels::LabeledSnapshot::new(&projected, Some(&labels));
    let scanned = super::super::labels::LabeledSnapshot::new(&projected, None);
    for workspace in 0..projected.workspaces.len() {
        assert_eq!(indexed.tab_count(workspace), scanned.tab_count(workspace));
        assert_eq!(
            indexed.active_later_tab(workspace),
            scanned.active_later_tab(workspace)
        );
        assert_eq!(
            indexed.workspace_facts(workspace),
            scanned.workspace_facts(workspace)
        );
        assert_eq!(
            indexed.workspace_label(workspace, "x"),
            scanned.workspace_label(workspace, "x")
        );
    }
    for tab in 0..projected.tabs.len() {
        assert_eq!(indexed.tab_label(tab), scanned.tab_label(tab));
    }
    assert_eq!(indexed.active_later_tab(0), Some(1));
    assert_eq!(indexed.tab_label(1), "@ ~/notes · htop");
    assert_eq!(indexed.workspace_label(1, "x"), "mine");
    assert_eq!(indexed.tab_label(2), "% herdr · opus");

    // Labels of another revision are not used.
    let mut next = projected.clone();
    next.revision += 1;
    next.tabs[1].foreground_process = Some("btop".into());
    assert_eq!(
        super::super::labels::LabeledSnapshot::new(&next, Some(&labels)).tab_label(1),
        "@ ~/notes · btop"
    );
}

#[test]
fn server_without_label_facts_keeps_existing_labels() {
    let mut projected = with_second_tab(snapshot(), true);
    projected.tabs[1].place = None;
    projected.tabs[1].foreground_process = None;
    let config = Config::default();
    let (_, rows) = compose_text(&config, projected);
    line_with(&rows, "client-shell");
    assert!(!rows.iter().any(|row| row.contains("└─")));
}

#[test]
fn navigator_rows_follow_the_shared_rule() {
    let mut projected = with_second_tab(labelled_snapshot(), true);
    projected.panes.push(ClientShellPane {
        pane_id: "pane_2".into(),
        tab_id: "tab_2".into(),
        ..projected.panes[0].clone()
    });
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(projected.clone()));
    state.open_navigator_overlay();
    let rows = |state: &ClientShellState| {
        let Some(ClientShellOverlay::Navigator(navigator)) = state.overlay.as_ref() else {
            panic!("expected navigator");
        };
        render::client_navigator_rows(&state.endpoints, &state.active_endpoint_id, navigator)
    };
    let found = rows(&state);
    let workspace = found
        .iter()
        .find(|row| matches!(row.target, ClientNavigatorTarget::Workspace { .. }))
        .expect("workspace row");
    assert_eq!(workspace.label, "@ herdr · nvim");
    assert!(found.iter().any(|row| row
        .detail
        .starts_with("@ herdr · nvim / @ ~/notes · htop / ")));
    assert!(!found.iter().any(|row| row.label.contains("mine")));

    projected.tabs[0].agent_label = Some("opus".into());
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(projected));
    state.open_navigator_overlay();
    let found = rows(&state);
    assert!(found.iter().any(|row| row.label == "mine"));
}
