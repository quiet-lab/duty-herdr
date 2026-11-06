use crate::config::{
    AgentSidebarToken, AgentsSidebarConfig, SidebarTokenStyle, SpaceSidebarToken,
    SpacesSidebarConfig,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedToken {
    pub kind: ResolvedTokenKind,
    pub style: SidebarTokenStyle,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ResolvedTokenKind {
    StateIcon,
    StateText(String),
    Machine(String),
    Workspace(String),
    Tab(String),
    Pane(String),
    Agent(String),
    TerminalTitle(String),
    Branch(String),
    GitStatus { ahead: usize, behind: usize },
    Custom(String),
}

impl ResolvedTokenKind {
    fn text_value(&self) -> Option<&str> {
        match self {
            Self::StateText(value)
            | Self::Machine(value)
            | Self::Workspace(value)
            | Self::Tab(value)
            | Self::Pane(value)
            | Self::Agent(value)
            | Self::TerminalTitle(value)
            | Self::Branch(value)
            | Self::Custom(value) => Some(value),
            Self::StateIcon | Self::GitStatus { .. } => None,
        }
    }
}

impl ResolvedToken {
    fn new(kind: ResolvedTokenKind, style: SidebarTokenStyle) -> Self {
        Self { kind, style }
    }

    #[cfg(test)]
    pub(super) fn unstyled(kind: ResolvedTokenKind) -> Self {
        Self::new(kind, SidebarTokenStyle::default())
    }
}

pub(crate) struct AgentTokenContext<'a> {
    pub(crate) machine: Option<&'a str>,
    pub(crate) workspace: &'a str,
    pub(crate) tab: Option<&'a str>,
    pub(crate) focused_tab: Option<&'a str>,
    /// Whether the agent's tab (token `tab`) is active in its workspace.
    pub(crate) tab_active: bool,
    pub(crate) pane: Option<&'a str>,
    pub(crate) agent_label: Option<&'a str>,
    pub(crate) terminal_title: Option<&'a str>,
    pub(crate) terminal_title_stripped: Option<&'a str>,
    pub(crate) canonical_agent: Option<crate::detect::Agent>,
    pub(crate) tokens: &'a std::collections::HashMap<String, String>,
    /// Tokens for `$name` colors: the pane's tokens over its workspace's tokens.
    pub(crate) color_tokens: &'a std::collections::HashMap<String, String>,
}

pub(crate) fn agent_rows(
    config: &AgentsSidebarConfig,
    context: AgentTokenContext<'_>,
    state_text: &str,
) -> Vec<Vec<ResolvedToken>> {
    config
        .rows_for_agent(context.canonical_agent)
        .iter()
        .filter_map(|row| {
            let resolved = row
                .iter()
                .filter_map(|configured| {
                    let (token, style) = configured.parts();
                    let kind = match token {
                        AgentSidebarToken::StateIcon => Some(ResolvedTokenKind::StateIcon),
                        AgentSidebarToken::StateText => {
                            Some(ResolvedTokenKind::StateText(state_text.to_string()))
                        }
                        AgentSidebarToken::Machine => context
                            .machine
                            .map(|value| ResolvedTokenKind::Machine(value.to_string())),
                        AgentSidebarToken::Workspace => {
                            Some(ResolvedTokenKind::Workspace(context.workspace.to_string()))
                        }
                        AgentSidebarToken::Tab => context
                            .tab
                            .map(|value| ResolvedTokenKind::Tab(value.to_string())),
                        AgentSidebarToken::FocusedTab => context
                            .focused_tab
                            .map(|value| ResolvedTokenKind::Tab(value.to_string())),
                        AgentSidebarToken::Pane => context
                            .pane
                            .map(|value| ResolvedTokenKind::Pane(value.to_string())),
                        AgentSidebarToken::Agent => context
                            .agent_label
                            .map(|value| ResolvedTokenKind::Agent(value.to_string())),
                        AgentSidebarToken::TerminalTitle => context
                            .terminal_title
                            .map(|value| ResolvedTokenKind::TerminalTitle(value.to_string())),
                        AgentSidebarToken::TerminalTitleStripped => context
                            .terminal_title_stripped
                            .map(|value| ResolvedTokenKind::TerminalTitle(value.to_string())),
                        AgentSidebarToken::Custom(name) => context
                            .tokens
                            .get(name)
                            .cloned()
                            .map(ResolvedTokenKind::Custom),
                        AgentSidebarToken::Styled { .. } => None,
                    }?;
                    let active = match token {
                        AgentSidebarToken::Tab => Some(context.tab_active),
                        AgentSidebarToken::FocusedTab => Some(true),
                        _ => None,
                    };
                    let style = kind
                        .text_value()
                        .map_or(Some(style.resolved(context.color_tokens)), |value| {
                            configured.style_for_value(value, context.color_tokens, active)
                        })?;
                    Some(ResolvedToken::new(kind, style))
                })
                .collect::<Vec<_>>();
            (!resolved.is_empty()).then_some(resolved)
        })
        .collect()
}

pub(crate) struct SpaceTokenContext<'a> {
    pub(crate) workspace: &'a str,
    pub(crate) branch: Option<&'a str>,
    pub(crate) state_text: &'a str,
    pub(crate) ahead_behind: Option<(usize, usize)>,
    pub(crate) tokens: &'a std::collections::HashMap<String, String>,
    pub(crate) suppress_git_details: bool,
    pub(crate) symbol: Option<&'a str>,
    pub(crate) place: Option<&'a str>,
    pub(crate) app: Option<&'a str>,
    pub(crate) auto_label: Option<&'a str>,
}

