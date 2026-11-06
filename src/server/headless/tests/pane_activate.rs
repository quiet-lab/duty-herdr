use super::*;
use crate::api::schema::{PaneActivateResult, PaneTarget, ResponseResult, SuccessResponse};

struct ActivationFixture {
    server: HeadlessServer,
    second_root: crate::layout::PaneId,
    second_split: crate::layout::PaneId,
    root_input: tokio::sync::mpsc::Receiver<Bytes>,
    split_input: tokio::sync::mpsc::Receiver<Bytes>,
}

fn focus_reporting_runtime() -> (
    crate::terminal::TerminalRuntime,
    tokio::sync::mpsc::Receiver<Bytes>,
) {
    crate::terminal::TerminalRuntime::test_with_channel_and_scrollback_bytes(
        80,
        24,
        0,
        b"\x1b[?1004h",
        4,
    )
}

/// Workspace 0 is the server's focused workspace. Workspace 1 has a second
/// tab split into two focus-reporting panes, the first one focused.
fn activation_fixture() -> ActivationFixture {
    let mut server = test_headless_server();
    let seen = crate::workspace::Workspace::test_new("seen");
    let mut background = crate::workspace::Workspace::test_new("background");
    let second_tab = background.test_add_tab(Some("second"));
    background.switch_tab(second_tab);
    let second_root = background.tabs[second_tab].root_pane;
    let second_split = background.test_split(ratatui::layout::Direction::Horizontal);
    background.tabs[second_tab].layout.focus_pane(second_root);
    background.switch_tab(0);
    let (root_runtime, root_input) = focus_reporting_runtime();
    let (split_runtime, split_input) = focus_reporting_runtime();
    background.insert_test_runtime(second_root, root_runtime);
    background.insert_test_runtime(second_split, split_runtime);
    server.app.state.workspaces = vec![seen, background];
    server.app.state.ensure_test_terminals();
    server.app.state.active = Some(0);
    server.app.state.selected = 0;
    server.app.state.mode = crate::app::Mode::Terminal;
    ActivationFixture {
        server,
        second_root,
        second_split,
        root_input,
        split_input,
    }
}

fn activate_request(
    pane_id: String,
) -> (
    crate::api::ApiRequestMessage,
    std::sync::mpsc::Receiver<String>,
) {
    let (respond_to, response_rx) = std::sync::mpsc::channel();
    (
        crate::api::ApiRequestMessage {
            request: crate::api::schema::Request {
                id: "activate".into(),
                method: crate::api::schema::Method::PaneActivate(PaneTarget { pane_id }),
            },
            respond_to,
            response_write_complete: None,
        },
        response_rx,
    )
}

fn activation_result(response_rx: &std::sync::mpsc::Receiver<String>) -> PaneActivateResult {
    let response = response_rx.recv().expect("pane activate response");
    match serde_json::from_str::<SuccessResponse>(&response) {
        Ok(SuccessResponse {
            result: ResponseResult::PaneActivate { activation },
            ..
        }) => activation,
        other => panic!("expected pane activate response, got {other:?}: {response}"),
    }
}

#[tokio::test]
async fn public_pane_activate_moves_no_client_and_refocuses_watchers() {
    let mut fixture = activation_fixture();
    let server = &mut fixture.server;
    let seen_tab_id = server.app.public_tab_id(0, 0).expect("seen tab id");
    let second_tab_id = server.app.public_tab_id(1, 1).expect("second tab id");
    let (away_control, _) = connect_matching_test_shell(server, 70);
    let (watcher_control, _) = connect_matching_test_shell(server, 71);
    let _ = away_control.recv().expect("away snapshot");
    let _ = watcher_control.recv().expect("watcher snapshot");
    assert!(server.focus_shell_client_on_tab(71, &second_tab_id));
    server.clients.get_mut(&70).unwrap().outer_terminal_focus = Some(true);
    server.clients.get_mut(&71).unwrap().outer_terminal_focus = Some(true);
    while fixture.root_input.try_recv().is_ok() {}
    let split_public = server.app.public_pane_id(1, fixture.second_split).unwrap();

    let (request, response_rx) = activate_request(split_public.clone());
    server.handle_api_request_with_shutdown_check(request);
    let activation = activation_result(&response_rx);

    // The watcher looked at the second tab through its own location; the
    // workspace's session-wide active tab was still the first one.
    assert!(activation.changed && activation.focus_changed && activation.tab_changed);
    assert!(!activation.workspace_focused);
    assert_eq!(activation.pane_id, split_public);
    assert_eq!(server.app.state.active, Some(0));
    assert_eq!(
        server.shell_tab_id_for_client(70).as_deref(),
        Some(seen_tab_id.as_str()),
        "the client on another workspace stays there"
    );
    assert_eq!(
        server.shell_tab_id_for_client(71).as_deref(),
        Some(second_tab_id.as_str())
    );
    assert_eq!(
        server.shell_focus_target(71).map(|target| target.pane_id),
        Some(fixture.second_split),
        "the watching client sees the focus move at once"
    );
    assert_eq!(
        fixture
            .root_input
            .try_recv()
            .expect("previous pane focus lost"),
        Bytes::from_static(b"\x1b[O")
    );
    assert_eq!(
        fixture
            .split_input
            .try_recv()
            .expect("activated pane focus gained"),
        Bytes::from_static(b"\x1b[I")
    );

    // The away client finds the activated pane when it switches over.
    let workspace_id = server.app.public_workspace_id(1);
    let (respond_to, _response_rx) = std::sync::mpsc::channel();
    server.handle_client_shell_api_request(
        70,
        crate::api::ApiRequestMessage {
            request: crate::api::schema::Request {
                id: "switch".into(),
                method: crate::api::schema::Method::WorkspaceFocus(
                    crate::api::schema::WorkspaceTarget { workspace_id },
                ),
            },
            respond_to,
            response_write_complete: None,
        },
    );
    assert_eq!(
        server.shell_tab_id_for_client(70).as_deref(),
        Some(second_tab_id.as_str())
    );
    assert_eq!(
        server.shell_focus_target(70).map(|target| target.pane_id),
        Some(fixture.second_split)
    );
    server.app.state.assert_invariants_for_test();
    shutdown_test_runtimes(server);
}

