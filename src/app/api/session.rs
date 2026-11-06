use crate::api::schema::{PaneInfo, ResponseResult, SessionSnapshot};
use crate::app::App;

use super::responses::encode_success;

impl App {
    pub(super) fn handle_session_snapshot(&mut self, id: String) -> String {
        encode_success(
            id,
            ResponseResult::SessionSnapshot {
                snapshot: Box::new(self.session_snapshot()),
            },
        )
    }

    pub(crate) fn session_snapshot(&self) -> SessionSnapshot {
        self.session_snapshot_with_pane_info(Self::pane_info, Layouts::Include)
    }

    #[cfg(test)]
    pub(crate) fn session_metadata_snapshot(&self) -> SessionSnapshot {
        self.session_snapshot_with_pane_info(Self::pane_metadata, Layouts::Include)
    }

    /// `session_metadata_snapshot` with empty `layouts`: the client shell
    /// snapshot does not carry pane layouts, and computing them is a large
    /// share of its construction.
    pub(crate) fn session_metadata_snapshot_without_layouts(&self) -> SessionSnapshot {
        self.session_snapshot_with_pane_info(Self::pane_metadata, Layouts::Omit)
    }

    fn session_snapshot_with_pane_info(
        &self,
        pane_info: fn(&Self, usize, crate::layout::PaneId) -> Option<PaneInfo>,
        layouts_policy: Layouts,
    ) -> SessionSnapshot {
        let focused_workspace_id = self
            .state
            .active
            .map(|ws_idx| self.public_workspace_id(ws_idx));
        let focused_tab_id = self.state.active.and_then(|ws_idx| {
            let ws = self.state.workspaces.get(ws_idx)?;
            self.public_tab_id(ws_idx, ws.active_tab)
        });
        let focused_pane_id = self.state.active.and_then(|ws_idx| {
            let ws = self.state.workspaces.get(ws_idx)?;
            self.public_pane_id(ws_idx, ws.focused_pane_id()?)
        });

        let mut workspaces = Vec::new();
        let mut tabs = Vec::new();
        let mut layouts = Vec::new();
        let mut panes = Vec::new();
        // Same panes in the same order as `collect_agent_infos`; an agent
        // reuses the metadata built for its pane instead of building it again.
        let mut agents = Vec::new();
        for (ws_idx, ws) in self.state.workspaces.iter().enumerate() {
            workspaces.push(self.workspace_info(ws_idx));
            for (tab_idx, tab_state) in ws.tabs.iter().enumerate() {
                tabs.extend(self.tab_info(ws_idx, tab_idx));
                if layouts_policy == Layouts::Include {
                    layouts.extend(self.pane_layout_snapshot(ws_idx, tab_idx));
                }
                for pane_id in tab_state.layout.pane_ids() {
                    let Some(pane) = pane_info(self, ws_idx, pane_id) else {
                        continue;
                    };
                    if let Some(terminal) = self.agent_terminal_for_pane(ws_idx, pane_id) {
                        agents.push(crate::app::agents::agent_info_from_pane(terminal, &pane));
                    }
                    panes.push(pane);
                }
            }
        }

        SessionSnapshot {
            version: crate::build_info::version(),
            protocol: crate::protocol::PROTOCOL_VERSION,
            focused_workspace_id,
            focused_tab_id,
            focused_pane_id,
            workspaces,
            tabs,
            panes,
            layouts,
            agents,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Layouts {
    Include,
    Omit,
}

#[cfg(test)]
mod tests {
    use crate::api::schema::{EmptyParams, Method, ResponseResult, SuccessResponse};
    use crate::{config::Config, workspace::Workspace};

    fn app_with_two_tabs() -> crate::app::App {
        let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = crate::app::App::new(
            &Config::default(),
            crate::app::AppPolicy::TEST,
            None,
            api_rx,
            crate::api::EventHub::default(),
        );
        let mut workspace = Workspace::test_new("snapshot");
        workspace.test_add_tab(None);
        app.state.workspaces = vec![workspace];
        app.state.ensure_test_terminals();
        app.state.active = Some(0);
        app
    }

    #[test]
    fn session_snapshot_bootstraps_runtime_resources() {
        let mut app = app_with_two_tabs();
        let response = app.handle_api_request(crate::api::schema::Request {
            id: "req_snapshot".into(),
            method: Method::SessionSnapshot(EmptyParams::default()),
        });

        let success: SuccessResponse = serde_json::from_str(&response).unwrap();
        let ResponseResult::SessionSnapshot { snapshot } = success.result else {
            panic!("expected session snapshot response");
        };
        assert_eq!(success.id, "req_snapshot");
        assert_eq!(snapshot.workspaces.len(), 1);
        assert_eq!(snapshot.tabs.len(), 2);
        assert_eq!(snapshot.panes.len(), 2);
        assert_eq!(snapshot.layouts.len(), 2);
        assert_eq!(
            snapshot.focused_workspace_id.as_deref(),
            Some(snapshot.workspaces[0].workspace_id.as_str())
        );
        assert_eq!(
            snapshot.focused_tab_id.as_deref(),
            Some(snapshot.tabs[0].tab_id.as_str())
        );
        assert_eq!(
            snapshot.focused_pane_id.as_deref(),
            Some(snapshot.panes[0].pane_id.as_str())
        );
    }
}