pub(crate) fn space_rows(
    config: &SpacesSidebarConfig,
    context: SpaceTokenContext<'_>,
) -> Vec<Vec<ResolvedToken>> {
    config
        .rows
        .iter()
        .filter_map(|row| {
            let resolved = row
                .iter()
                .filter_map(|configured| {
                    let (token, style) = configured.parts();
                    let kind = match token {
                        SpaceSidebarToken::StateIcon => Some(ResolvedTokenKind::StateIcon),
                        SpaceSidebarToken::StateText => {
                            Some(ResolvedTokenKind::StateText(context.state_text.to_string()))
                        }
                        SpaceSidebarToken::Workspace => {
                            Some(ResolvedTokenKind::Workspace(context.workspace.to_string()))
                        }
                        SpaceSidebarToken::Branch if !context.suppress_git_details => context
                            .branch
                            .map(|branch| ResolvedTokenKind::Branch(branch.to_string())),
                        SpaceSidebarToken::Branch => None,
                        SpaceSidebarToken::GitStatus if !context.suppress_git_details => context
                            .ahead_behind
                            .filter(|(ahead, behind)| *ahead > 0 || *behind > 0)
                            .map(|(ahead, behind)| ResolvedTokenKind::GitStatus { ahead, behind }),
                        SpaceSidebarToken::GitStatus => None,
                        SpaceSidebarToken::Symbol => context
                            .symbol
                            .map(|value| ResolvedTokenKind::Custom(value.to_string())),
                        SpaceSidebarToken::Place => context
                            .place
                            .map(|value| ResolvedTokenKind::Workspace(value.to_string())),
                        SpaceSidebarToken::App => context
                            .app
                            .map(|value| ResolvedTokenKind::Agent(value.to_string())),
                        SpaceSidebarToken::AutoLabel => context
                            .auto_label
                            .map(|value| ResolvedTokenKind::Workspace(value.to_string())),
                        SpaceSidebarToken::Custom(name) => context
                            .tokens
                            .get(name)
                            .cloned()
                            .map(ResolvedTokenKind::Custom),
                        SpaceSidebarToken::Styled { .. } => None,
                    }?;
                    let style = kind
                        .text_value()
                        .map_or(Some(style.resolved(context.tokens)), |value| {
                            configured.style_for_value(value, context.tokens, None)
                        })?;
                    Some(ResolvedToken::new(kind, style))
                })
                .collect::<Vec<_>>();
            (!resolved.is_empty()).then_some(resolved)
        })
        .collect()
}

