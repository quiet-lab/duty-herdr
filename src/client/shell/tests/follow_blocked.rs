use super::*;

const WORKSPACES: [&str; 4] = ["ws_1", "ws_2", "ws_3", "ws_4"];

/// Four workspaces with one agent pane each (`pane_N` in `ws_N`), `ws_1` focused.
struct Scenario {
    state: ClientShellState,
    projected: ClientShellSnapshot,
    seq: u64,
}

impl Scenario {
    fn new(enabled: bool) -> Self {
        let mut config = Config::default();
        config.ui.follow_blocked.enabled = enabled;
        let state = ClientShellState::new(ClientShellConfig::from_config(&config));
        let mut projected = snapshot();
        let template_workspace = projected.workspaces[0].clone();
        let template_tab = projected.tabs[0].clone();
        let template_pane = projected.panes[0].clone();
        projected.workspaces.clear();
        projected.tabs.clear();
        projected.panes.clear();
        for (index, workspace_id) in WORKSPACES.iter().enumerate() {
            let number = index + 1;
            let mut workspace = template_workspace.clone();
            workspace.workspace_id = (*workspace_id).into();
            workspace.active_tab_id = format!("tab_{number}");
            workspace.number = number;
            projected.workspaces.push(workspace);
            let mut tab = template_tab.clone();
            tab.tab_id = format!("tab_{number}");
            tab.workspace_id = (*workspace_id).into();
            projected.tabs.push(tab);
            let mut pane = template_pane.clone();
            pane.pane_id = format!("pane_{number}");
            pane.workspace_id = (*workspace_id).into();
            pane.tab_id = format!("tab_{number}");
            projected.panes.push(pane);
            projected.agents.push(ClientShellAgent {
                pane_id: format!("pane_{number}"),
                workspace_id: (*workspace_id).into(),
                tab_id: format!("tab_{number}"),
                name: None,
                display_agent: Some("claude".into()),
                agent: Some("claude".into()),
                title: None,
                terminal_title: None,
                terminal_title_stripped: None,
                agent_status: AgentStatus::Idle,
                state_change_seq: 0,
                state_labels: Vec::new(),
                tokens: Vec::new(),
                focused: false,
            });
        }
        let mut scenario = Self {
            state,
            projected,
            seq: 0,
        };
        scenario.focus("ws_1");
        assert!(scenario.publish().is_empty(), "baseline does not switch");
        scenario
    }

    fn focus(&mut self, workspace_id: &str) {
        let number = &workspace_id["ws_".len()..];
        self.projected.focused_workspace_id = Some(workspace_id.into());
        self.projected.focused_tab_id = Some(format!("tab_{number}"));
        self.projected.focused_pane_id = Some(format!("pane_{number}"));
        for workspace in &mut self.projected.workspaces {
            workspace.focused = workspace.workspace_id == workspace_id;
        }
    }

    fn set_status(&mut self, pane_id: &str, status: AgentStatus) {
        self.seq += 1;
        let agent = self
            .projected
            .agents
            .iter_mut()
            .find(|agent| agent.pane_id == pane_id)
            .expect("agent pane");
        agent.agent_status = status;
        agent.state_change_seq = self.seq;
    }

    /// Installs the next snapshot revision and returns the focus requests of the queue.
    fn publish(&mut self) -> Vec<crate::api::schema::Method> {
        self.publish_requests()
            .into_iter()
            .map(|request| request.method)
            .collect()
    }

    fn publish_requests(&mut self) -> Vec<crate::api::schema::Request> {
        self.projected.revision += 1;
        self.state.set_snapshot(Box::new(self.projected.clone()));
        self.state
            .follow_blocked_after_snapshot()
            .into_iter()
            .map(|action| match action {
                ClientShellAction::Endpoint { request, .. } => *request,
                other => panic!("unexpected action {other:?}"),
            })
            .collect()
    }

