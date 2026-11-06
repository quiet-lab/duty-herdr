//! Workspace and tab labels composed from snapshot facts.
//!
//! One rule serves the spaces panel, the agent panel, the tab bar, and the
//! mobile chrome: a manual label is shown only while an agent is detected in
//! the tab (for a workspace: in its first tab); otherwise the automatic label
//! `<symbol> <place> · <app>` is shown. A server that predates these facts
//! sends no place and no app, and the existing label is kept.

use std::borrow::Cow;

pub(crate) use crate::auto_label::LabelFacts;
use crate::auto_label::{display_label, non_empty};
use crate::protocol::{ClientShellSnapshot, ClientShellTab, ClientShellWorkspace};

/// Workspace metadata token holding the workspace symbol.
pub(crate) const SYMBOL_TOKEN: &str = "symbol";

/// Facts of `tab`; the symbol belongs to the tab's workspace.
pub(crate) fn tab_facts<'a>(symbol: Option<&'a str>, tab: &'a ClientShellTab) -> LabelFacts<'a> {
    LabelFacts::new(
        symbol,
        tab.place.as_deref(),
        tab.agent_label.as_deref(),
        tab.foreground_process.as_deref(),
    )
}

/// Application of a tab: the detected agent while there is one, otherwise the
/// foreground process of the tab's focused pane.
pub(crate) fn tab_app(tab: &ClientShellTab) -> Option<&str> {
    tab_facts(None, tab).app
}

/// Whether an agent is detected in `tab`.
pub(crate) fn tab_has_agent(tab: &ClientShellTab) -> bool {
    non_empty(tab.agent_label.as_deref()).is_some()
}

pub(crate) fn workspace_symbol(workspace: &ClientShellWorkspace) -> Option<&str> {
    workspace
        .tokens
        .iter()
        .find(|(key, _)| key == SYMBOL_TOKEN)
        .and_then(|(_, value)| non_empty(Some(value.as_str())))
}

/// Label of `tab` by the shared rule; `symbol` is the workspace symbol.
pub(crate) fn tab_label<'a>(symbol: Option<&str>, tab: &'a ClientShellTab) -> Cow<'a, str> {
    display_label(
        tab.custom_label.then_some(tab.label.as_str()),
        tab_has_agent(tab),
        tab_facts(symbol, tab).auto_label().map(Cow::Owned),
        &tab.label,
    )
}

/// Whether `tab` carries the facts of the automatic label, i.e. the server
/// sends them.
pub(crate) fn tab_has_facts(tab: &ClientShellTab) -> bool {
    tab.place.is_some() || tab_app(tab).is_some()
}

/// Whether the `tab` token names `tab` at all: a lone tab without a manual
/// label repeats its workspace and is left out.
pub(crate) fn tab_token_shown(tab: &ClientShellTab, workspace_tab_count: usize) -> bool {
    tab.custom_label || workspace_tab_count > 1
}

/// Tabs of one workspace as positions in `ClientShellSnapshot::tabs`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct WorkspaceTabs {
    first: Option<usize>,
    /// The active tab when it is not the first one.
    active_later: Option<usize>,
    count: usize,
}

impl WorkspaceTabs {
    fn scan(snapshot: &ClientShellSnapshot, workspace: &ClientShellWorkspace) -> Self {
        let mut tabs = Self::default();
        for (index, tab) in snapshot.tabs.iter().enumerate() {
            if tab.workspace_id == workspace.workspace_id {
                tabs.add(index, tab, workspace);
            }
        }
        tabs
    }

    fn add(&mut self, index: usize, tab: &ClientShellTab, workspace: &ClientShellWorkspace) {
        self.count += 1;
        if self.first.is_none() {
            self.first = Some(index);
        } else if tab.tab_id == workspace.active_tab_id {
            self.active_later = Some(index);
        }
    }
}

/// Tabs indexed by workspace and the automatic tab labels of one snapshot.
/// Built once when the snapshot is received, so render paths borrow labels
/// instead of scanning tabs and formatting per frame.
#[derive(Clone, Debug, Default)]
pub(crate) struct SnapshotLabels {
    boot_id: String,
    revision: u64,
    /// Parallel to `ClientShellSnapshot::workspaces`.
    workspaces: Vec<WorkspaceTabs>,
    /// Parallel to `ClientShellSnapshot::tabs`; the symbol is the workspace's.
    tab_auto_labels: Vec<Option<String>>,
}

