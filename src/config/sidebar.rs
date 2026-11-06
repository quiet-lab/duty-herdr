mod rules;

pub use rules::SidebarTokenRule;

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::detect::Agent;

const MAX_SIDEBAR_ROWS: usize = 16;
const MAX_SIDEBAR_TOKENS_PER_ROW: usize = 16;
const DEFAULT_SIDEBAR_ROW_GAP: u16 = 0;
const MAX_CUSTOM_TOKEN_NAME: usize = 32;

fn is_valid_custom_token_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_CUSTOM_TOKEN_NAME
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-'))
}

fn deserialize_sidebar_rows<'de, D, T>(deserializer: D) -> Result<Vec<Vec<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    let rows = Vec::<Vec<T>>::deserialize(deserializer)?;
    validate_sidebar_rows(&rows).map_err(serde::de::Error::custom)?;
    Ok(rows)
}

fn validate_sidebar_rows<T>(rows: &[Vec<T>]) -> Result<(), String> {
    if rows.len() > MAX_SIDEBAR_ROWS {
        return Err(format!(
            "sidebar layouts may contain at most {MAX_SIDEBAR_ROWS} rows"
        ));
    }
    if rows
        .iter()
        .any(|row| row.len() > MAX_SIDEBAR_TOKENS_PER_ROW)
    {
        return Err(format!(
            "sidebar rows may contain at most {MAX_SIDEBAR_TOKENS_PER_ROW} tokens"
        ));
    }
    Ok(())
}

/// Name of a user token whose value supplies a color (`fg = "$name"`).
/// Stored inline so styles stay `Copy`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SidebarColorTokenName {
    buf: [u8; MAX_CUSTOM_TOKEN_NAME],
    len: u8,
}

impl SidebarColorTokenName {
    fn new(name: &str) -> Option<Self> {
        if !is_valid_custom_token_name(name) {
            return None;
        }
        let mut buf = [0u8; MAX_CUSTOM_TOKEN_NAME];
        buf[..name.len()].copy_from_slice(name.as_bytes());
        Some(Self {
            buf,
            len: name.len() as u8,
        })
    }

    fn as_str(&self) -> &str {
        // The buffer only ever holds validated ASCII.
        std::str::from_utf8(&self.buf[..usize::from(self.len)]).unwrap_or_default()
    }
}

/// Metadata tokens that `$name` colors resolve against: the item's own tokens,
/// then `fallback` (a pane's workspace tokens), looked up without allocating.
#[derive(Debug, Clone, Copy, Default)]
pub struct ColorTokens<'a> {
    pub(crate) item: Option<&'a std::collections::HashMap<String, String>>,
    pub(crate) fallback: &'a [(String, String)],
}

impl<'a> ColorTokens<'a> {
    pub(crate) fn new(item: &'a std::collections::HashMap<String, String>) -> Self {
        Self {
            item: Some(item),
            fallback: &[],
        }
    }

    fn get(&self, name: &str) -> Option<&'a str> {
        self.item
            .and_then(|item| item.get(name))
            .or_else(|| {
                self.fallback
                    .iter()
                    .find(|(key, _)| key == name)
                    .map(|(_, value)| value)
            })
            .map(String::as_str)
    }
}

/// Reserved color name for the sidebar panel background.
const PANEL_COLOR: &str = "$panel";
/// Reserved color name for the default background of the terminal area.
const TERMINAL_COLOR: &str = "$terminal";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidebarTokenColor {
    Rgb {
        r: u8,
        g: u8,
        b: u8,
    },
    /// Color taken from the value of a user token of the current item.
    Token(SidebarColorTokenName),
    /// Background of the sidebar panel in the current theme (`$panel`).
    Panel,
    /// Default background of the terminal area (`$terminal`): the host
    /// terminal background, rendered as `Color::Reset`.
    Terminal,
    /// Ordered alternatives `a|b|c`; the first one that resolves is used.
    Fallback(ColorChain),
}

/// Maximum number of `|`-separated alternatives in one color value.
const MAX_COLOR_ALTERNATIVES: usize = 3;

/// One alternative of a color chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ColorAtom {
    Rgb { r: u8, g: u8, b: u8 },
    Token(SidebarColorTokenName),
    Panel,
    Terminal,
}

impl ColorAtom {
    fn color(self) -> SidebarTokenColor {
        match self {
            Self::Rgb { r, g, b } => SidebarTokenColor::Rgb { r, g, b },
            Self::Token(name) => SidebarTokenColor::Token(name),
            Self::Panel => SidebarTokenColor::Panel,
            Self::Terminal => SidebarTokenColor::Terminal,
        }
    }
}

/// Fixed-capacity list of two or more alternatives; kept `Copy` so styles stay
/// cheap to resolve in render paths.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColorChain {
    atoms: [ColorAtom; MAX_COLOR_ALTERNATIVES],
    len: u8,
}

impl ColorChain {
    fn atoms(&self) -> &[ColorAtom] {
        &self.atoms[..usize::from(self.len)]
    }
}

impl SidebarTokenColor {
    /// Parses `#RGB` or `#RRGGBB`.
    fn parse_hex(value: &str) -> Option<Self> {
        let hex = value.strip_prefix('#').filter(|hex| {
            hex.is_ascii()
                && matches!(hex.len(), 3 | 6)
                && hex.bytes().all(|byte| byte.is_ascii_hexdigit())
        })?;
        let digit = |index: usize| u8::from_str_radix(&hex[index..=index], 16).ok();
        let pair = |index: usize| u8::from_str_radix(&hex[index..index + 2], 16).ok();
        let (r, g, b) = if hex.len() == 3 {
            (digit(0)? * 17, digit(1)? * 17, digit(2)? * 17)
        } else {
            (pair(0)?, pair(2)?, pair(4)?)
        };
        Some(Self::Rgb { r, g, b })
    }

