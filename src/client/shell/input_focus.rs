//! Focus an agent's input pane when a Spaces or Agents row is selected.
//!
//! An agent integration may split the agent pane, open an input buffer in the
//! new pane and report the token `input_pane=<input pane id>` on the agent
//! pane. Selecting that agent or its workspace focuses the input pane while it
//! exists in the agent's tab; otherwise the original target is focused. The
//! rule reads snapshot facts only and runs per selection event, never per frame.

use super::*;

/// Agent pane token naming the pane id of its input pane.
pub(super) const INPUT_PANE_TOKEN: &str = "input_pane";

/// The agent's live input pane: the `input_pane` token names an existing pane,
/// other than the agent pane, in the agent's tab.
fn agent_input_pane<'a>(
    snapshot: &'a crate::protocol::ClientShellSnapshot,
    agent: &'a crate::protocol::ClientShellAgent,
) -> Option<&'a str> {
    let input_pane_id = agent
        .tokens
        .iter()
        .find(|(name, _)| name == INPUT_PANE_TOKEN)
        .map(|(_, value)| value.as_str())?;
    (input_pane_id != agent.pane_id
        && snapshot
            .panes
            .iter()
            .any(|pane| pane.pane_id == input_pane_id && pane.tab_id == agent.tab_id))
    .then_some(input_pane_id)
}

/// Pane to focus for a selection resolved to `target_pane_id`: the input pane
/// of that agent when it has one, otherwise the target itself.
pub(super) fn focus_pane_for_target<'a>(
    snapshot: &'a crate::protocol::ClientShellSnapshot,
    target_pane_id: &'a str,
) -> &'a str {
    snapshot
        .agents
        .iter()
        .find(|agent| agent.pane_id == target_pane_id)
        .and_then(|agent| agent_input_pane(snapshot, agent))
        .unwrap_or(target_pane_id)
}

/// Input pane for a workspace selection: the first live agent input pane of
/// the workspace's active tab, in pane order. The snapshot does not name the
/// focused pane of an unfocused workspace's tab, so the tab's agent with an
/// input pane is the target.
fn workspace_input_pane<'a>(
    snapshot: &'a crate::protocol::ClientShellSnapshot,
    workspace_id: &str,
) -> Option<&'a str> {
    let workspace = snapshot
        .workspaces
        .iter()
        .find(|workspace| workspace.workspace_id == workspace_id)?;
    snapshot
        .agents
        .iter()
        .filter(|agent| agent.tab_id == workspace.active_tab_id)
        .find_map(|agent| agent_input_pane(snapshot, agent))
}

/// Focus target for a selected Spaces or Agents row.
pub(super) fn selection_focus_target(
    snapshot: Option<&crate::protocol::ClientShellSnapshot>,
    target: ClientEndpointFocusTarget,
) -> ClientEndpointFocusTarget {
    let Some(snapshot) = snapshot else {
        return target;
    };
    match &target {
        ClientEndpointFocusTarget::Workspace(workspace_id) => {
            workspace_input_pane(snapshot, workspace_id).map_or(target.clone(), |pane_id| {
                ClientEndpointFocusTarget::Pane(pane_id.to_owned())
            })
        }
        ClientEndpointFocusTarget::Pane(pane_id) => {
            let focus = focus_pane_for_target(snapshot, pane_id);
            if focus == pane_id {
                target.clone()
            } else {
                ClientEndpointFocusTarget::Pane(focus.to_owned())
            }
        }
        _ => target,
    }
}

/// Focus method for a selected Spaces or Agents row on the active endpoint.
pub(super) fn selection_focus_method(
    snapshot: &crate::protocol::ClientShellSnapshot,
    method: crate::api::schema::Method,
) -> crate::api::schema::Method {
    use crate::api::schema::{Method, PaneTarget};
    let target = match method {
        Method::WorkspaceFocus(target) => ClientEndpointFocusTarget::Workspace(target.workspace_id),
        Method::PaneFocus(target) => ClientEndpointFocusTarget::Pane(target.pane_id),
        other => return other,
    };
    match selection_focus_target(Some(snapshot), target) {
        ClientEndpointFocusTarget::Workspace(workspace_id) => {
            Method::WorkspaceFocus(crate::api::schema::WorkspaceTarget { workspace_id })
        }
        ClientEndpointFocusTarget::Pane(pane_id) => Method::PaneFocus(PaneTarget { pane_id }),
        _ => unreachable!("selection focus keeps workspace and pane targets"),
    }
}

/// Pane the Agents list treats as current: a focused input pane maps back to
/// its agent so relative agent navigation continues from that agent.
pub(super) fn selection_pane_for_focused<'a>(
    snapshot: &'a crate::protocol::ClientShellSnapshot,
    focused_pane_id: &'a str,
) -> &'a str {
    snapshot
        .agents
        .iter()
        .find(|agent| agent_input_pane(snapshot, agent) == Some(focused_pane_id))
        .map_or(focused_pane_id, |agent| agent.pane_id.as_str())
}

impl ClientShellState {
    /// Snapshot of an endpoint: the current one when active, else its last received one.
    fn selection_snapshot(
        &self,
        endpoint_id: &ClientEndpointId,
    ) -> Option<&crate::protocol::ClientShellSnapshot> {
        if *endpoint_id == self.active_endpoint_id {
            return self.snapshot.as_deref();
        }
        self.endpoints
            .iter()
            .find(|endpoint| endpoint.endpoint_id == *endpoint_id)
            .and_then(|endpoint| endpoint.snapshot.as_deref())
    }

    /// Focus a selected workspace or agent row, preferring the agent's input pane.
    pub(super) fn focus_selection(
        &mut self,
        endpoint_id: ClientEndpointId,
        target: ClientEndpointFocusTarget,
        outcome: &mut ClientShellInput,
    ) -> bool {
        let target = selection_focus_target(self.selection_snapshot(&endpoint_id), target);
        self.focus_or_activate(endpoint_id, target, outcome)
    }
}