impl SnapshotLabels {
    pub(crate) fn new(snapshot: &ClientShellSnapshot) -> Self {
        let workspace_index = snapshot
            .workspaces
            .iter()
            .enumerate()
            .map(|(index, workspace)| (workspace.workspace_id.as_str(), index))
            .collect::<std::collections::HashMap<_, _>>();
        let mut workspaces = vec![WorkspaceTabs::default(); snapshot.workspaces.len()];
        let tab_auto_labels = snapshot
            .tabs
            .iter()
            .enumerate()
            .map(|(index, tab)| {
                let workspace =
                    workspace_index
                        .get(tab.workspace_id.as_str())
                        .map(|&workspace_index| {
                            let workspace = &snapshot.workspaces[workspace_index];
                            workspaces[workspace_index].add(index, tab, workspace);
                            workspace
                        });
                tab_facts(workspace.and_then(workspace_symbol), tab).auto_label()
            })
            .collect();
        Self {
            boot_id: snapshot.boot_id.clone(),
            revision: snapshot.revision,
            workspaces,
            tab_auto_labels,
        }
    }

    fn describes(&self, snapshot: &ClientShellSnapshot) -> bool {
        self.revision == snapshot.revision
            && self.workspaces.len() == snapshot.workspaces.len()
            && self.tab_auto_labels.len() == snapshot.tabs.len()
            && self.boot_id == snapshot.boot_id
    }
}

/// A snapshot with its [`SnapshotLabels`] when they describe it; without them
/// every lookup falls back to scanning the snapshot.
#[derive(Clone, Copy)]
pub(crate) struct LabeledSnapshot<'a> {
    pub(crate) snapshot: &'a ClientShellSnapshot,
    labels: Option<&'a SnapshotLabels>,
}

impl<'a> LabeledSnapshot<'a> {
    pub(crate) fn new(
        snapshot: &'a ClientShellSnapshot,
        labels: Option<&'a SnapshotLabels>,
    ) -> Self {
        Self {
            snapshot,
            labels: labels.filter(|labels| labels.describes(snapshot)),
        }
    }

    /// The snapshot with the labels its endpoint cached for it, if any.
    pub(crate) fn of_endpoint(
        snapshot: &'a ClientShellSnapshot,
        endpoints: &'a [super::ClientShellEndpoint],
        endpoint_id: &super::ClientEndpointId,
    ) -> Self {
        Self::new(
            snapshot,
            endpoints
                .iter()
                .find(|endpoint| &endpoint.endpoint_id == endpoint_id)
                .map(|endpoint| &endpoint.labels),
        )
    }

    fn workspace_tabs(self, workspace_index: usize) -> WorkspaceTabs {
        match self.labels {
            Some(labels) => labels
                .workspaces
                .get(workspace_index)
                .copied()
                .unwrap_or_default(),
            None => self
                .snapshot
                .workspaces
                .get(workspace_index)
                .map(|workspace| WorkspaceTabs::scan(self.snapshot, workspace))
                .unwrap_or_default(),
        }
    }

    pub(crate) fn tab_count(self, workspace_index: usize) -> usize {
        self.workspace_tabs(workspace_index).count
    }

    /// The active tab of the workspace when it is not the first one.
    pub(crate) fn active_later_tab(self, workspace_index: usize) -> Option<usize> {
        self.workspace_tabs(workspace_index).active_later
    }

