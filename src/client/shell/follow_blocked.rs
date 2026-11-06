//! Following agent dialogs (`[ui.follow_blocked]`).
//!
//! The client switches its active workspace to a workspace whose agent pane waits in
//! `blocked`, then to the next one, and returns to the workspace it started from once no
//! dialog is left. The queue is client presentation state computed from server facts (agent
//! statuses in the active endpoint snapshot); it runs once per installed snapshot, never per
//! frame and never on a timer.
//!
//! Rules:
//! - A pane counts as a dialog while it is a detected agent in `blocked` whose pane and
//!   workspace are present in the snapshot.
//! - The queue holds dialogs in the order they entered `blocked` (FIFO by
//!   `state_change_seq`). The first snapshot of an endpoint boot is a baseline: dialogs already
//!   open are queued without switching.
//! - Only two events switch: a pane enters `blocked`, or the followed pane leaves it.
//! - While the active workspace holds a dialog nothing switches; that dialog becomes the
//!   followed pane, so answering it moves on.
//! - Otherwise the client focuses the queue head (or the input pane its `input_pane` token
//!   names in the same tab) and remembers the workspace it left as the
//!   origin of the chain (only on the first switch of a chain).
//! - When the followed pane leaves `blocked` and the queue is empty, the client returns to the
//!   origin if it still exists.
//! - When the user leaves the followed workspace after the switch is confirmed, the chain ends
//!   (no follow-up switch, no return); the queue continues from the next entry into
//!   `blocked`.

use std::collections::{HashMap, VecDeque};

