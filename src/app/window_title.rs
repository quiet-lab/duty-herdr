//! Outer terminal window title.
//!
//! Herdr is a terminal emulator, so `OSC 0`/`OSC 2` written by a pane stops at
//! Herdr and never reaches the terminal Herdr itself runs in. Without this the
//! host window title keeps whatever the shell or `ssh` left behind, which is
//! what window managers show in tab and group bars.
//!
//! The title is rendered on the server so `{hostname}` names the host the panes
//! actually live on, not the machine a thin remote client runs on. The server
//! pushes the result to the foreground client, which writes the `OSC 0`.

use super::App;
use crate::config::{WindowTitlePart, WindowTitleTemplate, WindowTitleToken};

impl App {
    pub(crate) fn configure_window_title(&mut self, template: &str) {
        self.window_title_template =
            WindowTitleTemplate::parse(template)
                .ok()
                .flatten()
                .map(|template| {
                    // Resolve the hostname once here rather than per render.
                    let hostname = if template.uses(WindowTitleToken::Hostname) {
                        crate::platform::hostname().unwrap_or_default()
                    } else {
                        String::new()
                    };
                    (template, hostname)
                });
    }

    /// Whether `ui.window_title` asks Herdr to own the outer terminal title at
    /// all. When it does not, Herdr leaves whatever the shell or `ssh` set.
    pub(crate) fn window_title_configured(&self) -> bool {
        self.window_title_template.is_some()
    }

    /// Whether the title depends on the focused pane's own terminal title, which
    /// is the one input that arrives through PTY parsing rather than app state.
    pub(crate) fn window_title_uses_terminal_title(&self) -> bool {
        self.window_title_template
            .as_ref()
            .is_some_and(|(template, _)| template.uses(WindowTitleToken::TerminalTitle))
    }

    /// Renders the configured outer window title, or `None` when window titles
    /// are disabled or every token resolved empty.
    pub(crate) fn window_title(&self) -> Option<String> {
        let target = self.state.active.and_then(|workspace_index| {
            self.state
                .workspaces
                .get(workspace_index)
                .map(|workspace| (workspace_index, workspace.active_tab_index()))
        });
        self.window_title_for_target(target)
    }

    pub(crate) fn window_title_for(
        &self,
        workspace_index: usize,
        tab_index: usize,
    ) -> Option<String> {
        self.window_title_for_target(Some((workspace_index, tab_index)))
    }

    fn window_title_for_target(&self, target: Option<(usize, usize)>) -> Option<String> {
        let (template, hostname) = self.window_title_template.as_ref()?;
        let tab = target.and_then(|(workspace_index, tab_index)| {
            self.state
                .workspaces
                .get(workspace_index)?
                .tabs
                .get(tab_index)
        });
        let terminal = tab
            .and_then(|tab| tab.terminal_id(tab.layout.focused()))
            .and_then(|terminal_id| self.state.terminals.get(terminal_id));

        let mut title = String::new();
        for part in template.parts() {
            match part {
                WindowTitlePart::Literal(literal) => title.push_str(literal),
                WindowTitlePart::Token(WindowTitleToken::Hostname) => title.push_str(hostname),
                WindowTitlePart::Token(WindowTitleToken::Workspace) => {
                    if let Some(name) = target.and_then(|(workspace_index, _)| {
                        self.workspace_title_label(workspace_index)
                    }) {
                        title.push_str(&name);
                    }
                }
                WindowTitlePart::Token(WindowTitleToken::Tab) => {
                    if let Some(name) = target.and_then(|(workspace_index, tab_index)| {
                        self.tab_title_label(workspace_index, tab_index)
                    }) {
                        title.push_str(&name);
                    }
                }
                WindowTitlePart::Token(WindowTitleToken::Pane) => {
                    if let Some(label) =
                        terminal.and_then(|terminal| terminal.manual_label.as_deref())
                    {
                        title.push_str(label);
                    }
                }
                WindowTitlePart::Token(WindowTitleToken::TerminalTitle) => {
                    if let Some(terminal_title) =
                        terminal.and_then(|terminal| terminal.terminal_title_stripped())
                    {
                        title.push_str(&terminal_title);
                    }
                }
            }
        }

        Some(title)
    }