    /// The server applied the focus request: the next snapshot shows it.
    fn apply(&mut self, methods: &[crate::api::schema::Method]) {
        for method in methods {
            match method {
                crate::api::schema::Method::PaneFocus(target) => {
                    let workspace_id = self
                        .projected
                        .panes
                        .iter()
                        .find(|pane| pane.pane_id == target.pane_id)
                        .map(|pane| pane.workspace_id.clone())
                        .expect("pane exists");
                    self.focus(&workspace_id);
                }
                crate::api::schema::Method::WorkspaceFocus(target) => {
                    let workspace_id = target.workspace_id.clone();
                    self.focus(&workspace_id);
                }
                other => panic!("unexpected method {other:?}"),
            }
        }
        assert!(self.publish().is_empty(), "confirmation does not switch");
    }
}

fn pane_focus(pane_id: &str) -> Vec<crate::api::schema::Method> {
    vec![crate::api::schema::Method::PaneFocus(
        crate::api::schema::PaneTarget {
            pane_id: pane_id.into(),
        },
    )]
}

fn workspace_focus(workspace_id: &str) -> Vec<crate::api::schema::Method> {
    vec![crate::api::schema::Method::WorkspaceFocus(
        crate::api::schema::WorkspaceTarget {
            workspace_id: workspace_id.into(),
        },
    )]
}

#[test]
fn follow_blocked_switches_to_background_dialog_and_returns() {
    let mut scenario = Scenario::new(true);
    scenario.set_status("pane_2", AgentStatus::Blocked);
    let switch = scenario.publish();
    assert_eq!(switch, pane_focus("pane_2"));
    scenario.apply(&switch);

    scenario.set_status("pane_2", AgentStatus::Working);
    let back = scenario.publish();
    assert_eq!(back, workspace_focus("ws_1"));
    scenario.apply(&back);
}

#[test]
fn follow_blocked_stays_on_dialog_in_active_workspace() {
    let mut scenario = Scenario::new(true);
    scenario.set_status("pane_1", AgentStatus::Blocked);
    assert!(scenario.publish().is_empty());
    scenario.set_status("pane_1", AgentStatus::Working);
    assert!(scenario.publish().is_empty());
}

#[test]
fn follow_blocked_visits_dialogs_in_fifo_order() {
    let mut scenario = Scenario::new(true);
    scenario.set_status("pane_3", AgentStatus::Blocked);
    let first = scenario.publish();
    assert_eq!(first, pane_focus("pane_3"));
    // A second dialog opens before the first switch is confirmed.
    scenario.set_status("pane_2", AgentStatus::Blocked);
    assert!(scenario.publish().is_empty());
    scenario.apply(&first);

    scenario.set_status("pane_3", AgentStatus::Working);
    let second = scenario.publish();
    assert_eq!(second, pane_focus("pane_2"));
    scenario.apply(&second);

    scenario.set_status("pane_2", AgentStatus::Idle);
    assert_eq!(scenario.publish(), workspace_focus("ws_1"));
}

#[test]
fn follow_blocked_orders_dialogs_entering_in_one_snapshot_by_sequence() {
    let mut scenario = Scenario::new(true);
    scenario.set_status("pane_4", AgentStatus::Blocked);
    scenario.set_status("pane_2", AgentStatus::Blocked);
    let first = scenario.publish();
    assert_eq!(first, pane_focus("pane_4"));
    scenario.apply(&first);
    scenario.set_status("pane_4", AgentStatus::Working);
    assert_eq!(scenario.publish(), pane_focus("pane_2"));
}

#[test]
fn follow_blocked_answering_the_active_dialog_moves_to_the_queue() {
    let mut scenario = Scenario::new(true);
    scenario.set_status("pane_1", AgentStatus::Blocked);
    assert!(scenario.publish().is_empty());
    scenario.set_status("pane_2", AgentStatus::Blocked);
    assert!(scenario.publish().is_empty(), "the user is answering");
    scenario.set_status("pane_1", AgentStatus::Working);
    let next = scenario.publish();
    assert_eq!(next, pane_focus("pane_2"));
    scenario.apply(&next);
    scenario.set_status("pane_2", AgentStatus::Working);
    assert_eq!(scenario.publish(), workspace_focus("ws_1"));
}