    /// Replaces a token reference with the color in the token's value; a
    /// missing, empty or non-color value yields `None`.
    pub(crate) fn resolve(self, tokens: ColorTokens<'_>) -> Option<Self> {
        match self {
            Self::Rgb { .. } | Self::Panel | Self::Terminal => Some(self),
            Self::Token(name) => tokens.get(name.as_str()).and_then(Self::parse_hex),
            Self::Fallback(chain) => chain
                .atoms()
                .iter()
                .find_map(|atom| atom.color().resolve(tokens)),
        }
    }

    pub(crate) fn ratatui(self) -> ratatui::style::Color {
        self.ratatui_with_panel(ratatui::style::Color::Reset)
    }

    /// Terminal color with `$panel` replaced by `panel`.
    pub(crate) fn ratatui_with_panel(self, panel: ratatui::style::Color) -> ratatui::style::Color {
        match self {
            Self::Rgb { r, g, b } => ratatui::style::Color::Rgb(r, g, b),
            Self::Panel => panel,
            // The cell shows the host terminal background.
            Self::Terminal => ratatui::style::Color::Reset,
            // Unresolved references are dropped before rendering.
            Self::Token(_) | Self::Fallback(_) => ratatui::style::Color::Reset,
        }
    }

    /// Parses one alternative: `#RGB`, `#RRGGBB`, `$panel`, `$terminal` or `$token`.
    fn parse_atom(value: &str) -> Result<ColorAtom, String> {
        if value.is_empty() {
            return Err("sidebar token color has an empty alternative".to_owned());
        }
        if value == PANEL_COLOR {
            return Ok(ColorAtom::Panel);
        }
        if value == TERMINAL_COLOR {
            return Ok(ColorAtom::Terminal);
        }
        if let Some(name) = value.strip_prefix('$') {
            return SidebarColorTokenName::new(name)
                .map(ColorAtom::Token)
                .ok_or_else(|| format!("invalid custom sidebar token `{value}` in color"));
        }
        match Self::parse_hex(value) {
            Some(Self::Rgb { r, g, b }) => Ok(ColorAtom::Rgb { r, g, b }),
            _ => Err(format!(
                "sidebar token color must be #RGB, #RRGGBB, $panel, $terminal or $token, got `{value}`"
            )),
        }
    }

    fn parse(value: &str) -> Result<Self, String> {
        if !value.contains('|') {
            return Self::parse_atom(value).map(ColorAtom::color);
        }
        let mut atoms = [ColorAtom::Panel; MAX_COLOR_ALTERNATIVES];
        let mut len = 0;
        for part in value.split('|') {
            let atom = Self::parse_atom(part)?;
            if len > 0 && !matches!(atoms[len - 1], ColorAtom::Token(_)) {
                return Err(format!(
                    "sidebar token color `{value}` has an alternative after one that always resolves"
                ));
            }
            if len == MAX_COLOR_ALTERNATIVES {
                return Err(format!(
                    "sidebar token color accepts at most {MAX_COLOR_ALTERNATIVES} alternatives"
                ));
            }
            atoms[len] = atom;
            len += 1;
        }
        Ok(Self::Fallback(ColorChain {
            atoms,
            len: len as u8,
        }))
    }
}

impl ColorAtom {
    fn encode(self) -> String {
        match self {
            Self::Rgb { r, g, b } => format!("#{r:02x}{g:02x}{b:02x}"),
            Self::Token(name) => format!("${}", name.as_str()),
            Self::Panel => PANEL_COLOR.to_owned(),
            Self::Terminal => TERMINAL_COLOR.to_owned(),
        }
    }
}

impl Serialize for SidebarTokenColor {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Self::Rgb { r, g, b } => serializer.serialize_str(&format!("#{r:02x}{g:02x}{b:02x}")),
            Self::Token(name) => serializer.serialize_str(&format!("${}", name.as_str())),
            Self::Panel => serializer.serialize_str(PANEL_COLOR),
            Self::Terminal => serializer.serialize_str(TERMINAL_COLOR),
            Self::Fallback(chain) => {
                let parts: Vec<String> = chain.atoms().iter().map(|atom| atom.encode()).collect();
                serializer.serialize_str(&parts.join("|"))
            }
        }
    }
}