    /// Workspace label by the shared rule of `crate::auto_label`, as in the
    /// spaces panel: the manual workspace label while an agent is detected in
    /// the first tab, otherwise `<symbol> <place> · <app>` of the first tab.
    /// Reads cached state of the first tab only, like `tab_title_label`.
    pub(crate) fn workspace_title_label(&self, workspace_index: usize) -> Option<String> {
        let workspace = self.state.workspaces.get(workspace_index)?;
        let fallback = workspace.display_name_from_terminals(&self.state.terminals);
        let Some(tab) = workspace.tabs.first() else {
            return Some(fallback);
        };
        let terminal_of = |pane_id| {
            tab.terminal_id(pane_id)
                .and_then(|terminal_id| self.state.terminals.get(terminal_id))
        };
        let focused = tab.layout.focused();
        let focused_terminal = terminal_of(focused);
        // The focused pane answers first, then the other panes of the tab.
        let agent = std::iter::once(focused)
            .chain(
                tab.layout
                    .pane_ids()
                    .into_iter()
                    .filter(|pane_id| *pane_id != focused),
            )
            .filter_map(terminal_of)
            .find_map(first_tab_agent_label);
        let symbol = workspace.effective_symbol(&self.state.workspace_symbols);
        let place = workspace.place_label();
        let facts = crate::auto_label::LabelFacts::new(
            symbol.as_deref(),
            Some(&place),
            agent.as_deref(),
            focused_terminal.and_then(|terminal| terminal.foreground_process.as_deref()),
        );
        Some(
            crate::auto_label::display_label(
                workspace.custom_name.as_deref(),
                agent.is_some(),
                facts.auto_label(),
                &fallback,
            )
            .into_owned(),
        )
    }

    /// Tab label by the shared rule of `crate::auto_label`: the manual tab
    /// label while an agent is detected in the tab, otherwise
    /// `<symbol> <place> · <app>`. Reads cached state of this one tab only:
    /// terminal metadata, the cached workspace Git identity, and the cwd last
    /// admitted to `TerminalState`; no process or filesystem reads.
    pub(crate) fn tab_title_label(
        &self,
        workspace_index: usize,
        tab_index: usize,
    ) -> Option<String> {
        let workspace = self.state.workspaces.get(workspace_index)?;
        let tab = workspace.tabs.get(tab_index)?;
        let fallback = workspace.tab_display_name(tab_index)?;
        let terminal_of = |pane_id| {
            tab.terminal_id(pane_id)
                .and_then(|terminal_id| self.state.terminals.get(terminal_id))
        };
        let agent_of = |terminal: &crate::terminal::TerminalState| {
            terminal
                .is_agent_terminal()
                .then(|| {
                    terminal
                        .effective_presentation()
                        .display_agent
                        .or_else(|| terminal.effective_agent_label().map(str::to_owned))
                })
                .flatten()
                .filter(|label| !label.is_empty())
        };
        let focused = tab.layout.focused();
        let focused_terminal = terminal_of(focused);
        // The focused pane answers first; other panes of this tab are looked at
        // only when it runs no agent.
        let agent = focused_terminal.and_then(agent_of).or_else(|| {
            tab.layout
                .pane_ids()
                .into_iter()
                .filter(|pane_id| *pane_id != focused)
                .find_map(|pane_id| terminal_of(pane_id).and_then(agent_of))
        });
        let place = if tab_index == 0 {
            Some(workspace.place_label())
        } else {
            terminal_of(tab.root_pane).map(|terminal| workspace.place_label_for(&terminal.cwd))
        };
        let symbol = workspace.effective_symbol(&self.state.workspace_symbols);
        let facts = crate::auto_label::LabelFacts::new(
            symbol.as_deref(),
            place.as_deref(),
            agent.as_deref(),
            focused_terminal.and_then(|terminal| terminal.foreground_process.as_deref()),
        );
        Some(
            crate::auto_label::display_label(
                tab.custom_name.as_deref(),
                agent.is_some(),
                facts.auto_label(),
                &fallback,
            )
            .into_owned(),
        )
    }
}

/// Agent shown for a terminal of the first tab: its display agent, otherwise
/// its agent label, while the terminal runs an agent.
fn first_tab_agent_label(terminal: &crate::terminal::TerminalState) -> Option<String> {
    terminal
        .is_agent_terminal()
        .then(|| {
            terminal
                .effective_presentation()
                .display_agent
                .or_else(|| terminal.effective_agent_label().map(str::to_owned))
        })
        .flatten()
        .filter(|label| !label.is_empty())
}

#[cfg(test)]
mod tests {
    use crate::app::App;
    use crate::config::Config;
    use crate::workspace::Workspace;

    fn test_app() -> App {
        let event_hub = crate::api::EventHub::default();
        let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::new(
            &Config::default(),
            crate::app::AppPolicy::TEST,
            None,
            api_rx,
            event_hub,
        );
        app.state.workspaces = vec![Workspace::test_new("herd")];
        app.state.active = Some(0);
        app.state.ensure_test_terminals();
        app
    }