pub(crate) fn separator(previous: &ResolvedToken, current: &ResolvedToken) -> &'static str {
    if matches!(previous.kind, ResolvedTokenKind::StateIcon)
        || matches!(current.kind, ResolvedTokenKind::GitStatus { .. })
    {
        " "
    } else {
        " · "
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AgentSidebarToken, SpaceSidebarToken};

    struct Entry {
        workspace: String,
        tab: Option<String>,
        focused_tab: Option<String>,
        pane: Option<String>,
        agent_label: Option<String>,
        terminal_title: Option<String>,
        terminal_title_stripped: Option<String>,
        canonical_agent: Option<crate::detect::Agent>,
        tokens: std::collections::HashMap<String, String>,
    }

    fn entry() -> Entry {
        Entry {
            workspace: "repo".into(),
            tab: None,
            focused_tab: None,
            pane: None,
            agent_label: Some("pi".into()),
            terminal_title: None,
            terminal_title_stripped: None,
            canonical_agent: Some(crate::detect::Agent::Pi),
            tokens: std::collections::HashMap::new(),
        }
    }

    fn context(entry: &Entry) -> AgentTokenContext<'_> {
        AgentTokenContext {
            machine: None,
            workspace: &entry.workspace,
            tab: entry.tab.as_deref(),
            focused_tab: entry.focused_tab.as_deref(),
            tab_active: false,
            pane: entry.pane.as_deref(),
            agent_label: entry.agent_label.as_deref(),
            terminal_title: entry.terminal_title.as_deref(),
            terminal_title_stripped: entry.terminal_title_stripped.as_deref(),
            canonical_agent: entry.canonical_agent,
            tokens: &entry.tokens,
            color_tokens: &entry.tokens,
        }
    }

    #[test]
    fn conditional_styles_merge_first_match_and_keep_missing_values_absent() {
        let config: AgentsSidebarConfig = toml::from_str(r##"
rows = [["state_icon", { token = "machine", fg = "#fff", bold = true, dim = true, rules = [{ equals = "Local", fg = "#f00", bold = false }, { contains = "Loc", fg = "#0f0", dim = false }] }], [{ token = "$missing", rules = [{ equals = "", bold = true }] }]]
"##).unwrap();
        let entry = entry();
        for (machine, color, bold, dim) in [
            ("Local", (255, 0, 0), false, true),
            ("Localhost", (0, 255, 0), true, false),
            ("local", (255, 255, 255), true, true),
        ] {
            let mut context = context(&entry);
            context.machine = Some(machine);
            let rows = agent_rows(&config, context, "working");
            assert_eq!(rows.len(), 1);
            assert_eq!(
                rows[0][0],
                ResolvedToken::unstyled(ResolvedTokenKind::StateIcon)
            );
            let token = &rows[0][1];
            assert_eq!(token.kind, ResolvedTokenKind::Machine(machine.into()));
            assert_eq!(
                token.style.fg.unwrap().ratatui(),
                ratatui::style::Color::Rgb(color.0, color.1, color.2)
            );
            assert_eq!(token.style.bold, Some(bold));
            assert_eq!(token.style.dim, Some(dim));
        }
        assert_eq!(agent_rows(&config, context(&entry), "working")[0].len(), 1);
    }

    #[test]
    fn focused_tab_resolves_as_text_with_rules_and_drops_when_absent() {
        let config: AgentsSidebarConfig = toml::from_str(r##"
rows = [["state_icon", "tab"], [{ token = "focused_tab", fg = "#888", rules = [{ equals = "logs", fg = "#f00" }] }]]
"##).unwrap();
        let mut entry = entry();
        entry.tab = Some("agent".into());
        assert_eq!(agent_rows(&config, context(&entry), "working").len(), 1);
        for (focused, color) in [("logs", (255, 0, 0)), ("agent", (0x88, 0x88, 0x88))] {
            entry.focused_tab = Some(focused.into());
            let rows = agent_rows(&config, context(&entry), "working");
            assert_eq!(rows.len(), 2);
            assert_eq!(rows[0][1].kind, ResolvedTokenKind::Tab("agent".into()));
            assert_eq!(rows[1][0].kind, ResolvedTokenKind::Tab(focused.into()));
            assert_eq!(
                rows[1][0].style.fg.unwrap().ratatui(),
                ratatui::style::Color::Rgb(color.0, color.1, color.2)
            );
        }
    }

    #[test]
    fn conditional_style_survives_truncation_and_removes_theme_modifiers() {
        use ratatui::style::{Color, Modifier, Style};
        let config: AgentsSidebarConfig = toml::from_str(r##"
rows = [[{ token = "workspace", rules = [{ equals = "long-workspace-name", fg = "#f00", bold = false, dim = false }] }]]
"##).unwrap();
        let mut entry = entry();
        entry.workspace = "long-workspace-name".into();
        let rows = agent_rows(&config, context(&entry), "working");
        let theme = Style::default()
            .fg(Color::Blue)
            .add_modifier(Modifier::BOLD | Modifier::DIM);
        for width in [4, 40] {
            let spans = super::super::resolved_token_spans(
                &rows[0],
                ("*", theme),
                theme,
                theme,
                theme,
                theme,
                &super::super::Palette::catppuccin(),
                width,
                super::super::TokenPaint {
                    panel_text: Color::Reset,
                    cursor: false,
                },
            );
            assert_eq!(spans.len(), 1);
            assert!(super::super::display_width(&spans[0].content) <= width);
            assert_eq!(spans[0].style.fg, Some(Color::Rgb(255, 0, 0)));
            assert!(!spans[0]
                .style
                .add_modifier
                .intersects(Modifier::BOLD | Modifier::DIM));
            assert!(spans[0]
                .style
                .sub_modifier
                .contains(Modifier::BOLD | Modifier::DIM));
        }
    }

    #[test]
    fn custom_numeric_rules_resolve_in_agent_overrides_and_space_rows() {
        let config: crate::config::SidebarConfig = toml::from_str(
            r#"
[agents]
rows = [["workspace"]]
[agents.rows_by_agent]
pi = [[{ token = "$load", rules = [{ gt = 80, bold = true }, { gt = 50, dim = true }] }]]
[spaces]
rows = [[{ token = "$load", rules = [{ lt = 50, dim = true }] }]]
"#,
        )
        .unwrap();
        let mut entry = entry();
        for (value, bold, dim) in [
            ("90", Some(true), None),
            ("60", None, Some(true)),
            ("20", None, None),
            ("90%", None, None),
        ] {
            entry.tokens.insert("load".into(), value.into());
            let rows = agent_rows(&config.agents, context(&entry), "working");
            assert_eq!(rows[0][0].kind, ResolvedTokenKind::Custom(value.into()));
            assert_eq!(rows[0][0].style.bold, bold);
            assert_eq!(rows[0][0].style.dim, dim);
            let spaces = space_rows(
                &config.spaces,
                SpaceTokenContext {
                    workspace: "repo",
                    branch: None,
                    state_text: "working",
                    ahead_behind: None,
                    suppress_git_details: false,
                    tokens: &entry.tokens,
                    symbol: None,
                    place: None,
                    app: None,
                    auto_label: None,
                },
            );
            assert_eq!(spaces[0][0].style.dim, (value == "20").then_some(true));
        }
    }

    #[test]
    fn conditional_hide_removes_tokens_and_empty_rows() {
        let config: crate::config::SidebarConfig = toml::from_str(
            r##"
[agents]
rows = [[{ token = "machine", fg = "#61afef", rules = [{ equals = "Local", hide = true }] }, "agent"]]
[agents.rows_by_agent]
pi = [[{ token = "$load", rules = [{ lt = 50, hide = true }] }], ["agent"]]
[spaces]
rows = [[{ token = "$load", rules = [{ lt = 50, hide = true }] }], ["workspace"]]
"##,
        ).unwrap();
        let encoded = toml::to_string(&config).unwrap();
        assert!(encoded.contains("hide = true"));
        let config: crate::config::SidebarConfig = toml::from_str(&encoded).unwrap();
        let mut entry = entry();
        entry.canonical_agent = None;
        for (machine, count) in [("Local", 1), ("Remote", 2)] {
            let mut ctx = context(&entry);
            ctx.machine = Some(machine);
            let rows = agent_rows(&config.agents, ctx, "working");
            assert_eq!(rows[0].len(), count);
            assert_eq!(
                rows[0].last().unwrap().kind,
                ResolvedTokenKind::Agent("pi".into())
            );
        }
        entry.canonical_agent = Some(crate::detect::Agent::Pi);
        for (value, count) in [("20", 1), ("90", 2)] {
            entry.tokens.insert("load".into(), value.into());
            assert_eq!(
                agent_rows(&config.agents, context(&entry), "working").len(),
                count
            );
            let rows = space_rows(
                &config.spaces,
                SpaceTokenContext {
                    workspace: "repo",
                    branch: None,
                    state_text: "working",
                    ahead_behind: None,
                    suppress_git_details: false,
                    tokens: &entry.tokens,
                    symbol: None,
                    place: None,
                    app: None,
                    auto_label: None,
                },
            );
            assert_eq!(rows.len(), count);
        }
    }

    #[test]
    fn conditional_hide_preserves_first_match_wins() {
        for first in ["hide = false", "bold = true"] {
            let config: AgentsSidebarConfig = toml::from_str(&format!(
                "rows = [[{{ token = 'agent', rules = [{{ equals = 'pi', {first} }}, {{ contains = '', hide = true }}] }}]]"
            )).unwrap();
            let entry = entry();
            let rows = agent_rows(&config, context(&entry), "working");
            assert_eq!(rows[0][0].kind, ResolvedTokenKind::Agent("pi".into()));
        }
    }

    #[test]
    fn missing_custom_tokens_elide_rows_and_separators() {
        let entry = entry();
        let config = AgentsSidebarConfig {
            rows: vec![
                vec![
                    AgentSidebarToken::StateIcon,
                    AgentSidebarToken::Custom("missing".into()),
                ],
                vec![AgentSidebarToken::Custom("missing".into())],
                vec![AgentSidebarToken::Agent],
            ],
            ..Default::default()
        };

        let rows = agent_rows(&config, context(&entry), "working");

        assert_eq!(rows.len(), 2);
        assert_eq!(
            rows[0],
            vec![ResolvedToken::unstyled(ResolvedTokenKind::StateIcon)]
        );
        assert_eq!(
            rows[1],
            vec![ResolvedToken::unstyled(ResolvedTokenKind::Agent(
                "pi".into()
            ))]
        );
    }

    #[test]
    fn machine_token_only_resolves_for_multi_machine_rows() {
        let entry = entry();
        let config = AgentsSidebarConfig {
            rows: vec![vec![
                AgentSidebarToken::Machine,
                AgentSidebarToken::Workspace,
            ]],
            ..Default::default()
        };

        assert_eq!(
            agent_rows(&config, context(&entry), "working"),
            vec![vec![ResolvedToken::unstyled(ResolvedTokenKind::Workspace(
                "repo".into()
            ))]]
        );

        let mut remote_context = context(&entry);
        remote_context.machine = Some("Build");
        assert_eq!(
            agent_rows(&config, remote_context, "working"),
            vec![vec![
                ResolvedToken::unstyled(ResolvedTokenKind::Machine("Build".into())),
                ResolvedToken::unstyled(ResolvedTokenKind::Workspace("repo".into())),
            ]]
        );
    }

    #[test]
    fn state_text_and_arbitrary_values_are_independent_tokens() {
        let mut entry = entry();
        entry
            .tokens
            .insert("summary".into(), "reviewing auth".into());
        let config = AgentsSidebarConfig {
            rows: vec![vec![
                AgentSidebarToken::StateText,
                AgentSidebarToken::Custom("summary".into()),
            ]],
            ..Default::default()
        };

        assert_eq!(
            agent_rows(&config, context(&entry), "deep in the mines"),
            vec![vec![
                ResolvedToken::unstyled(ResolvedTokenKind::StateText("deep in the mines".into())),
                ResolvedToken::unstyled(ResolvedTokenKind::Custom("reviewing auth".into())),
            ]]
        );
    }

    #[test]
    fn terminal_title_builtins_are_distinct_from_custom_tokens() {
        let mut entry = entry();
        entry.terminal_title = Some("⠋ raw title".into());
        entry.terminal_title_stripped = Some("raw title".into());
        entry
            .tokens
            .insert("terminal_title".into(), "custom title".into());
        let config = AgentsSidebarConfig {
            rows: vec![vec![
                AgentSidebarToken::TerminalTitle,
                AgentSidebarToken::TerminalTitleStripped,
                AgentSidebarToken::Custom("terminal_title".into()),
            ]],
            ..Default::default()
        };

        assert_eq!(
            agent_rows(&config, context(&entry), "working"),
            vec![vec![
                ResolvedToken::unstyled(ResolvedTokenKind::TerminalTitle("⠋ raw title".into())),
                ResolvedToken::unstyled(ResolvedTokenKind::TerminalTitle("raw title".into())),
                ResolvedToken::unstyled(ResolvedTokenKind::Custom("custom title".into())),
            ]]
        );
    }

    #[test]
    fn known_agent_override_replaces_default_rows() {
        let mut config = AgentsSidebarConfig {
            rows: vec![vec![AgentSidebarToken::Workspace]],
            ..Default::default()
        };
        config
            .rows_by_agent
            .insert("pi".into(), vec![vec![AgentSidebarToken::Agent]]);
        let mut pi = entry();
        pi.agent_label = Some("renamed pi".into());

        assert_eq!(
            agent_rows(&config, context(&pi), "working"),
            vec![vec![ResolvedToken::unstyled(ResolvedTokenKind::Agent(
                "renamed pi".into()
            ))]]
        );

        pi.canonical_agent = None;
        assert_eq!(
            agent_rows(&config, context(&pi), "working"),
            vec![vec![ResolvedToken::unstyled(ResolvedTokenKind::Workspace(
                "repo".into()
            ))]]
        );
    }

    #[test]
    fn grouped_children_suppress_all_builtin_git_details() {
        let config = SpacesSidebarConfig::default();

        assert_eq!(
            space_rows(
                &config,
                SpaceTokenContext {
                    workspace: "feature",
                    branch: Some("worktree/feature"),
                    state_text: "idle",
                    ahead_behind: Some((2, 1)),
                    tokens: &std::collections::HashMap::new(),
                    suppress_git_details: true,
                    symbol: None,
                    place: None,
                    app: None,
                    auto_label: None,
                },
            ),
            vec![vec![
                ResolvedToken::unstyled(ResolvedTokenKind::StateIcon),
                ResolvedToken::unstyled(ResolvedTokenKind::Workspace("feature".into())),
            ]]
        );
    }

    #[test]
    fn workspace_custom_token_can_replace_git_specific_details() {
        let tokens = std::collections::HashMap::from([("jj_status".into(), "2 changes".into())]);
        let config = SpacesSidebarConfig {
            rows: vec![vec![SpaceSidebarToken::Custom("jj_status".into())]],
            ..Default::default()
        };

        assert_eq!(
            space_rows(
                &config,
                SpaceTokenContext {
                    workspace: "repo",
                    branch: None,
                    state_text: "idle",
                    ahead_behind: None,
                    tokens: &tokens,
                    suppress_git_details: false,
                    symbol: None,
                    place: None,
                    app: None,
                    auto_label: None,
                },
            ),
            vec![vec![ResolvedToken::unstyled(ResolvedTokenKind::Custom(
                "2 changes".into()
            ))]]
        );
    }

    fn fg_of(token: &ResolvedToken) -> Option<ratatui::style::Color> {
        token.style.fg.map(|fg| fg.ratatui())
    }

    #[test]
    fn token_reference_fg_takes_color_from_workspace_tokens_in_spaces() {
        use ratatui::style::Color;
        let config: SpacesSidebarConfig = toml::from_str(
            r##"rows = [[{ token = "workspace", fg = "$color" }], [{ token = "workspace", fg = "#00ff00", rules = [{ contains = "x", fg = "$color" }] }]]"##,
        )
        .unwrap();
        let rows = |workspace: &str, value: Option<&str>| {
            let tokens: std::collections::HashMap<String, String> = value
                .map(|value| ("color".to_string(), value.to_string()))
                .into_iter()
                .collect();
            space_rows(
                &config,
                SpaceTokenContext {
                    workspace,
                    branch: None,
                    state_text: "idle",
                    ahead_behind: None,
                    suppress_git_details: false,
                    tokens: &tokens,
                    symbol: None,
                    place: None,
                    app: None,
                    auto_label: None,
                },
            )
        };
        let red = Some(Color::Rgb(255, 0, 0));
        let green = Some(Color::Rgb(0, 255, 0));
        let rows_red = rows("ax", Some("#ff0000"));
        assert_eq!(fg_of(&rows_red[0][0]), red);
        assert_eq!(fg_of(&rows_red[1][0]), red);
        // Rule does not match: general fg is kept.
        assert_eq!(fg_of(&rows("repo", Some("#ff0000"))[1][0]), green);
        for value in [None, Some(""), Some("red")] {
            let rows = rows("ax", value);
            assert_eq!(fg_of(&rows[0][0]), None);
            // Unresolvable rule reference falls back to the general fg.
            assert_eq!(fg_of(&rows[1][0]), green);
        }
    }

    #[test]
    fn token_reference_fg_takes_color_from_pane_tokens_in_agents() {
        use ratatui::style::Color;
        let config: AgentsSidebarConfig = toml::from_str(
            r##"rows = [[{ token = "workspace", fg = "$color" }], [{ token = "workspace", rules = [{ contains = "x", fg = "$color" }] }]]"##,
        )
        .unwrap();
        let mut entry = entry();
        entry.workspace = "ax".into();
        entry.tokens.insert("color".into(), "#ff0000".into());
        let rows = agent_rows(&config, context(&entry), "working");
        assert_eq!(fg_of(&rows[0][0]), Some(Color::Rgb(255, 0, 0)));
        assert_eq!(fg_of(&rows[1][0]), Some(Color::Rgb(255, 0, 0)));
        for value in [None, Some("red")] {
            entry.tokens.clear();
            if let Some(value) = value {
                entry.tokens.insert("color".into(), value.into());
            }
            let rows = agent_rows(&config, context(&entry), "working");
            assert_eq!(fg_of(&rows[0][0]), None);
            assert_eq!(fg_of(&rows[1][0]), None);
        }
    }

    #[test]
    fn space_label_tokens_resolve_and_drop_when_missing() {
        let config: SpacesSidebarConfig =
            toml::from_str(r#"rows = [["symbol", "place", "app"], ["auto_label"]]"#).unwrap();
        let tokens = std::collections::HashMap::new();
        let context = |symbol, place, app, auto_label| SpaceTokenContext {
            workspace: "repo",
            branch: None,
            state_text: "idle",
            ahead_behind: None,
            tokens: &tokens,
            suppress_git_details: false,
            symbol,
            place,
            app,
            auto_label,
        };
        assert_eq!(
            space_rows(
                &config,
                context(
                    Some("🦦"),
                    Some("herdr"),
                    Some("nvim"),
                    Some("🦦 herdr · nvim")
                )
            ),
            vec![
                vec![
                    ResolvedToken::unstyled(ResolvedTokenKind::Custom("🦦".into())),
                    ResolvedToken::unstyled(ResolvedTokenKind::Workspace("herdr".into())),
                    ResolvedToken::unstyled(ResolvedTokenKind::Agent("nvim".into())),
                ],
                vec![ResolvedToken::unstyled(ResolvedTokenKind::Workspace(
                    "🦦 herdr · nvim".into()
                ))],
            ]
        );
        assert_eq!(
            space_rows(
                &config,
                context(None, Some("~/notes"), None, Some("~/notes"))
            ),
            vec![
                vec![ResolvedToken::unstyled(ResolvedTokenKind::Workspace(
                    "~/notes".into()
                ))],
                vec![ResolvedToken::unstyled(ResolvedTokenKind::Workspace(
                    "~/notes".into()
                ))],
            ]
        );
        assert!(space_rows(&config, context(None, None, None, None)).is_empty());
    }

    #[test]
    fn active_rule_follows_tab_activity_and_skips_other_tokens() {
        use crate::config::SidebarTokenColor;
        let config: AgentsSidebarConfig = toml::from_str(
            r##"rows = [[{ token = "tab", fg = "$panel", bg = "$color", rules = [{ active = true, fg = "$color", bg = "$panel" }] }, { token = "focused_tab", rules = [{ active = true, bold = true }] }, { token = "workspace", rules = [{ active = true, bold = true }, { active = false, bold = false }] }]]"##,
        )
        .unwrap();
        let mut entry = entry();
        entry.tab = Some("agent".into());
        entry.focused_tab = Some("logs".into());
        entry.tokens.insert("color".into(), "#ff0000".into());
        let red = Some(SidebarTokenColor::Rgb { r: 255, g: 0, b: 0 });
        for active in [true, false] {
            let mut ctx = context(&entry);
            ctx.tab_active = active;
            let rows = agent_rows(&config, ctx, "working");
            let (tab, focused, workspace) = (&rows[0][0], &rows[0][1], &rows[0][2]);
            if active {
                assert_eq!(tab.style.fg, red);
                assert_eq!(tab.style.bg, Some(SidebarTokenColor::Panel));
            } else {
                assert_eq!(tab.style.fg, Some(SidebarTokenColor::Panel));
                assert_eq!(tab.style.bg, red);
            }
            // `focused_tab` always shows the active tab of the workspace.
            assert_eq!(focused.style.bold, Some(true));
            // Tokens that show no tab never match `active`.
            assert_eq!(workspace.style.bold, None);
        }
        let spaces: SpacesSidebarConfig = toml::from_str(
            r#"rows = [[{ token = "workspace", rules = [{ active = true, bold = true }, { active = false, bold = false }] }]]"#,
        )
        .unwrap();
        let rows = space_rows(
            &spaces,
            SpaceTokenContext {
                workspace: "repo",
                branch: None,
                state_text: "idle",
                ahead_behind: None,
                tokens: &entry.tokens,
                suppress_git_details: false,
                symbol: None,
                place: None,
                app: None,
                auto_label: None,
            },
        );
        assert_eq!(rows[0][0].style.bold, None);
    }
}
