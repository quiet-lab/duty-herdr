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

/// The tab that names the workspace: its first tab.
pub(crate) fn first_tab<'a>(
    snapshot: &'a ClientShellSnapshot,
    workspace: &ClientShellWorkspace,
) -> Option<&'a ClientShellTab> {
    snapshot
        .tabs
        .iter()
        .find(|tab| tab.workspace_id == workspace.workspace_id)
}

/// The active tab of `workspace` when it is not the first one.
pub(crate) fn active_later_tab<'a>(
    snapshot: &'a ClientShellSnapshot,
    workspace: &ClientShellWorkspace,
    first: Option<&ClientShellTab>,
) -> Option<&'a ClientShellTab> {
    if first.is_some_and(|first| first.tab_id == workspace.active_tab_id) {
        return None;
    }
    snapshot.tabs.iter().find(|tab| {
        tab.tab_id == workspace.active_tab_id && tab.workspace_id == workspace.workspace_id
    })
}

/// Label of `tab` by the shared rule; `symbol` is the workspace symbol.
pub(crate) fn tab_label<'a>(symbol: Option<&str>, tab: &'a ClientShellTab) -> Cow<'a, str> {
    display_label(
        tab.custom_label.then_some(tab.label.as_str()),
        tab_has_agent(tab),
        tab_facts(symbol, tab).auto_label(),
        &tab.label,
    )
}

/// Whether `tab` carries the facts of the automatic label, i.e. the server
/// sends them.
pub(crate) fn tab_has_facts(tab: &ClientShellTab) -> bool {
    tab.place.is_some() || tab_app(tab).is_some()
}

/// Facts and label of a workspace, taken from its first tab.
pub(crate) struct WorkspaceLabel<'a> {
    pub(crate) facts: LabelFacts<'a>,
    pub(crate) auto_label: Option<String>,
    pub(crate) agent_alive: bool,
}

impl<'a> WorkspaceLabel<'a> {
    pub(crate) fn new(
        workspace: &'a ClientShellWorkspace,
        first: Option<&'a ClientShellTab>,
    ) -> Self {
        let symbol = workspace_symbol(workspace);
        let facts = first.map_or(
            LabelFacts {
                symbol,
                ..LabelFacts::default()
            },
            |tab| tab_facts(symbol, tab),
        );
        Self {
            facts,
            auto_label: facts.auto_label(),
            agent_alive: first.is_some_and(tab_has_agent),
        }
    }

    /// The `workspace` token: the manual label while an agent is alive in the
    /// first tab, otherwise the automatic label, otherwise `fallback`.
    pub(crate) fn display<'b>(
        &self,
        workspace: &'b ClientShellWorkspace,
        fallback: &'b str,
    ) -> Cow<'b, str> {
        display_label(
            workspace.custom_label.then_some(workspace.label.as_str()),
            self.agent_alive,
            self.auto_label.clone(),
            fallback,
        )
    }
}

/// Label of `workspace` for chrome outside the spaces panel.
pub(crate) fn workspace_label<'a>(
    snapshot: &'a ClientShellSnapshot,
    workspace: &'a ClientShellWorkspace,
) -> Cow<'a, str> {
    WorkspaceLabel::new(workspace, first_tab(snapshot, workspace))
        .display(workspace, &workspace.label)
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