#[test]
fn follow_blocked_stays_when_origin_workspace_is_gone() {
    let mut scenario = Scenario::new(true);
    scenario.set_status("pane_2", AgentStatus::Blocked);
    let switch = scenario.publish();
    scenario.apply(&switch);
    scenario
        .projected
        .workspaces
        .retain(|ws| ws.workspace_id != "ws_1");
    scenario
        .projected
        .tabs
        .retain(|tab| tab.workspace_id != "ws_1");
    scenario
        .projected
        .panes
        .retain(|pane| pane.workspace_id != "ws_1");
    scenario
        .projected
        .agents
        .retain(|agent| agent.workspace_id != "ws_1");
    assert!(scenario.publish().is_empty());
    scenario.set_status("pane_2", AgentStatus::Working);
    assert!(scenario.publish().is_empty());
}

#[test]
fn follow_blocked_yields_to_manual_navigation() {
    let mut scenario = Scenario::new(true);
    scenario.set_status("pane_2", AgentStatus::Blocked);
    let switch = scenario.publish();
    scenario.apply(&switch);
    scenario.set_status("pane_3", AgentStatus::Blocked);
    assert!(
        scenario.publish().is_empty(),
        "the user is answering pane_2"
    );

    // The user leaves the dialog for a workspace without one.
    scenario.focus("ws_4");
    assert!(scenario.publish().is_empty());
    // Answering elsewhere or the abandoned dialog closing does not pull the user back.
    scenario.set_status("pane_2", AgentStatus::Working);
    assert!(scenario.publish().is_empty());

    // The next dialog resumes the queue from its head; the chain starts from ws_4.
    scenario.set_status("pane_1", AgentStatus::Blocked);
    let resumed = scenario.publish();
    assert_eq!(resumed, pane_focus("pane_3"));
    scenario.apply(&resumed);
    scenario.set_status("pane_3", AgentStatus::Working);
    let next = scenario.publish();
    assert_eq!(next, pane_focus("pane_1"));
    scenario.apply(&next);
    scenario.set_status("pane_1", AgentStatus::Working);
    assert_eq!(scenario.publish(), workspace_focus("ws_4"));
}

#[test]
fn follow_blocked_snapshot_before_confirmation_is_not_manual_navigation() {
    let mut scenario = Scenario::new(true);
    scenario.set_status("pane_2", AgentStatus::Blocked);
    let switch = scenario.publish();
    assert_eq!(switch, pane_focus("pane_2"));
    // A snapshot produced before the server applied the focus still shows ws_1.
    assert!(scenario.publish().is_empty());
    scenario.apply(&switch);
    scenario.set_status("pane_2", AgentStatus::Working);
    assert_eq!(scenario.publish(), workspace_focus("ws_1"));
}

#[test]
fn follow_blocked_ignores_panes_without_agent_and_other_statuses() {
    let mut scenario = Scenario::new(true);
    scenario.projected.agents[1].agent = None;
    scenario.set_status("pane_2", AgentStatus::Blocked);
    scenario.set_status("pane_3", AgentStatus::Idle);
    scenario.set_status("pane_4", AgentStatus::Done);
    assert!(scenario.publish().is_empty());
}

#[test]
fn follow_blocked_ignores_agents_of_missing_panes() {
    let mut scenario = Scenario::new(true);
    scenario
        .projected
        .panes
        .retain(|pane| pane.pane_id != "pane_2");
    scenario.set_status("pane_2", AgentStatus::Blocked);
    assert!(scenario.publish().is_empty());
}