    fn tab_auto_label(self, tab_index: usize) -> Option<Cow<'a, str>> {
        match self.labels {
            Some(labels) => labels
                .tab_auto_labels
                .get(tab_index)?
                .as_deref()
                .map(Cow::Borrowed),
            None => {
                let tab = self.snapshot.tabs.get(tab_index)?;
                let symbol = self
                    .snapshot
                    .workspaces
                    .iter()
                    .find(|workspace| workspace.workspace_id == tab.workspace_id)
                    .and_then(workspace_symbol);
                tab_facts(symbol, tab).auto_label().map(Cow::Owned)
            }
        }
    }

    /// Label of the tab at `tab_index` by the shared rule.
    pub(crate) fn tab_label(self, tab_index: usize) -> Cow<'a, str> {
        let Some(tab) = self.snapshot.tabs.get(tab_index) else {
            return Cow::Borrowed("");
        };
        display_label(
            tab.custom_label.then_some(tab.label.as_str()),
            tab_has_agent(tab),
            self.tab_auto_label(tab_index),
            &tab.label,
        )
    }

    /// Facts of the workspace, taken from its first tab.
    pub(crate) fn workspace_facts(self, workspace_index: usize) -> LabelFacts<'a> {
        let Some(workspace) = self.snapshot.workspaces.get(workspace_index) else {
            return LabelFacts::default();
        };
        let symbol = workspace_symbol(workspace);
        match self
            .workspace_tabs(workspace_index)
            .first
            .and_then(|index| self.snapshot.tabs.get(index))
        {
            Some(tab) => tab_facts(symbol, tab),
            None => LabelFacts {
                symbol,
                ..LabelFacts::default()
            },
        }
    }

    /// Automatic label of the workspace: the one of its first tab.
    pub(crate) fn workspace_auto_label(self, workspace_index: usize) -> Option<Cow<'a, str>> {
        self.workspace_tabs(workspace_index)
            .first
            .and_then(|index| self.tab_auto_label(index))
    }

    /// The `workspace` token: the manual label while an agent is alive in the
    /// first tab, otherwise the automatic label of the first tab, otherwise
    /// `fallback`.
    pub(crate) fn workspace_label(self, workspace_index: usize, fallback: &'a str) -> Cow<'a, str> {
        let Some(workspace) = self.snapshot.workspaces.get(workspace_index) else {
            return Cow::Borrowed(fallback);
        };
        let first = self.workspace_tabs(workspace_index).first;
        display_label(
            workspace.custom_label.then_some(workspace.label.as_str()),
            first
                .and_then(|index| self.snapshot.tabs.get(index))
                .is_some_and(tab_has_agent),
            first.and_then(|index| self.tab_auto_label(index)),
            fallback,
        )
    }
}

/// Label of `workspace` for chrome outside the spaces panel.
pub(crate) fn workspace_label<'a>(
    snapshot: &'a ClientShellSnapshot,
    workspace: &'a ClientShellWorkspace,
) -> Cow<'a, str> {
    let labeled = LabeledSnapshot::new(snapshot, None);
    snapshot
        .workspaces
        .iter()
        .position(|candidate| std::ptr::eq(candidate, workspace))
        .map(|index| labeled.workspace_label(index, &workspace.label))
        .unwrap_or_else(|| Cow::Borrowed(&workspace.label))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::schema::AgentStatus;

    fn tab(id: &str, custom: Option<&str>) -> ClientShellTab {
        ClientShellTab {
            tab_id: id.into(),
            workspace_id: "w1".into(),
            number: 1,
            label: custom.unwrap_or("2").into(),
            custom_label: custom.is_some(),
            zoomed: false,
            focused: false,
            agent_status: AgentStatus::Unknown,
            place: None,
            agent_label: None,
            foreground_process: None,
        }
    }

    #[test]
    fn tab_app_prefers_detected_agent_over_foreground_process() {
        let mut tab = tab("t", None);
        tab.foreground_process = Some("nvim".into());
        assert_eq!(tab_app(&tab), Some("nvim"));
        assert!(!tab_has_agent(&tab));
        tab.agent_label = Some("claude".into());
        assert_eq!(tab_app(&tab), Some("claude"));
        assert!(tab_has_agent(&tab));
        tab.agent_label = Some(String::new());
        assert_eq!(tab_app(&tab), Some("nvim"));
    }

    #[test]
    fn tab_label_follows_the_shared_rule() {
        let mut tab = tab("t", Some("review"));
        tab.place = Some("herdr".into());
        tab.foreground_process = Some("nvim".into());
        assert_eq!(tab_label(Some("🦦"), &tab), "🦦 herdr · nvim");
        tab.agent_label = Some("claude".into());
        assert_eq!(tab_label(Some("🦦"), &tab), "review");
        tab.custom_label = false;
        tab.label = "2".into();
        assert_eq!(tab_label(None, &tab), "herdr · claude");
        // A server without label facts keeps the existing label.
        let old = self::tab("t", None);
        assert_eq!(tab_label(Some("🦦"), &old), "2");
        assert!(!tab_has_facts(&old));
    }
}