impl<'de> Deserialize<'de> for SidebarTokenColor {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct SidebarTokenStyle {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fg: Option<SidebarTokenColor>,
    /// Background of the token; the first token with one fills its whole row.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bg: Option<SidebarTokenColor>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bold: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dim: Option<bool>,
}

impl SidebarTokenStyle {
    /// Resolves token-reference colors against the current item's tokens.
    pub(crate) fn resolved(self, tokens: ColorTokens<'_>) -> Self {
        Self {
            fg: self.fg.and_then(|fg| fg.resolve(tokens)),
            bg: self.bg.and_then(|bg| bg.resolve(tokens)),
            ..self
        }
    }
}

#[allow(clippy::large_enum_variant)] // inline `Copy` color chains keep styles allocation-free in render paths
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentSidebarToken {
    StateIcon,
    StateText,
    Machine,
    Workspace,
    Tab,
    FocusedTab,
    Pane,
    Agent,
    TerminalTitle,
    TerminalTitleStripped,
    Custom(String),
    Styled {
        token: Box<AgentSidebarToken>,
        style: SidebarTokenStyle,
        rules: Vec<SidebarTokenRule>,
    },
}

#[allow(clippy::large_enum_variant)] // inline `Copy` color chains keep styles allocation-free in render paths
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpaceSidebarToken {
    StateIcon,
    StateText,
    Workspace,
    Branch,
    GitStatus,
    /// Workspace metadata token `symbol`.
    Symbol,
    /// Repository name of the first tab's root pane, or its cwd with `~`.
    Place,
    /// Agent of the first tab while detected, otherwise the foreground process
    /// of the first tab's focused pane.
    App,
    /// `<symbol> <place> · <app>`.
    AutoLabel,
    Custom(String),
    Styled {
        token: Box<SpaceSidebarToken>,
        style: SidebarTokenStyle,
        rules: Vec<SidebarTokenRule>,
    },
}

impl AgentSidebarToken {
    /// Style of the token for `value`; `active` is whether the tab shown by the
    /// token is active in its workspace (`None` for tokens that show no tab).
    pub(crate) fn style_for_value(
        &self,
        value: &str,
        tokens: ColorTokens<'_>,
        active: Option<bool>,
    ) -> Option<SidebarTokenStyle> {
        match self {
            Self::Styled { style, rules, .. } => {
                rules::matching_style(rules, *style, value, tokens, active)
            }
            _ => Some(SidebarTokenStyle::default()),
        }
    }

    pub(crate) fn parts(&self) -> (&Self, SidebarTokenStyle) {
        match self {
            Self::Styled { token, style, .. } => (token, *style),
            token => (token, SidebarTokenStyle::default()),
        }
    }
}

impl SpaceSidebarToken {
    /// Style of the token for `value`; `active` is whether the tab shown by the
    /// token is active in its workspace (`None` for tokens that show no tab).
    pub(crate) fn style_for_value(
        &self,
        value: &str,
        tokens: ColorTokens<'_>,
        active: Option<bool>,
    ) -> Option<SidebarTokenStyle> {
        match self {
            Self::Styled { style, rules, .. } => {
                rules::matching_style(rules, *style, value, tokens, active)
            }
            _ => Some(SidebarTokenStyle::default()),
        }
    }

