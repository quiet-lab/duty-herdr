use super::super::input_focus as rule;
use super::*;
use crate::api::schema::Method;

fn agent(pane_id: &str, tab_id: &str, input_pane: Option<&str>) -> ClientShellAgent {
    ClientShellAgent {
        pane_id: pane_id.into(),
        workspace_id: "ws_1".into(),
        tab_id: tab_id.into(),
        name: Some(pane_id.into()),
        display_agent: None,
        agent: Some("claude".into()),
        title: None,
        terminal_title: None,
        terminal_title_stripped: None,
        agent_status: AgentStatus::Idle,
        state_change_seq: 1,
        state_labels: Vec::new(),
        tokens: input_pane
            .map(|pane| vec![("input_pane".to_owned(), pane.to_owned())])
            .unwrap_or_default(),
        focused: false,
    }
}

fn pane(pane_id: &str, workspace_id: &str, tab_id: &str) -> ClientShellPane {
    let mut pane = snapshot().panes[0].clone();
    pane.pane_id = pane_id.into();
    pane.workspace_id = workspace_id.into();
    pane.tab_id = tab_id.into();
    pane.focused = false;
    pane
}

/// `ws_1` holds the focused shell `pane_1`; `ws_2`'s active tab `tab_2`
/// holds the agent `pane_2` and its input pane `pane_3`.
fn input_snapshot(input_pane: Option<&str>) -> ClientShellSnapshot {
    let mut projected = snapshot();
    let mut workspace = projected.workspaces[0].clone();
    workspace.workspace_id = "ws_2".into();
    workspace.active_tab_id = "tab_2".into();
    workspace.number = 2;
    workspace.label = "agent".into();
    workspace.focused = false;
    projected.workspaces.push(workspace);
    let mut tab = projected.tabs[0].clone();
    tab.tab_id = "tab_2".into();
    tab.workspace_id = "ws_2".into();
    tab.focused = false;
    projected.tabs.push(tab);
    projected.panes.push(pane("pane_2", "ws_2", "tab_2"));
    projected.panes.push(pane("pane_3", "ws_2", "tab_2"));
    let mut claude = agent("pane_2", "tab_2", input_pane);
    claude.workspace_id = "ws_2".into();
    projected.agents.push(claude);
    projected
}

fn single_focus_method(outcome: &ClientShellInput) -> &Method {
    let [ClientShellAction::Endpoint { request, .. }] = &outcome.actions[..] else {
        panic!("selection should send one endpoint focus request");
    };
    &request.method
}

fn click(state: &mut ClientShellState, rect: Rect) -> ClientShellInput {
    let mut outcome = state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: rect.x + 2,
        row: rect.y,
        modifiers: KeyModifiers::empty(),
    })]);
    let up = state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
        kind: MouseEventKind::Up(MouseButton::Left),
        column: rect.x + 2,
        row: rect.y,
        modifiers: KeyModifiers::empty(),
    })]);
    outcome.actions.extend(up.actions);
    outcome
}

fn shell_state(projected: ClientShellSnapshot) -> ClientShellState {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(projected));
    state.set_pane_surface(surface());
    state.compose(106, 30).expect("sidebar frame");
    state
}

fn workspace_rect(state: &ClientShellState, workspace_id: &str) -> Rect {
    state
        .hits
        .workspaces
        .iter()
        .find(|hit| hit.workspace_id == workspace_id)
        .expect("workspace row")
        .rect
}

fn agent_rect(state: &ClientShellState, pane_id: &str) -> Rect {
    state
        .hits
        .agents
        .iter()
        .find(|(_, agent_pane_id)| agent_pane_id == pane_id)
        .expect("agent row")
        .0
}

fn is_pane_focus(method: &Method, pane_id: &str) -> bool {
    matches!(method, Method::PaneFocus(target) if target.pane_id == pane_id)
}

fn is_workspace_focus(method: &Method, workspace_id: &str) -> bool {
    matches!(method, Method::WorkspaceFocus(target) if target.workspace_id == workspace_id)
}

