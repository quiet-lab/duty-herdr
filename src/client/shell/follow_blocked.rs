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
//! - The queue holds dialogs in the order they entered `blocked` (FIFO; panes entering in one
//!   snapshot are ordered by `state_change_seq`). A pane that stays `blocked` keeps its place and, when followed,
//!   stays followed even if its sequence changes: a card of several questions stays
//!   `blocked` between them, so only leaving `blocked` (or vanishing from the snapshot)
//!   is an answer.
//! - The first snapshot of an endpoint boot is a baseline: dialogs already open are queued
//!   without switching; a dialog in the focused workspace becomes the followed pane.
//! - Only two events switch: a pane enters `blocked`, or the followed pane leaves it.
//! - While the active workspace holds a dialog nothing switches; that dialog becomes the
//!   followed pane, so answering it moves on.
//! - Otherwise the client focuses the queue head (or the input pane its `input_pane` token
//!   names in the same tab) and remembers the workspace it left as the origin of the chain
//!   (only on the first switch of a chain).
//! - When the followed pane leaves `blocked` and the queue is empty, the client returns to the
//!   origin if it still exists.
//! - When the user focuses another workspace than the followed one (before the switch is
//!   confirmed: another workspace than both the followed one and the one the request left),
//!   the chain ends (no follow-up switch, no return); the queue continues from the next entry
//!   into `blocked`.
//! - A switch that could not be sent, was cancelled or was rejected ends its chain quietly.

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
    /// Workspace focused when the client requested the switch; `None` once confirmed or
    /// when the client adopted a dialog the user was already looking at.
    requested_from: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Dialog {
    workspace_id: String,
}

#[derive(Debug, Default)]
pub(super) struct FollowBlocked {
    scope: Option<(ClientEndpointId, String)>,
    /// Current dialogs by pane ID.
    blocked: HashMap<String, Dialog>,
    queue: VecDeque<String>,
    followed: Option<FollowedPane>,
    origin_workspace_id: Option<String>,
    /// Endpoint request ID of the unconfirmed focus switch.
    pending_request_id: Option<String>,
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

    /// The focus switch was not sent, was cancelled or was rejected: forget it and the
    /// chain it started, so the next event can try again.
    fn abandon_unconfirmed(&mut self) {
        self.pending_request_id = None;
        if self
            .followed
            .as_ref()
            .is_some_and(|followed| !followed.confirmed)
        {
            self.followed = None;
            self.origin_workspace_id = None;
        }
    }

    pub(super) fn request_failed(&mut self, request_id: &str) -> bool {
        if self.pending_request_id.as_deref() != Some(request_id) {
            return false;
        }
        self.abandon_unconfirmed();
        true
    }

    /// Follows the dialog of the focused workspace that entered `blocked` first.
    fn adopt_focused_dialog(&mut self, focused: Option<&str>) -> bool {
        let Some((pane_id, workspace_id)) = self.queue.iter().find_map(|pane_id| {
            self.blocked
                .get(pane_id)
                .filter(|dialog| Some(dialog.workspace_id.as_str()) == focused)
                .map(|dialog| (pane_id.clone(), dialog.workspace_id.clone()))
        }) else {
            return false;
        };
        self.followed = Some(FollowedPane {
            pane_id,
            workspace_id,
            confirmed: true,
            requested_from: None,
        });
        if self.origin_workspace_id.is_none() {
            self.origin_workspace_id = focused.map(str::to_owned);
        }
        true
    }