    pub(crate) fn parts(&self) -> (&Self, SidebarTokenStyle) {
        match self {
            Self::Styled { token, style, .. } => (token, *style),
            token => (token, SidebarTokenStyle::default()),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawStyledSidebarToken {
    token: String,
    #[serde(default)]
    fg: Option<SidebarTokenColor>,
    #[serde(default)]
    bg: Option<SidebarTokenColor>,
    #[serde(default)]
    bold: Option<bool>,
    #[serde(default)]
    dim: Option<bool>,
    #[serde(default)]
    rules: Vec<SidebarTokenRule>,
}

#[derive(Deserialize)]
#[serde(untagged)]
#[allow(clippy::large_enum_variant)] // inline `Copy` color chains keep styles allocation-free in render paths
enum RawSidebarToken {
    Plain(String),
    Styled(RawStyledSidebarToken),
}

impl RawSidebarToken {
    fn parts(self) -> Result<(String, Option<SidebarTokenStyle>, Vec<SidebarTokenRule>), String> {
        match self {
            Self::Plain(token) => Ok((token, None, Vec::new())),
            Self::Styled(token) => {
                if token.rules.len() > 16 {
                    return Err("sidebar tokens may contain at most 16 rules".into());
                }
                if !token.rules.is_empty()
                    && matches!(token.token.as_str(), "state_icon" | "git_status")
                {
                    return Err("sidebar rules require a text-valued token".into());
                }
                Ok((
                    token.token,
                    Some(SidebarTokenStyle {
                        fg: token.fg,
                        bg: token.bg,
                        bold: token.bold,
                        dim: token.dim,
                    }),
                    token.rules,
                ))
            }
        }
    }
}

fn parse_sidebar_token<T>(value: String, builtins: &[(&str, T)]) -> Result<T, String>
where
    T: Clone + From<String>,
{
    if let Some((_, token)) = builtins.iter().find(|(name, _)| *name == value) {
        return Ok(token.clone());
    }
    let Some(name) = value.strip_prefix('$') else {
        return Err(format!(
            "unknown sidebar token `{value}`; custom tokens must start with `$`"
        ));
    };
    if !is_valid_custom_token_name(name) {
        return Err(format!("invalid custom sidebar token `{value}`"));
    }
    Ok(T::from(name.to_string()))
}

fn serialize_styled_token<S>(
    name: String,
    style: SidebarTokenStyle,
    rules: &[SidebarTokenRule],
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    use serde::ser::SerializeMap;
    let mut map = serializer.serialize_map(None)?;
    map.serialize_entry("token", &name)?;
    if let Some(fg) = style.fg {
        map.serialize_entry("fg", &fg)?;
    }
    if let Some(bg) = style.bg {
        map.serialize_entry("bg", &bg)?;
    }
    if let Some(bold) = style.bold {
        map.serialize_entry("bold", &bold)?;
    }
    if let Some(dim) = style.dim {
        map.serialize_entry("dim", &dim)?;
    }
    if !rules.is_empty() {
        map.serialize_entry("rules", rules)?;
    }
    map.end()
}

fn agent_token_name(token: &AgentSidebarToken) -> String {
    match token {
        AgentSidebarToken::StateIcon => "state_icon".into(),
        AgentSidebarToken::StateText => "state_text".into(),
        AgentSidebarToken::Machine => "machine".into(),
        AgentSidebarToken::Workspace => "workspace".into(),
        AgentSidebarToken::Tab => "tab".into(),
        AgentSidebarToken::FocusedTab => "focused_tab".into(),
        AgentSidebarToken::Pane => "pane".into(),
        AgentSidebarToken::Agent => "agent".into(),
        AgentSidebarToken::TerminalTitle => "terminal_title".into(),
        AgentSidebarToken::TerminalTitleStripped => "terminal_title_stripped".into(),
        AgentSidebarToken::Custom(name) => format!("${name}"),
        AgentSidebarToken::Styled { token, .. } => agent_token_name(token),
    }
}

fn space_token_name(token: &SpaceSidebarToken) -> String {
    match token {
        SpaceSidebarToken::StateIcon => "state_icon".into(),
        SpaceSidebarToken::StateText => "state_text".into(),
        SpaceSidebarToken::Workspace => "workspace".into(),
        SpaceSidebarToken::Branch => "branch".into(),
        SpaceSidebarToken::GitStatus => "git_status".into(),
        SpaceSidebarToken::Symbol => "symbol".into(),
        SpaceSidebarToken::Place => "place".into(),
        SpaceSidebarToken::App => "app".into(),
        SpaceSidebarToken::AutoLabel => "auto_label".into(),
        SpaceSidebarToken::Custom(name) => format!("${name}"),
        SpaceSidebarToken::Styled { token, .. } => space_token_name(token),
    }
}

impl Serialize for AgentSidebarToken {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Self::Styled {
                token,
                style,
                rules,
            } => serialize_styled_token(agent_token_name(token), *style, rules, serializer),
            token => serializer.serialize_str(&agent_token_name(token)),
        }
    }
}

impl From<String> for AgentSidebarToken {
    fn from(value: String) -> Self {
        Self::Custom(value)
    }
}

impl<'de> Deserialize<'de> for AgentSidebarToken {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let (value, style, rules) = RawSidebarToken::deserialize(deserializer)?
            .parts()
            .map_err(serde::de::Error::custom)?;
        let token = parse_sidebar_token(
            value,
            &[
                ("state_icon", Self::StateIcon),
                ("state_text", Self::StateText),
                ("machine", Self::Machine),
                ("workspace", Self::Workspace),
                ("tab", Self::Tab),
                ("focused_tab", Self::FocusedTab),
                ("pane", Self::Pane),
                ("agent", Self::Agent),
                ("terminal_title", Self::TerminalTitle),
                ("terminal_title_stripped", Self::TerminalTitleStripped),
            ],
        )
        .map_err(serde::de::Error::custom)?;
        Ok(style.map_or(token.clone(), |style| Self::Styled {
            token: Box::new(token),
            style,
            rules,
        }))
    }
}

impl Serialize for SpaceSidebarToken {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Self::Styled {
                token,
                style,
                rules,
            } => serialize_styled_token(space_token_name(token), *style, rules, serializer),
            token => serializer.serialize_str(&space_token_name(token)),
        }
    }
}

impl From<String> for SpaceSidebarToken {
    fn from(value: String) -> Self {
        Self::Custom(value)
    }
}

impl<'de> Deserialize<'de> for SpaceSidebarToken {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let (value, style, rules) = RawSidebarToken::deserialize(deserializer)?
            .parts()
            .map_err(serde::de::Error::custom)?;
        let token = parse_sidebar_token(
            value,
            &[
                ("state_icon", Self::StateIcon),
                ("state_text", Self::StateText),
                ("workspace", Self::Workspace),
                ("branch", Self::Branch),
                ("git_status", Self::GitStatus),
                ("symbol", Self::Symbol),
                ("place", Self::Place),
                ("app", Self::App),
                ("auto_label", Self::AutoLabel),
            ],
        )
        .map_err(serde::de::Error::custom)?;
        Ok(style.map_or(token.clone(), |style| Self::Styled {
            token: Box::new(token),
            style,
            rules,
        }))
    }
}

type AgentSidebarRows = Vec<Vec<AgentSidebarToken>>;
type SpaceSidebarRows = Vec<Vec<SpaceSidebarToken>>;

fn deserialize_rows_by_agent<'de, D>(
    deserializer: D,
) -> Result<BTreeMap<String, AgentSidebarRows>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let rows_by_agent = BTreeMap::<String, AgentSidebarRows>::deserialize(deserializer)?;
    for (id, rows) in &rows_by_agent {
        if crate::detect::parse_canonical_agent_label(id).is_none() {
            return Err(serde::de::Error::custom(format!(
                "unknown canonical agent id `{id}` in sidebar rows_by_agent"
            )));
        }
        validate_sidebar_rows(rows).map_err(serde::de::Error::custom)?;
    }
    Ok(rows_by_agent)
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct AgentsSidebarConfig {
    #[serde(deserialize_with = "deserialize_sidebar_rows")]
    pub rows: AgentSidebarRows,
    #[serde(default, deserialize_with = "deserialize_rows_by_agent")]
    pub rows_by_agent: BTreeMap<String, AgentSidebarRows>,
    pub row_gap: u16,
}

impl AgentsSidebarConfig {
    pub(crate) fn rows_for_agent(&self, agent: Option<Agent>) -> &AgentSidebarRows {
        agent
            .and_then(|agent| self.rows_by_agent.get(crate::detect::agent_label(agent)))
            .unwrap_or(&self.rows)
    }
}