#[test]
fn selecting_workspace_or_agent_focuses_the_agent_input_pane() {
    let mut state = shell_state(input_snapshot(Some("pane_3")));

    let workspace = {
        let rect = workspace_rect(&state, "ws_2");
        click(&mut state, rect)
    };
    assert!(is_pane_focus(single_focus_method(&workspace), "pane_3"));

    let agent = {
        let rect = agent_rect(&state, "pane_2");
        click(&mut state, rect)
    };
    assert!(is_pane_focus(single_focus_method(&agent), "pane_3"));

    let mut key = ClientShellInput::default();
    state.record_binding(
        crate::input::KeybindMatch::Action(crate::input::KeybindAction::FocusAgent(0)),
        &mut key,
    );
    assert!(is_pane_focus(single_focus_method(&key), "pane_3"));

    let mut switch = ClientShellInput::default();
    state.record_binding(
        crate::input::KeybindMatch::Action(crate::input::KeybindAction::SwitchWorkspace(1)),
        &mut switch,
    );
    assert!(is_pane_focus(single_focus_method(&switch), "pane_3"));
}

#[test]
fn selection_without_input_token_keeps_the_previous_target() {
    let mut state = shell_state(input_snapshot(None));

    let workspace = {
        let rect = workspace_rect(&state, "ws_2");
        click(&mut state, rect)
    };
    assert!(is_workspace_focus(single_focus_method(&workspace), "ws_2"));

    let agent = {
        let rect = agent_rect(&state, "pane_2");
        click(&mut state, rect)
    };
    assert!(is_pane_focus(single_focus_method(&agent), "pane_2"));
}

#[test]
fn input_token_outside_the_agent_tab_or_for_a_closed_pane_is_ignored() {
    let mut other_tab = input_snapshot(Some("pane_1"));
    assert_eq!(rule::focus_pane_for_target(&other_tab, "pane_2"), "pane_2");
    other_tab.agents[0].tokens = vec![("input_pane".into(), "pane_9".into())];
    assert_eq!(rule::focus_pane_for_target(&other_tab, "pane_2"), "pane_2");
    other_tab.agents[0].tokens = vec![("input_pane".into(), "pane_2".into())];
    assert_eq!(rule::focus_pane_for_target(&other_tab, "pane_2"), "pane_2");
    assert_eq!(
        rule::selection_focus_target(
            Some(&other_tab),
            ClientEndpointFocusTarget::Workspace("ws_2".into()),
        ),
        ClientEndpointFocusTarget::Workspace("ws_2".into())
    );
    // A non-agent target never redirects.
    assert_eq!(rule::focus_pane_for_target(&other_tab, "pane_1"), "pane_1");
}

#[test]
fn workspace_selection_uses_the_first_agent_input_pane_in_pane_order() {
    let mut projected = input_snapshot(Some("pane_3"));
    projected.panes.push(pane("pane_4", "ws_2", "tab_2"));
    projected.panes.push(pane("pane_5", "ws_2", "tab_2"));
    let mut later = agent("pane_4", "tab_2", Some("pane_5"));
    later.workspace_id = "ws_2".into();
    projected.agents.push(later);

    assert_eq!(
        rule::selection_focus_target(
            Some(&projected),
            ClientEndpointFocusTarget::Workspace("ws_2".into()),
        ),
        ClientEndpointFocusTarget::Pane("pane_3".into())
    );
    assert_eq!(rule::focus_pane_for_target(&projected, "pane_4"), "pane_5");
}

#[test]
fn relative_agent_navigation_continues_from_the_agent_of_a_focused_input_pane() {
    let mut projected = input_snapshot(Some("pane_3"));
    projected.panes.push(pane("pane_4", "ws_2", "tab_2"));
    let mut second = agent("pane_4", "tab_2", None);
    second.workspace_id = "ws_2".into();
    projected.agents.push(second);
    projected.focused_workspace_id = Some("ws_2".into());
    projected.focused_tab_id = Some("tab_2".into());
    projected.focused_pane_id = Some("pane_3".into());
    assert_eq!(
        rule::selection_pane_for_focused(&projected, "pane_3"),
        "pane_2"
    );
    let mut state = shell_state(projected);

    let mut next = ClientShellInput::default();
    state.record_binding(
        crate::input::KeybindMatch::Action(crate::input::KeybindAction::NextAgent),
        &mut next,
    );
    assert!(is_pane_focus(single_focus_method(&next), "pane_4"));
}