    /// Updates the queue from the active endpoint snapshot and returns the focus change to
    /// request, if any.
    pub(super) fn observe(
        &mut self,
        endpoint_id: &ClientEndpointId,
        snapshot: &ClientShellSnapshot,
    ) -> Option<FollowBlockedFocus> {
        let same_scope = self
            .scope
            .as_ref()
            .is_some_and(|(endpoint, boot)| endpoint == endpoint_id && boot == &snapshot.boot_id);
        // Most snapshots change titles, focus or output with no dialog open: skip them
        // without collecting or allocating.
        if same_scope
            && self.blocked.is_empty()
            && !snapshot
                .agents
                .iter()
                .any(|agent| agent.agent_status == crate::api::schema::AgentStatus::Blocked)
        {
            return None;
        }
        let mut dialogs = dialog_panes(snapshot);
        dialogs.sort_by(|left, right| left.2.cmp(&right.2).then(left.0.cmp(right.0)));
        let focused = snapshot.focused_workspace_id.as_deref();
        if !same_scope {
            self.reset();
            self.scope = Some((endpoint_id.clone(), snapshot.boot_id.clone()));
            for (pane_id, workspace_id, _) in dialogs {
                self.blocked.insert(
                    pane_id.to_owned(),
                    Dialog {
                        workspace_id: workspace_id.to_owned(),
                    },
                );
                self.queue.push_back(pane_id.to_owned());
            }
            // Baseline: open dialogs queue without switching; one the user is looking at
            // is followed, so answering it moves on.
            self.adopt_focused_dialog(focused);
            return None;
        }

        let unchanged = dialogs.len() == self.blocked.len()
            && dialogs.iter().all(|(pane_id, workspace_id, _)| {
                self.blocked
                    .get(*pane_id)
                    .is_some_and(|dialog| dialog.workspace_id == *workspace_id)
            });
        let mut entered = false;
        let mut answered = false;
        if !unchanged {
            let mut current = HashMap::with_capacity(dialogs.len());
            for (pane_id, workspace_id, _) in dialogs {
                // A pane already blocked keeps its place whatever its sequence did: a card
                // of several questions changes it between questions.
                if !self.blocked.contains_key(pane_id) {
                    entered = true;
                    self.queue.push_back(pane_id.to_owned());
                }
                current.insert(
                    pane_id.to_owned(),
                    Dialog {
                        workspace_id: workspace_id.to_owned(),
                    },
                );
            }
            self.blocked = current;
            let blocked = &self.blocked;
            self.queue.retain(|pane_id| blocked.contains_key(pane_id));
        }

        if let Some(followed) = self.followed.as_mut() {
            match self.blocked.get(&followed.pane_id) {
                None => {
                    self.followed = None;
                    self.pending_request_id = None;
                    answered = true;
                }
                Some(dialog) => {
                    followed.workspace_id.clone_from(&dialog.workspace_id);
                    let on_dialog = focused == Some(followed.workspace_id.as_str());
                    // Before confirmation, a focus that is neither the dialog nor the
                    // workspace the request left is the user's own choice.
                    let user_moved = if followed.confirmed {
                        !on_dialog
                    } else {
                        !on_dialog && focused != followed.requested_from.as_deref()
                    };
                    if on_dialog {
                        followed.confirmed = true;
                        followed.requested_from = None;
                        self.pending_request_id = None;
                    } else if user_moved {
                        self.followed = None;
                        self.origin_workspace_id = None;
                        self.pending_request_id = None;
                    }
                }
            }
        }
        if !(entered || answered) || self.followed.is_some() {
            return None;
        }

        if self.adopt_focused_dialog(focused) {
            // The user is looking at a dialog: wait for its answer.
            return None;
        }

        if let Some((pane_id, workspace_id)) = self.queue.iter().find_map(|pane_id| {
            self.blocked
                .get(pane_id)
                .map(|dialog| (pane_id.clone(), dialog.workspace_id.clone()))
        }) {
            if self.origin_workspace_id.is_none() {
                self.origin_workspace_id = focused.map(str::to_owned);
            }
            self.followed = Some(FollowedPane {
                pane_id: pane_id.clone(),
                workspace_id,
                confirmed: false,
                requested_from: focused.map(str::to_owned),
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
                    pane_id: super::input_focus::input_pane_for_target(snapshot, &pane_id)
                        .map_or(pane_id.clone(), str::to_owned),
                })
            }
            FollowBlockedFocus::Workspace(workspace_id) => {
                crate::api::schema::Method::WorkspaceFocus(crate::api::schema::WorkspaceTarget {
                    workspace_id,
                })
            }
        };
        let request_id = format!("client-shell:{}", self.next_request_id);
        let mut outcome = ClientShellInput::default();
        if self.push_endpoint_method_with_kind(method, PendingEndpointKind::Generic, &mut outcome) {
            self.follow_blocked.pending_request_id = Some(request_id);
        } else {
            self.follow_blocked.abandon_unconfirmed();
        }
        outcome.actions
    }
}