impl Default for AgentsSidebarConfig {
    fn default() -> Self {
        Self {
            rows: vec![
                vec![
                    AgentSidebarToken::StateIcon,
                    AgentSidebarToken::Machine,
                    AgentSidebarToken::Workspace,
                    AgentSidebarToken::Tab,
                ],
                vec![AgentSidebarToken::Agent],
            ],
            rows_by_agent: BTreeMap::new(),
            row_gap: DEFAULT_SIDEBAR_ROW_GAP,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct SpacesSidebarConfig {
    #[serde(deserialize_with = "deserialize_sidebar_rows")]
    pub rows: SpaceSidebarRows,
    pub row_gap: u16,
}

impl Default for SpacesSidebarConfig {
    fn default() -> Self {
        Self {
            rows: vec![
                vec![SpaceSidebarToken::StateIcon, SpaceSidebarToken::Workspace],
                vec![SpaceSidebarToken::Branch, SpaceSidebarToken::GitStatus],
            ],
            row_gap: DEFAULT_SIDEBAR_ROW_GAP,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct SidebarConfig {
    pub agents: AgentsSidebarConfig,
    pub spaces: SpacesSidebarConfig,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_compact_agent_and_existing_space_layouts() {
        let config = SidebarConfig::default();
        assert_eq!(
            config.agents.rows,
            vec![
                vec![
                    AgentSidebarToken::StateIcon,
                    AgentSidebarToken::Machine,
                    AgentSidebarToken::Workspace,
                    AgentSidebarToken::Tab,
                ],
                vec![AgentSidebarToken::Agent],
            ]
        );
        assert!(config.agents.rows_by_agent.is_empty());
        assert_eq!(config.agents.row_gap, 0);
        assert_eq!(
            config.spaces.rows,
            vec![
                vec![SpaceSidebarToken::StateIcon, SpaceSidebarToken::Workspace],
                vec![SpaceSidebarToken::Branch, SpaceSidebarToken::GitStatus],
            ]
        );
        assert_eq!(config.spaces.row_gap, 0);
    }

    #[test]
    fn parses_and_serializes_space_label_tokens() {
        let config: crate::config::Config = toml::from_str(
            r##"
[ui.sidebar.spaces]
rows = [["state_icon", "symbol", { token = "place", fg = "$color" }, "app"], ["auto_label"]]
"##,
        )
        .expect("space label tokens");
        let rows = &config.ui.sidebar.spaces.rows;
        assert_eq!(rows[0][1], SpaceSidebarToken::Symbol);
        assert!(matches!(
            &rows[0][2],
            SpaceSidebarToken::Styled { token, .. } if **token == SpaceSidebarToken::Place
        ));
        assert_eq!(rows[0][3], SpaceSidebarToken::App);
        assert_eq!(rows[1], vec![SpaceSidebarToken::AutoLabel]);
        let encoded = toml::to_string(&config.ui.sidebar.spaces).expect("encode");
        let decoded: SpacesSidebarConfig = toml::from_str(&encoded).expect("decode");
        assert_eq!(&decoded.rows, rows);
    }

    #[test]
    fn parses_builtin_and_arbitrary_custom_tokens() {
        let config: crate::config::Config = toml::from_str(
            r#"
[ui.sidebar.agents]
rows = [["state_icon", "workspace"], ["state_text", "agent", "$summary"], ["terminal_title", "terminal_title_stripped", "$terminal_title"]]
row_gap = 1

[ui.sidebar.agents.rows_by_agent]
claude = [["terminal_title_stripped"], ["agent", "$model"]]

[ui.sidebar.spaces]
rows = [["workspace"], ["$jj_status"]]
row_gap = 3
"#,
        )
        .expect("sidebar token config");

        assert_eq!(
            config.ui.sidebar.agents.rows[1],
            vec![
                AgentSidebarToken::StateText,
                AgentSidebarToken::Agent,
                AgentSidebarToken::Custom("summary".into()),
            ]
        );
        assert_eq!(
            config.ui.sidebar.agents.rows[2],
            vec![
                AgentSidebarToken::TerminalTitle,
                AgentSidebarToken::TerminalTitleStripped,
                AgentSidebarToken::Custom("terminal_title".into()),
            ]
        );
        assert_eq!(
            config.ui.sidebar.agents.rows_by_agent["claude"],
            vec![
                vec![AgentSidebarToken::TerminalTitleStripped],
                vec![
                    AgentSidebarToken::Agent,
                    AgentSidebarToken::Custom("model".into()),
                ],
            ]
        );
        assert_eq!(config.ui.sidebar.agents.row_gap, 1);
        assert_eq!(
            config.ui.sidebar.spaces.rows[1],
            vec![SpaceSidebarToken::Custom("jj_status".into())]
        );
        assert_eq!(config.ui.sidebar.spaces.row_gap, 3);
    }

    #[test]
    fn parses_occurrence_styles_without_changing_plain_tokens() {
        let config: crate::config::Config = toml::from_str(
            r##"
[ui.sidebar.agents]
rows = [[{ token = "workspace", fg = "#abc", bold = false }, "workspace"], [{ token = "$summary", dim = false }]]

[ui.sidebar.agents.rows_by_agent]
claude = [[{ token = "agent", fg = "#112233", bold = true, dim = false }]]

[ui.sidebar.spaces]
rows = [[{ token = "git_status", fg = "#ff00aa" }], [{ token = "$jj", bold = true }]]
"##,
        )
        .unwrap();

        let (token, style) = config.ui.sidebar.agents.rows[0][0].parts();
        assert_eq!(token, &AgentSidebarToken::Workspace);
        assert_eq!(style.bold, Some(false));
        assert_eq!(
            style.fg.unwrap().ratatui(),
            ratatui::style::Color::Rgb(0xaa, 0xbb, 0xcc)
        );
        assert_eq!(
            config.ui.sidebar.agents.rows[0][1],
            AgentSidebarToken::Workspace
        );

        let (token, style) = config.ui.sidebar.agents.rows_by_agent["claude"][0][0].parts();
        assert_eq!(token, &AgentSidebarToken::Agent);
        assert_eq!(style.bold, Some(true));
        assert_eq!(style.dim, Some(false));

        let (token, style) = config.ui.sidebar.spaces.rows[0][0].parts();
        assert_eq!(token, &SpaceSidebarToken::GitStatus);
        assert_eq!(
            style.fg.unwrap().ratatui(),
            ratatui::style::Color::Rgb(0xff, 0x00, 0xaa)
        );
        let (token, style) = config.ui.sidebar.spaces.rows[1][0].parts();
        assert_eq!(token, &SpaceSidebarToken::Custom("jj".into()));
        assert_eq!(style.bold, Some(true));
    }

    #[test]
    fn parses_focused_tab_as_plain_and_styled_text_token() {
        let input = r##"
[agents]
rows = [["state_icon", "tab"], ["focused_tab"], [{ token = "focused_tab", fg = "#888", rules = [{ equals = "logs", fg = "#f00" }] }]]
"##;
        let config: SidebarConfig = toml::from_str(input).expect("focused_tab sidebar config");
        assert_eq!(config.agents.rows[1], vec![AgentSidebarToken::FocusedTab]);
        let token = &config.agents.rows[2][0];
        let (base, style) = token.parts();
        assert_eq!(base, &AgentSidebarToken::FocusedTab);
        assert_eq!(
            style.fg.unwrap().ratatui(),
            ratatui::style::Color::Rgb(0x88, 0x88, 0x88)
        );
        assert_eq!(
            token
                .style_for_value("logs", ColorTokens::default(), None)
                .and_then(|style| style.fg)
                .map(SidebarTokenColor::ratatui),
            Some(ratatui::style::Color::Rgb(0xff, 0x00, 0x00))
        );
        assert_eq!(
            token
                .style_for_value("build", ColorTokens::default(), None)
                .and_then(|style| style.fg),
            style.fg
        );
        let encoded = toml::to_string(&config).unwrap();
        assert!(encoded.contains("focused_tab"));
        assert_eq!(toml::from_str::<SidebarConfig>(&encoded).unwrap(), config);
    }

    #[test]
    fn conditional_sidebar_rules_round_trip() {
        let input = r##"
[agents]
rows = [[{ token = "machine", fg = "#fff", rules = [{ equals = "Local", fg = "#f00" }, { starts_with = "fed", ignore_case = true, bold = true }] }]]
[agents.rows_by_agent]
pi = [[{ token = "$load", rules = [{ gt = 80, dim = false }, { lt = 20.5, dim = true }] }]]
[spaces]
rows = [[{ token = "$status", rules = [{ contains = "error", bold = true }] }]]
"##;
        let config: SidebarConfig = toml::from_str(input).expect("conditional sidebar config");
        let encoded = toml::to_string(&config).unwrap();
        assert!(encoded.contains("rules"));
        assert_eq!(toml::from_str::<SidebarConfig>(&encoded).unwrap(), config);
    }

    #[test]
    fn conditional_sidebar_rules_reject_invalid_conditions_and_nontext_tokens() {
        for rule in [
            "{ bold = true }",
            "{ equals = 'x', contains = 'x' }",
            "{ regex = 'x' }",
            "{ gt = '80' }",
            "{ equals = 80 }",
            "{ gt = nan }",
            "{ lt = inf }",
            "{ gt = 80, ignore_case = false }",
            "{ equals = 'x', underline = true }",
            "{ equals = 'x', fg = 'red' }",
        ] {
            let input = format!("[agents]\nrows = [[{{ token = 'machine', rules = [{rule}] }}]]");
            assert!(toml::from_str::<SidebarConfig>(&input).is_err(), "{rule}");
        }
        for (section, token) in [
            ("agents", "state_icon"),
            ("spaces", "state_icon"),
            ("spaces", "git_status"),
        ] {
            let input = format!(
                "[{section}]\nrows = [[{{ token = '{token}', rules = [{{ equals = 'x' }}] }}]]"
            );
            assert!(toml::from_str::<SidebarConfig>(&input).is_err());
        }
        for count in [16, 17] {
            let rules = std::iter::repeat_n("{ equals = 'x' }", count)
                .collect::<Vec<_>>()
                .join(",");
            let input = format!("[agents]\nrows = [[{{ token = 'machine', rules = [{rules}] }}]]");
            assert_eq!(toml::from_str::<SidebarConfig>(&input).is_ok(), count == 16);
        }
    }

    #[test]
    fn rejects_invalid_occurrence_styles() {
        for entry in [
            r##"{ token = "workspace", fg = "red" }"##,
            r##"{ token = "workspace", fg = "#abcd" }"##,
            r##"{ token = "workspace", underline = true }"##,
        ] {
            let input = format!("[ui.sidebar.agents]\nrows = [[{entry}]]\n");
            assert!(
                toml::from_str::<crate::config::Config>(&input).is_err(),
                "accepted {entry}"
            );
        }
    }

    #[test]
    fn rejects_unknown_bare_and_malformed_custom_tokens() {
        for token in ["summary", "$", "$bad.name"] {
            let input = format!("[ui.sidebar.agents]\\nrows = [[\"{token}\"]]\\n");
            assert!(toml::from_str::<crate::config::Config>(&input).is_err());
        }
    }

    #[test]
    fn rejects_oversized_sidebar_layouts() {
        let too_many_rows = std::iter::repeat_n("[\"agent\"]", MAX_SIDEBAR_ROWS + 1)
            .collect::<Vec<_>>()
            .join(",");
        let input = format!("[ui.sidebar.agents]\nrows = [{too_many_rows}]\n");
        assert!(toml::from_str::<crate::config::Config>(&input).is_err());

        let too_many_tokens = std::iter::repeat_n("\"workspace\"", MAX_SIDEBAR_TOKENS_PER_ROW + 1)
            .collect::<Vec<_>>()
            .join(",");
        let input = format!("[ui.sidebar.spaces]\nrows = [[{too_many_tokens}]]\n");
        assert!(toml::from_str::<crate::config::Config>(&input).is_err());

        let input = format!("[ui.sidebar.agents.rows_by_agent]\nclaude = [{too_many_rows}]\n");
        assert!(toml::from_str::<crate::config::Config>(&input).is_err());
    }

    #[test]
    fn accepts_every_canonical_agent_override_key() {
        let agents = Agent::ALL;
        let entries = agents
            .iter()
            .map(|agent| format!("{} = [[\"agent\"]]", crate::detect::agent_label(*agent)))
            .collect::<Vec<_>>()
            .join("\n");
        let input = format!("[ui.sidebar.agents.rows_by_agent]\n{entries}\n");
        let config: crate::config::Config = toml::from_str(&input).expect("canonical keys");

        assert_eq!(config.ui.sidebar.agents.rows_by_agent.len(), agents.len());
    }

    #[test]
    fn rejects_alias_case_whitespace_and_unknown_override_keys() {
        for key in ["claude-code", "Claude", "' claude '", "unknown"] {
            let input = format!("[ui.sidebar.agents.rows_by_agent]\n{key} = [[\"agent\"]]\n");
            assert!(
                toml::from_str::<crate::config::Config>(&input).is_err(),
                "accepted key {key:?}"
            );
        }
    }

    #[test]
    fn fg_accepts_token_reference_and_round_trips() {
        let config: SidebarConfig = toml::from_str(
            r##"
[spaces]
rows = [[{ token = "workspace", fg = "$color", rules = [{ contains = "x", fg = "$other-1" }] }]]
"##,
        )
        .unwrap();
        let (_, style) = config.spaces.rows[0][0].parts();
        assert!(matches!(style.fg, Some(SidebarTokenColor::Token(_))));
        let encoded = toml::to_string(&config).unwrap();
        assert!(encoded.contains(r#"fg = "$color""#));
        assert!(encoded.contains(r#"fg = "$other-1""#));
        assert_eq!(toml::from_str::<SidebarConfig>(&encoded).unwrap(), config);
    }

    #[test]
    fn fg_rejects_invalid_token_reference() {
        for fg in ["$", "$bad.name", "$bad name", "red"] {
            let input =
                format!("[spaces]\nrows = [[{{ token = \"workspace\", fg = \"{fg}\" }}]]\n");
            assert!(toml::from_str::<SidebarConfig>(&input).is_err(), "{fg}");
            let rule = format!("[spaces]\nrows = [[{{ token = \"workspace\", rules = [{{ equals = \"a\", fg = \"{fg}\" }}] }}]]\n");
            assert!(toml::from_str::<SidebarConfig>(&rule).is_err(), "{fg}");
        }
    }

    #[test]
    fn bg_and_panel_color_parse_and_round_trip() {
        let config: SidebarConfig = toml::from_str(
            r##"
[agents]
rows = [["state_icon", { token = "tab", fg = "$panel", bg = "$color", rules = [{ active = true, fg = "$color", bg = "$panel" }] }]]
[spaces]
rows = [[{ token = "workspace", bg = "#123" }, { token = "branch", fg = "#010203", bg = "#a0b0c0" }]]
"##,
        )
        .unwrap();
        let (_, style) = config.agents.rows[0][1].parts();
        assert_eq!(style.fg, Some(SidebarTokenColor::Panel));
        assert!(matches!(style.bg, Some(SidebarTokenColor::Token(_))));
        let (_, style) = config.spaces.rows[0][0].parts();
        assert_eq!(
            style.bg,
            Some(SidebarTokenColor::Rgb {
                r: 0x11,
                g: 0x22,
                b: 0x33
            })
        );
        let encoded = toml::to_string(&config).unwrap();
        assert!(encoded.contains(r#"fg = "$panel""#));
        assert!(encoded.contains(r#"bg = "$color""#));
        assert!(encoded.contains(r#"bg = "$panel""#));
        assert!(encoded.contains("active = true"));
        assert_eq!(toml::from_str::<SidebarConfig>(&encoded).unwrap(), config);
    }

    #[test]
    fn panel_color_resolves_to_the_given_panel_color() {
        use ratatui::style::Color;
        let panel = SidebarTokenColor::Panel;
        assert_eq!(panel.resolve(ColorTokens::default()), Some(panel));
        assert_eq!(
            panel.ratatui_with_panel(Color::Rgb(1, 2, 3)),
            Color::Rgb(1, 2, 3)
        );
    }

    #[test]
    fn terminal_color_parses_chains_round_trips_and_resets() {
        use ratatui::style::Color;
        let config: SidebarConfig = toml::from_str(
            r##"
[spaces]
rows = [[{ token = "workspace", fg = "$color|$terminal", bg = "$terminal" }]]
"##,
        )
        .unwrap();
        let (_, style) = config.spaces.rows[0][0].parts();
        assert_eq!(style.bg, Some(SidebarTokenColor::Terminal));
        let encoded = toml::to_string(&config).unwrap();
        assert!(encoded.contains(r#"bg = "$terminal""#));
        assert!(encoded.contains(r#"fg = "$color|$terminal""#));
        assert_eq!(toml::from_str::<SidebarConfig>(&encoded).unwrap(), config);
        let terminal = SidebarTokenColor::Terminal;
        assert_eq!(terminal.resolve(ColorTokens::default()), Some(terminal));
        assert_eq!(
            terminal.ratatui_with_panel(Color::Rgb(1, 2, 3)),
            Color::Reset
        );
        assert_eq!(
            color("$color|$terminal").resolve(ColorTokens::default()),
            Some(terminal)
        );
        assert!(SidebarTokenColor::parse("$terminal|$color").is_err());
        assert!(SidebarTokenColor::parse("$terminal|#fff").is_err());
    }

    fn color(value: &str) -> SidebarTokenColor {
        SidebarTokenColor::parse(value).unwrap()
    }

    #[test]
    fn fallback_chain_parses_two_and_three_alternatives_and_round_trips() {
        let config: SidebarConfig = toml::from_str(
            r##"
[spaces]
rows = [[{ token = "workspace", fg = "$color|#ffffff", bg = "$accent|$color|#89b4fa", rules = [{ active = true, fg = "$color|$panel" }] }]]
"##,
        )
        .unwrap();
        let (_, style) = config.spaces.rows[0][0].parts();
        assert!(matches!(style.fg, Some(SidebarTokenColor::Fallback(_))));
        assert!(matches!(style.bg, Some(SidebarTokenColor::Fallback(_))));
        let encoded = toml::to_string(&config).unwrap();
        assert!(encoded.contains(r##"fg = "$color|#ffffff""##));
        assert!(encoded.contains(r##"bg = "$accent|$color|#89b4fa""##));
        assert!(encoded.contains(r#"fg = "$color|$panel""#));
        assert_eq!(toml::from_str::<SidebarConfig>(&encoded).unwrap(), config);
    }

    #[test]
    fn single_value_parses_as_before() {
        assert_eq!(color("$panel"), SidebarTokenColor::Panel);
        assert!(matches!(color("$color"), SidebarTokenColor::Token(_)));
        assert_eq!(
            color("#fff"),
            SidebarTokenColor::Rgb {
                r: 255,
                g: 255,
                b: 255
            }
        );
    }

    #[test]
    fn fallback_chain_rejects_invalid_alternatives() {
        for value in [
            "|",
            "$color|",
            "|#fff",
            "$color||#fff",
            "$bad.name|#fff",
            "$color|red",
            "$color|#abcd",
            "#fff|$color",
            "$panel|$color",
            "$color | #fff",
            " $color|#fff",
            "$a|$b|$c|$d",
        ] {
            assert!(SidebarTokenColor::parse(value).is_err(), "{value}");
            let input =
                format!("[spaces]\nrows = [[{{ token = \"workspace\", fg = \"{value}\" }}]]\n");
            assert!(toml::from_str::<SidebarConfig>(&input).is_err(), "{value}");
        }
    }

    #[test]
    fn fallback_chain_resolves_first_available_alternative() {
        let white = SidebarTokenColor::Rgb {
            r: 255,
            g: 255,
            b: 255,
        };
        let pink = SidebarTokenColor::Rgb {
            r: 0xf3,
            g: 0x8b,
            b: 0xa8,
        };
        let chain = color("$color|#ffffff");
        assert_eq!(chain.resolve(ColorTokens::default()), Some(white));
        let tokens = std::collections::HashMap::from([("color".to_owned(), "#f38ba8".to_owned())]);
        assert_eq!(chain.resolve(ColorTokens::new(&tokens)), Some(pink));
        // A non-color token value falls through to the next alternative.
        let tokens = std::collections::HashMap::from([("color".to_owned(), "red".to_owned())]);
        assert_eq!(chain.resolve(ColorTokens::new(&tokens)), Some(white));
        assert_eq!(
            color("$color|$panel").resolve(ColorTokens::default()),
            Some(SidebarTokenColor::Panel)
        );
        assert_eq!(color("$a|$b").resolve(ColorTokens::default()), None);
    }

    #[test]
    fn bg_rejects_invalid_values() {
        for bg in ["$", "$bad.name", "red", "#abcd", "panel"] {
            let input =
                format!("[spaces]\nrows = [[{{ token = \"workspace\", bg = \"{bg}\" }}]]\n");
            assert!(toml::from_str::<SidebarConfig>(&input).is_err(), "{bg}");
            let rule = format!("[spaces]\nrows = [[{{ token = \"workspace\", rules = [{{ equals = \"a\", bg = \"{bg}\" }}] }}]]\n");
            assert!(toml::from_str::<SidebarConfig>(&rule).is_err(), "{bg}");
        }
    }
}