#[test]
fn follow_blocked_baseline_queues_open_dialogs_without_switching() {
    let mut config = Config::default();
    config.ui.follow_blocked.enabled = true;
    let mut scenario = Scenario::new(false);
    scenario.set_status("pane_2", AgentStatus::Blocked);
    assert!(scenario.publish().is_empty(), "disabled");
    scenario.state.config.apply_live_config(&config, &[], &[]);
    assert!(scenario.publish().is_empty(), "baseline after enabling");
    scenario.set_status("pane_3", AgentStatus::Blocked);
    assert_eq!(scenario.publish(), pane_focus("pane_2"));
}

#[test]
fn follow_blocked_disabled_never_switches() {
    let mut scenario = Scenario::new(false);
    scenario.set_status("pane_2", AgentStatus::Blocked);
    assert!(scenario.publish().is_empty());
    scenario.set_status("pane_3", AgentStatus::Blocked);
    assert!(scenario.publish().is_empty());
    scenario.set_status("pane_2", AgentStatus::Working);
    assert!(scenario.publish().is_empty());
}

#[test]
fn follow_blocked_restarts_from_baseline_after_server_restart() {
    let mut scenario = Scenario::new(true);
    scenario.set_status("pane_2", AgentStatus::Blocked);
    let switch = scenario.publish();
    scenario.apply(&switch);
    scenario.projected.boot_id = "boot-2".into();
    assert!(scenario.publish().is_empty());
    scenario.set_status("pane_2", AgentStatus::Working);
    assert!(scenario.publish().is_empty(), "the new boot has no chain");
}

fn add_input_pane(scenario: &mut Scenario, pane_id: &str, tab_id: &str) {
    let mut pane = scenario.projected.panes[0].clone();
    pane.pane_id = pane_id.into();
    pane.tab_id = tab_id.into();
    pane.workspace_id = format!("ws_{}", &tab_id["tab_".len()..]);
    scenario.projected.panes.push(pane);
}

fn set_input_token(scenario: &mut Scenario, agent_pane: &str, input_pane: &str) {
    let agent = scenario
        .projected
        .agents
        .iter_mut()
        .find(|agent| agent.pane_id == agent_pane)
        .expect("agent pane");
    agent.tokens = vec![("input_pane".into(), input_pane.into())];
}

#[test]
fn follow_blocked_focuses_the_input_pane_of_the_dialog() {
    let mut scenario = Scenario::new(true);
    add_input_pane(&mut scenario, "nvim_2", "tab_2");
    set_input_token(&mut scenario, "pane_2", "nvim_2");
    scenario.set_status("pane_2", AgentStatus::Blocked);
    let switch = scenario.publish();
    assert_eq!(switch, pane_focus("nvim_2"));
    scenario.apply(&switch);
    scenario.set_status("pane_2", AgentStatus::Working);
    assert_eq!(scenario.publish(), workspace_focus("ws_1"));
}

#[test]
fn follow_blocked_focuses_the_dialog_without_a_usable_input_pane() {
    let mut scenario = Scenario::new(true);
    // The input pane lives in another tab, and pane_3 names a missing pane.
    add_input_pane(&mut scenario, "nvim_x", "tab_4");
    set_input_token(&mut scenario, "pane_2", "nvim_x");
    set_input_token(&mut scenario, "pane_3", "nvim_missing");
    scenario.set_status("pane_2", AgentStatus::Blocked);
    let switch = scenario.publish();
    assert_eq!(switch, pane_focus("pane_2"));
    scenario.apply(&switch);
    scenario.set_status("pane_3", AgentStatus::Blocked);
    assert!(scenario.publish().is_empty());
    scenario.set_status("pane_2", AgentStatus::Working);
    assert_eq!(scenario.publish(), pane_focus("pane_3"));
}

