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
/// other than the agent pane, in the agent's tab, and that tab is not zoomed
/// on one pane.
fn agent_input_pane<'a>(
    snapshot: &'a crate::protocol::ClientShellSnapshot,
    agent: &'a crate::protocol::ClientShellAgent,
) -> Option<&'a str> {
    let input_pane_id = agent
        .tokens
        .iter()
        .find(|(name, _)| name == INPUT_PANE_TOKEN)
        .map(|(_, value)| value.as_str())?;
    let zoomed = snapshot
        .tabs
        .iter()
        .any(|tab| tab.tab_id == agent.tab_id && tab.zoomed);
    (!zoomed
        && input_pane_id != agent.pane_id
        && snapshot
            .panes
            .iter()
            .any(|pane| pane.pane_id == input_pane_id && pane.tab_id == agent.tab_id))
    .then_some(input_pane_id)
}

/// Input pane of the agent in `target_pane_id`, when that agent has one.
pub(super) fn input_pane_for_target<'a>(
    snapshot: &'a crate::protocol::ClientShellSnapshot,
    target_pane_id: &str,
) -> Option<&'a str> {
    snapshot
        .agents
        .iter()
        .find(|agent| agent.pane_id == target_pane_id)
        .and_then(|agent| agent_input_pane(snapshot, agent))
}

/// Input pane for a workspace selection.
///
/// In the focused workspace the target is the focused pane. The snapshot does
/// not name the focused pane of another workspace's tab, so there the target is
/// the first agent of the active tab, in pane order, that has an input pane.
fn workspace_input_pane<'a>(
    snapshot: &'a crate::protocol::ClientShellSnapshot,
    workspace_id: &str,
) -> Option<&'a str> {
    if snapshot.focused_workspace_id.as_deref() == Some(workspace_id) {
        return input_pane_for_target(snapshot, snapshot.focused_pane_id.as_deref()?);
    }
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

/// Replacement pane for a selected Spaces or Agents row, if the rule applies.
pub(super) fn selection_input_pane<'a>(
    snapshot: &'a crate::protocol::ClientShellSnapshot,
    target: &ClientEndpointFocusTarget,
) -> Option<&'a str> {
    match target {
        ClientEndpointFocusTarget::Workspace(workspace_id) => {
            workspace_input_pane(snapshot, workspace_id)
        }
        ClientEndpointFocusTarget::Pane(pane_id) => input_pane_for_target(snapshot, pane_id),
        _ => None,
    }
}

/// Focus method for a selected Spaces or Agents row on the active endpoint.
pub(super) fn selection_focus_method(
    snapshot: &crate::protocol::ClientShellSnapshot,
    method: crate::api::schema::Method,
) -> crate::api::schema::Method {
    use crate::api::schema::{Method, PaneTarget};
    let input_pane = match &method {
        Method::WorkspaceFocus(target) => workspace_input_pane(snapshot, &target.workspace_id),
        Method::PaneFocus(target) => input_pane_for_target(snapshot, &target.pane_id),
        _ => None,
    };
    input_pane.map_or(method, |pane_id| {
        Method::PaneFocus(PaneTarget {
            pane_id: pane_id.to_owned(),
        })
    })
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
    /// Snapshot the rule may read for a selection on `endpoint_id`.
    ///
    /// Workspace selections on an inactive endpoint keep their target: its last
    /// snapshot may name an input pane closed since, and a stale pane target
    /// would fail where the workspace target succeeds. Agent rows come from that
    /// same snapshot, so their pane targets already share its age.
    fn selection_snapshot(
        &self,
        endpoint_id: &ClientEndpointId,
        target: &ClientEndpointFocusTarget,
    ) -> Option<&crate::protocol::ClientShellSnapshot> {
        if *endpoint_id == self.active_endpoint_id {
            return self.snapshot.as_deref();
        }
        if matches!(target, ClientEndpointFocusTarget::Workspace(_)) {
            return None;
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
        let target = self
            .selection_snapshot(&endpoint_id, &target)
            .and_then(|snapshot| selection_input_pane(snapshot, &target))
            .map_or(target, |pane_id| {
                ClientEndpointFocusTarget::Pane(pane_id.to_owned())
            });
        self.focus_or_activate(endpoint_id, target, outcome)
    }
}