use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum FollowBlockedFocus {
    Pane(String),
    Workspace(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FollowedPane {
    pane_id: String,
    workspace_id: String,
    /// The snapshot showed the followed workspace focused. Until then a snapshot that
    /// predates the focus request must not read as the user leaving.
    confirmed: bool,
}

#[derive(Debug, Default)]
pub(super) struct FollowBlocked {
    scope: Option<(ClientEndpointId, String)>,
    /// Current dialogs: pane ID to workspace ID.
    blocked: HashMap<String, String>,
    queue: VecDeque<String>,
    followed: Option<FollowedPane>,
    origin_workspace_id: Option<String>,
}

fn dialog_panes(snapshot: &ClientShellSnapshot) -> Vec<(&str, &str, u64)> {
    snapshot
        .agents
        .iter()
        .filter(|agent| agent.agent_status == crate::api::schema::AgentStatus::Blocked)
        .filter(|agent| agent.agent.is_some())
        .filter(|agent| {
            snapshot
                .workspaces
                .iter()
                .any(|workspace| workspace.workspace_id == agent.workspace_id)
                && snapshot
                    .panes
                    .iter()
                    .any(|pane| pane.pane_id == agent.pane_id)
        })
        .map(|agent| {
            (
                agent.pane_id.as_str(),
                agent.workspace_id.as_str(),
                agent.state_change_seq,
            )
        })
        .collect()
}

impl FollowBlocked {
    pub(super) fn reset(&mut self) {
        *self = Self::default();
    }

    /// The focus request could not be sent: forget the unconfirmed switch so the next
    /// event can try again.
    fn abandon_unconfirmed(&mut self) {
        if self
            .followed
            .as_ref()
            .is_some_and(|followed| !followed.confirmed)
        {
            self.followed = None;
        }
    }

    /// Updates the queue from the active endpoint snapshot and returns the focus change to
    /// request, if any.
    pub(super) fn observe(
        &mut self,
        endpoint_id: &ClientEndpointId,
        snapshot: &ClientShellSnapshot,
    ) -> Option<FollowBlockedFocus> {
        let mut dialogs = dialog_panes(snapshot);
        dialogs.sort_by(|left, right| left.2.cmp(&right.2).then(left.0.cmp(right.0)));
        let same_scope = self
            .scope
            .as_ref()
            .is_some_and(|(endpoint, boot)| endpoint == endpoint_id && boot == &snapshot.boot_id);
        if !same_scope {
            self.reset();
            self.scope = Some((endpoint_id.clone(), snapshot.boot_id.clone()));
            for (pane_id, workspace_id, _) in dialogs {
                self.blocked
                    .insert(pane_id.to_owned(), workspace_id.to_owned());
                self.queue.push_back(pane_id.to_owned());
            }
            return None;
        }

        let mut entered = false;
        let mut current = HashMap::with_capacity(dialogs.len());
        for (pane_id, workspace_id, _) in dialogs {
            if !self.blocked.contains_key(pane_id) {
                entered = true;
                self.queue.push_back(pane_id.to_owned());
            }
            current.insert(pane_id.to_owned(), workspace_id.to_owned());
        }
        self.blocked = current;
        let blocked = &self.blocked;
        self.queue.retain(|pane_id| blocked.contains_key(pane_id));

        let focused = snapshot.focused_workspace_id.as_deref();
        let mut answered = false;
        if let Some(followed) = self.followed.as_mut() {
            match self.blocked.get(&followed.pane_id) {
                None => {
                    self.followed = None;
                    answered = true;
                }
                Some(workspace_id) => {
                    followed.workspace_id.clone_from(workspace_id);
                    if focused == Some(followed.workspace_id.as_str()) {
                        followed.confirmed = true;
                    } else if followed.confirmed {
                        // The user moved away from the dialog the client switched to.
                        self.followed = None;
                        self.origin_workspace_id = None;
                    }
                }
            }
        }
        if !(entered || answered) || self.followed.is_some() {
            return None;
        }

        let queued = |pane_id: &String| {
            self.blocked
                .get(pane_id)
                .map(|workspace_id| (pane_id.clone(), workspace_id.clone()))
        };
        if let Some((pane_id, workspace_id)) = self
            .queue
            .iter()
            .filter_map(queued)
            .find(|(_, workspace_id)| Some(workspace_id.as_str()) == focused)
        {
            // The user is looking at a dialog: wait for its answer.
            self.followed = Some(FollowedPane {
                pane_id,
                workspace_id,
                confirmed: true,
            });
            if self.origin_workspace_id.is_none() {
                self.origin_workspace_id = focused.map(str::to_owned);
            }
            return None;
        }

        if let Some((pane_id, workspace_id)) = self.queue.iter().find_map(queued) {
            if self.origin_workspace_id.is_none() {
                self.origin_workspace_id = focused.map(str::to_owned);
            }
            self.followed = Some(FollowedPane {
                pane_id: pane_id.clone(),
                workspace_id,
                confirmed: false,
            });
            return Some(FollowBlockedFocus::Pane(pane_id));
        }

        let origin = self.origin_workspace_id.take()?;
        (answered
            && focused != Some(origin.as_str())
            && snapshot
                .workspaces
                .iter()
                .any(|workspace| workspace.workspace_id == origin))
        .then_some(FollowBlockedFocus::Workspace(origin))
    }
}

/// Pane to focus for a dialog pane: the input pane named by the agent's `input_pane` token
/// when it exists in the same tab, otherwise the dialog pane itself.
///
/// NOTE: local copy of the target-pane rule of the sidebar-focus-input task; replace it with
/// that shared function when both branches merge.
fn focus_pane_for_dialog(snapshot: &ClientShellSnapshot, pane_id: String) -> String {
    let Some(agent) = snapshot
        .agents
        .iter()
        .find(|agent| agent.pane_id == pane_id)
    else {
        return pane_id;
    };
    let Some((_, input_pane_id)) = agent.tokens.iter().find(|(key, _)| key == "input_pane") else {
        return pane_id;
    };
    if snapshot
        .panes
        .iter()
        .any(|pane| &pane.pane_id == input_pane_id && pane.tab_id == agent.tab_id)
    {
        input_pane_id.clone()
    } else {
        pane_id
    }
}

impl ClientShellState {
    /// Runs the dialog queue against the active endpoint snapshot that was just installed and
    /// returns the endpoint request that moves focus, if the queue asks for one.
    pub(crate) fn follow_blocked_after_snapshot(&mut self) -> Vec<ClientShellAction> {
        if !self.config.follow_blocked {
            self.follow_blocked.reset();
            return Vec::new();
        }
        let Some(snapshot) = self.snapshot.as_deref() else {
            return Vec::new();
        };
        let Some(focus) = self
            .follow_blocked
            .observe(&self.active_endpoint_id, snapshot)
        else {
            return Vec::new();
        };
        let method = match focus {
            FollowBlockedFocus::Pane(pane_id) => {
                crate::api::schema::Method::PaneFocus(crate::api::schema::PaneTarget {
                    pane_id: focus_pane_for_dialog(snapshot, pane_id),
                })
            }
            FollowBlockedFocus::Workspace(workspace_id) => {
                crate::api::schema::Method::WorkspaceFocus(crate::api::schema::WorkspaceTarget {
                    workspace_id,
                })
            }
        };
        let mut outcome = ClientShellInput::default();
        if !self.push_endpoint_method_with_kind(method, PendingEndpointKind::Generic, &mut outcome)
        {
            self.follow_blocked.abandon_unconfirmed();
        }
        outcome.actions
    }
}