    fn root_terminal(app: &mut App, tab: usize) -> &mut crate::terminal::TerminalState {
        let tab = &app.state.workspaces[0].tabs[tab];
        let terminal_id = tab.terminal_id(tab.root_pane).unwrap().clone();
        app.state.terminals.get_mut(&terminal_id).unwrap()
    }

    #[test]
    fn renders_workspace_and_tab_names() {
        let mut app = test_app();
        app.configure_window_title("{workspace}/{tab}");
        app.state.workspaces[0].cached_identity_cwd = "/srv/herd".into();
        app.state.workspaces[0].cached_git_space = None;

        // Without an agent the manual workspace and tab labels are hidden:
        // both show the place and the foreground process.
        assert_eq!(app.window_title().as_deref(), Some("/srv/herd//srv/herd"));
        root_terminal(&mut app, 0).set_foreground_process(Some("nvim".into()));
        assert_eq!(
            app.window_title().as_deref(),
            Some("/srv/herd · nvim//srv/herd · nvim")
        );

        // The manual labels show only while an agent is detected in the tab.
        app.state.workspaces[0].tabs[0].custom_name = Some("build".into());
        assert_eq!(
            app.window_title().as_deref(),
            Some("/srv/herd · nvim//srv/herd · nvim")
        );
        root_terminal(&mut app, 0).set_detected_state(
            Some(crate::detect::Agent::Pi),
            crate::detect::AgentState::Working,
        );
        assert_eq!(app.window_title().as_deref(), Some("herd/build"));
        app.state.workspaces[0].tabs[0].custom_name = None;
        let title = app.window_title().unwrap();
        assert!(title.starts_with("herd//srv/herd · "), "{title}");
        assert!(!title.ends_with("nvim"), "{title}");

        // Without a manual workspace label the agent names the workspace.
        app.state.workspaces[0].custom_name = None;
        let title = app.window_title().unwrap();
        assert!(title.starts_with("/srv/herd · "), "{title}");
        assert!(!title.contains("nvim"), "{title}");
    }

    #[test]
    fn later_tab_title_uses_its_root_pane_place_and_workspace_symbol() {
        let config: Config = toml::from_str(
            r#"
[ui.workspace_symbols.outside]
main = "S"
"#,
        )
        .unwrap();
        let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::new(
            &config,
            crate::app::AppPolicy::TEST,
            None,
            api_rx,
            crate::api::EventHub::default(),
        );
        let mut workspace = Workspace::test_new("herd");
        workspace.test_add_tab(None);
        workspace.cached_identity_cwd = "/srv/herd".into();
        workspace.cached_git_space = None;
        app.state.workspaces = vec![workspace];
        app.state.active = Some(0);
        app.state.ensure_test_terminals();
        app.configure_window_title("{tab}");
        root_terminal(&mut app, 1).cwd = "/srv/notes".into();
        root_terminal(&mut app, 1).set_foreground_process(Some("htop".into()));

        assert_eq!(app.window_title_for(0, 0).as_deref(), Some("S /srv/herd"));
        assert_eq!(
            app.window_title_for(0, 1).as_deref(),
            Some("S /srv/notes · htop")
        );

        // The workspace token always names the first tab, whichever is active.
        app.configure_window_title("{workspace}|{tab}");
        root_terminal(&mut app, 0).set_foreground_process(Some("nvim".into()));
        assert_eq!(
            app.window_title_for(0, 1).as_deref(),
            Some("S /srv/herd · nvim|S /srv/notes · htop")
        );
    }

    #[test]
    fn renders_focused_pane_label_and_terminal_title() {
        let mut app = test_app();
        app.configure_window_title("{pane}|{terminal_title}");

        let pane_id = app.state.workspaces[0].tabs[0].root_pane;
        let terminal_id = app.state.workspaces[0].tabs[0].panes[&pane_id]
            .attached_terminal_id
            .clone();
        let terminal = app
            .state
            .terminals
            .get_mut(&terminal_id)
            .expect("focused terminal");
        terminal.manual_label = Some("api".into());
        terminal.set_terminal_title(Some("⠋ building".into()));

        assert_eq!(app.window_title().as_deref(), Some("api|building"));
    }

    #[test]
    fn empty_template_disables_window_titles() {
        let mut app = test_app();
        app.configure_window_title("");

        assert_eq!(app.window_title(), None);
    }

    #[test]
    fn invalid_template_disables_window_titles() {
        let mut app = test_app();
        app.configure_window_title("{nope}");

        assert_eq!(app.window_title(), None);
    }

    #[test]
    fn unset_tokens_render_empty() {
        let mut app = test_app();
        app.configure_window_title("[{pane}]");

        assert_eq!(app.window_title().as_deref(), Some("[]"));
    }
}