#[tokio::test]
async fn public_pane_activate_selects_background_tab_for_every_client() {
    let mut fixture = activation_fixture();
    let server = &mut fixture.server;
    let seen_tab_id = server.app.public_tab_id(0, 0).expect("seen tab id");
    let first_tab_id = server.app.public_tab_id(1, 0).expect("first tab id");
    let second_tab_id = server.app.public_tab_id(1, 1).expect("second tab id");
    let (away_control, _) = connect_matching_test_shell(server, 72);
    let (other_control, _) = connect_matching_test_shell(server, 73);
    let _ = away_control.recv().expect("away snapshot");
    let _ = other_control.recv().expect("other snapshot");
    assert!(server.focus_shell_client_on_tab(73, &first_tab_id));
    let root_public = server.app.public_pane_id(1, fixture.second_root).unwrap();

    let (request, response_rx) = activate_request(root_public);
    server.handle_api_request_with_shutdown_check(request);
    let activation = activation_result(&response_rx);

    assert!(activation.tab_changed && !activation.focus_changed);
    assert_eq!(activation.tab_id, second_tab_id);
    assert_eq!(server.app.state.active, Some(0));
    assert_eq!(server.app.state.workspaces[1].active_tab, 1);
    assert_eq!(
        server.shell_tab_id_for_client(72).as_deref(),
        Some(seen_tab_id.as_str())
    );
    assert_eq!(
        server.shell_tab_id_for_client(73).as_deref(),
        Some(second_tab_id.as_str()),
        "a client on the workspace follows the activated tab"
    );
    for client_id in [72, 73] {
        let location = server.clients[&client_id]
            .shell_location
            .as_ref()
            .expect("shell location");
        assert_eq!(
            location
                .active_tab_ids
                .get(&server.app.public_workspace_id(1)),
            Some(&second_tab_id)
        );
    }
    server.app.state.assert_invariants_for_test();
    shutdown_test_runtimes(server);
}

#[tokio::test]
async fn shell_pane_activate_keeps_the_requesting_client_workspace() {
    let mut fixture = activation_fixture();
    let server = &mut fixture.server;
    let seen_tab_id = server.app.public_tab_id(0, 0).expect("seen tab id");
    let second_tab_id = server.app.public_tab_id(1, 1).expect("second tab id");
    let (control, _) = connect_matching_test_shell(server, 74);
    let _ = control.recv().expect("snapshot");
    let split_public = server.app.public_pane_id(1, fixture.second_split).unwrap();

    let (request, response_rx) = activate_request(split_public);
    server.handle_client_shell_api_request(74, request);
    let activation = activation_result(&response_rx);

    assert!(activation.changed);
    assert_eq!(
        server.shell_tab_id_for_client(74).as_deref(),
        Some(seen_tab_id.as_str())
    );
    let location = server.clients[&74].shell_location.as_ref().unwrap();
    assert_eq!(
        location
            .active_tab_ids
            .get(&server.app.public_workspace_id(1)),
        Some(&second_tab_id)
    );
    assert_eq!(
        server.app.state.workspaces[1].focused_pane_id(),
        Some(fixture.second_split)
    );
    server.app.state.assert_invariants_for_test();
    shutdown_test_runtimes(server);
}

#[tokio::test]
async fn public_pane_activate_unknown_pane_changes_nothing() {
    let mut fixture = activation_fixture();
    let server = &mut fixture.server;
    let (control, _) = connect_matching_test_shell(server, 75);
    let _ = control.recv().expect("snapshot");
    let before = server.clients[&75].shell_location.clone();

    let (request, response_rx) = activate_request("w9:p9".into());
    server.handle_api_request_with_shutdown_check(request);
    let response = response_rx.recv().expect("response");
    let error: crate::api::schema::ErrorResponse =
        serde_json::from_str(&response).expect("error response");

    assert_eq!(error.error.code, "pane_not_found");
    assert_eq!(server.clients[&75].shell_location, before);
    assert_eq!(server.app.state.workspaces[1].active_tab, 0);
    shutdown_test_runtimes(server);
}