#[test]
fn follow_blocked_detects_manual_navigation_before_confirmation() {
    let mut scenario = Scenario::new(true);
    scenario.set_status("pane_2", AgentStatus::Blocked);
    assert_eq!(scenario.publish(), pane_focus("pane_2"));
    // The server applied the switch and the user's own switch to ws_4 in one snapshot.
    scenario.focus("ws_4");
    assert!(scenario.publish().is_empty());
    scenario.set_status("pane_2", AgentStatus::Working);
    assert!(scenario.publish().is_empty(), "no return against the user");
}

#[test]
fn follow_blocked_rejected_switch_ends_the_chain_quietly() {
    let mut scenario = Scenario::new(true);
    scenario.set_status("pane_2", AgentStatus::Blocked);
    let requests = scenario.publish_requests();
    assert_eq!(requests.len(), 1);
    let (repaint, actions) = scenario.state.handle_endpoint_result(
        "boot-1",
        &requests[0].id,
        Err(ClientShellEndpointError {
            code: Some("pane_not_found".into()),
            message: "pane not found".into(),
        }),
    );
    assert!(!repaint && actions.is_empty());
    assert!(scenario.state.visible_endpoint_notice.is_none());
    // The next dialog is followed again.
    scenario.set_status("pane_3", AgentStatus::Blocked);
    assert_eq!(scenario.publish(), pane_focus("pane_2"));
}

#[test]
fn follow_blocked_dialog_asking_again_between_snapshots_moves_on() {
    let mut scenario = Scenario::new(true);
    scenario.set_status("pane_2", AgentStatus::Blocked);
    let switch = scenario.publish();
    scenario.apply(&switch);
    scenario.set_status("pane_3", AgentStatus::Blocked);
    assert!(scenario.publish().is_empty());
    // pane_2 is answered and asks the next question before the client sees a snapshot.
    scenario.set_status("pane_2", AgentStatus::Working);
    scenario.set_status("pane_2", AgentStatus::Blocked);
    assert!(
        scenario.publish().is_empty(),
        "the user stays on the follow-up question"
    );
    // Answering the follow-up question is an answer of the followed pane.
    scenario.set_status("pane_2", AgentStatus::Working);
    let next = scenario.publish();
    assert_eq!(next, pane_focus("pane_3"));
    scenario.apply(&next);
    scenario.set_status("pane_3", AgentStatus::Working);
    assert_eq!(scenario.publish(), workspace_focus("ws_1"));
}

#[test]
fn follow_blocked_dialog_asking_again_moves_behind_the_queue() {
    let mut scenario = Scenario::new(true);
    scenario.set_status("pane_2", AgentStatus::Blocked);
    let switch = scenario.publish();
    scenario.apply(&switch);
    scenario.set_status("pane_3", AgentStatus::Blocked);
    scenario.set_status("pane_4", AgentStatus::Blocked);
    assert!(scenario.publish().is_empty());
    // pane_3 asks again while waiting: it goes behind pane_4.
    scenario.set_status("pane_3", AgentStatus::Working);
    scenario.set_status("pane_3", AgentStatus::Blocked);
    assert!(
        scenario.publish().is_empty(),
        "the user is answering pane_2"
    );
    scenario.set_status("pane_2", AgentStatus::Working);
    assert_eq!(scenario.publish(), pane_focus("pane_4"));
}

#[test]
fn follow_blocked_baseline_follows_the_focused_dialog() {
    let mut scenario = Scenario::new(true);
    scenario.set_status("pane_1", AgentStatus::Blocked);
    scenario.set_status("pane_2", AgentStatus::Blocked);
    scenario.projected.boot_id = "boot-2".into();
    assert!(scenario.publish().is_empty(), "baseline");
    scenario.set_status("pane_1", AgentStatus::Working);
    let next = scenario.publish();
    assert_eq!(next, pane_focus("pane_2"));
    scenario.apply(&next);
    scenario.set_status("pane_2", AgentStatus::Working);
    assert_eq!(scenario.publish(), workspace_focus("ws_1"));
}
