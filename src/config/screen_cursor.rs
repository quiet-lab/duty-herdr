//! Cursor shape rules matched against an agent's detection screen.

use std::collections::BTreeMap;
use std::num::NonZeroUsize;
use std::sync::Arc;

use serde::Deserialize;

use super::model::ImeCursorShape;

/// Cursor shape rules of one agent. Rules are tried in order, the first match
/// wins; without a match the pane uses `default`, and without `default` it keeps
/// its own shape.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ScreenCursorShapeConfig {
    pub default: Option<ImeCursorShape>,
    pub rules: Vec<ScreenCursorShapeRule>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(try_from = "RawScreenCursorShapeRule")]
pub struct ScreenCursorShapeRule {
    pub contains: String,
    pub shape: ScreenRuleShape,
    /// Restricts the match to this many bottom non-empty lines of the screen,
    /// so conversation text above the status line cannot match. Default: all lines.
    #[serde(default)]
    pub bottom_non_empty_lines: Option<NonZeroUsize>,
    /// Restricts the match to the lines below the last screen line containing
    /// this text (for Claude Code, the lower border of the input box). Without
    /// such a line on the screen the rule does not match. Combined with
    /// `bottom_non_empty_lines`, that limit counts inside this area.
    pub after_last_line_containing: Option<String>,
    /// Restricts the match to this many top non-empty lines of the area below
    /// `after_last_line_containing` (Claude Code: the status line is the first
    /// non-empty line under the input box border). Requires
    /// `after_last_line_containing`; excludes `bottom_non_empty_lines`.
    pub top_non_empty_lines: Option<NonZeroUsize>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawScreenCursorShapeRule {
    #[serde(deserialize_with = "deserialize_rule_text")]
    contains: String,
    shape: ScreenRuleShape,
    #[serde(default)]
    bottom_non_empty_lines: Option<NonZeroUsize>,
    #[serde(default, deserialize_with = "deserialize_optional_rule_text")]
    after_last_line_containing: Option<String>,
    #[serde(default)]
    top_non_empty_lines: Option<NonZeroUsize>,
}

impl TryFrom<RawScreenCursorShapeRule> for ScreenCursorShapeRule {
    type Error = String;

    fn try_from(raw: RawScreenCursorShapeRule) -> Result<Self, String> {
        if raw.top_non_empty_lines.is_some() && raw.after_last_line_containing.is_none() {
            return Err(
                "cursor shape rule `top_non_empty_lines` requires `after_last_line_containing`"
                    .into(),
            );
        }
        if raw.top_non_empty_lines.is_some() && raw.bottom_non_empty_lines.is_some() {
            return Err(
                "cursor shape rule `top_non_empty_lines` and `bottom_non_empty_lines` are mutually exclusive"
                    .into(),
            );
        }
        Ok(Self {
            contains: raw.contains,
            shape: raw.shape,
            bottom_non_empty_lines: raw.bottom_non_empty_lines,
            after_last_line_containing: raw.after_last_line_containing,
            top_non_empty_lines: raw.top_non_empty_lines,
        })
    }
}

/// Shape of a rule: a cursor shape, or `keep` to leave the shape the screen
/// already selected. `keep` is not an `ImeCursorShape` value, which
/// `cjk_ime_cursor_shape` shares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenRuleShape {
    Keep,
    Shape(ImeCursorShape),
}

impl<'de> Deserialize<'de> for ScreenRuleShape {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        if value == "keep" {
            return Ok(Self::Keep);
        }
        let deserializer =
            serde::de::IntoDeserializer::<D::Error>::into_deserializer(value.as_str());
        // A string can only fail as an unknown name; list `keep` among the valid ones.
        ImeCursorShape::deserialize(deserializer)
            .map(Self::Shape)
            .map_err(|_: D::Error| {
                serde::de::Error::unknown_variant(
                    &value,
                    &[
                        "keep",
                        "block",
                        "steady_block",
                        "underline",
                        "steady_underline",
                        "bar",
                        "steady_bar",
                    ],
                )
            })
    }
}

/// Outcome of evaluating a screen against the rules of one agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenShapeDecision {
    /// A `keep` rule matched: the shape already applied to the pane stays;
    /// `fallback` (the table's `default`) applies only while none is applied yet.
    Keep { fallback: Option<u8> },
    /// DECSCUSR parameter to apply; `None` leaves the pane's own shape.
    Set(Option<u8>),
}

fn validate_rule_text<E: serde::de::Error>(field: &str, value: &str) -> Result<(), E> {
    if value.trim().is_empty() {
        return Err(E::custom(format!(
            "cursor shape rule `{field}` must not be empty"
        )));
    }
    if value.contains('\n') {
        return Err(E::custom(format!(
            "cursor shape rule `{field}` must be a single line"
        )));
    }
    Ok(())
}

fn deserialize_rule_text<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    validate_rule_text("contains", &value)?;
    Ok(value)
}

fn deserialize_optional_rule_text<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<String>::deserialize(deserializer)?;
    if let Some(value) = &value {
        validate_rule_text("after_last_line_containing", value)?;
    }
    Ok(value)
}

pub(super) fn deserialize_cursor_shape_by_screen<'de, D>(
    deserializer: D,
) -> Result<BTreeMap<String, ScreenCursorShapeConfig>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let by_agent = BTreeMap::<String, ScreenCursorShapeConfig>::deserialize(deserializer)?;
    for id in by_agent.keys() {
        if crate::detect::parse_canonical_agent_label(id).is_none() {
            return Err(serde::de::Error::custom(format!(
                "unknown canonical agent id `{id}` in ui.cursor_shape_by_screen"
            )));
        }
    }
    Ok(by_agent)
}

impl ScreenCursorShapeConfig {
    /// Shape this screen selects: the first matching rule, else `default`;
    /// without both the pane's own shape stays (`Set(None)`).
    pub fn shape_for_screen(&self, screen: &str) -> ScreenShapeDecision {
        match self.rules.iter().find(|rule| rule.matches(screen)) {
            Some(rule) => match rule.shape {
                ScreenRuleShape::Keep => ScreenShapeDecision::Keep {
                    fallback: self.default.map(ImeCursorShape::to_decscusr),
                },
                ScreenRuleShape::Shape(shape) => {
                    ScreenShapeDecision::Set(Some(shape.to_decscusr()))
                }
            },
            None => ScreenShapeDecision::Set(self.default.map(ImeCursorShape::to_decscusr)),
        }
    }
}

impl ScreenCursorShapeRule {
    fn matches(&self, screen: &str) -> bool {
        let needle = self.contains.as_str();
        // Lines below the last separator, counted from the bottom.
        let area = match self.after_last_line_containing.as_deref() {
            Some(separator) => {
                match screen
                    .lines()
                    .rev()
                    .position(|line| line.contains(separator))
                {
                    Some(below) => below,
                    None => return false,
                }
            }
            None => usize::MAX,
        };
        let mut lines = screen.lines().rev().take(area);
        if let Some(top) = self.top_non_empty_lines {
            // The area is the last `area` lines of the screen: skip the rest.
            let skip = screen.lines().count() - area;
            return screen
                .lines()
                .skip(skip)
                .filter(|line| !line.trim().is_empty())
                .take(top.get())
                .any(|line| line.contains(needle));
        }
        match self.bottom_non_empty_lines {
            Some(limit) => lines
                .filter(|line| !line.trim().is_empty())
                .take(limit.get())
                .any(|line| line.contains(needle)),
            None => lines.any(|line| line.contains(needle)),
        }
    }
}

/// Rules shared with pane runtimes; replacing the `Arc`s makes each pane
/// evaluate the screen again.
#[derive(Debug, Clone, Default)]
pub struct ScreenCursorRules(BTreeMap<String, Arc<ScreenCursorShapeConfig>>);

impl ScreenCursorRules {
    pub fn from_config(by_agent: &BTreeMap<String, ScreenCursorShapeConfig>) -> Self {
        Self(
            by_agent
                .iter()
                .map(|(id, config)| (id.clone(), Arc::new(config.clone())))
                .collect(),
        )
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn for_agent(&self, agent: crate::detect::Agent) -> Option<Arc<ScreenCursorShapeConfig>> {
        self.0.get(crate::detect::agent_label(agent)).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    fn parse(toml_text: &str) -> Result<Config, toml::de::Error> {
        toml::from_str(toml_text)
    }

    fn rules(
        default: Option<ImeCursorShape>,
        rules: &[(&str, ImeCursorShape)],
    ) -> ScreenCursorShapeConfig {
        ScreenCursorShapeConfig {
            default,
            rules: rules
                .iter()
                .map(|(contains, shape)| ScreenCursorShapeRule {
                    contains: (*contains).into(),
                    shape: ScreenRuleShape::Shape(*shape),
                    bottom_non_empty_lines: None,
                    after_last_line_containing: None,
                    top_non_empty_lines: None,
                })
                .collect(),
        }
    }

    fn set(shape: Option<u8>) -> ScreenShapeDecision {
        ScreenShapeDecision::Set(shape)
    }

    const CLAUDE_RULES: &str = r#"
[ui.cursor_shape_by_screen.claude]
default = "steady_block"
rules = [
  { contains = "-- INSERT --", shape = "steady_bar", after_last_line_containing = "────" },
  { contains = "Press Ctrl-C again to exit", shape = "keep", after_last_line_containing = "────" },
]
"#;

    fn claude() -> ScreenCursorShapeConfig {
        parse(CLAUDE_RULES)
            .unwrap()
            .ui
            .cursor_shape_by_screen
            .remove("claude")
            .unwrap()
    }

    const INPUT_BOX: &str = "conversation\n────────\n❯ hi\n────────\n";

    #[test]
    fn after_last_line_containing_sees_footer_lines_below_the_separator() {
        let claude = claude();
        for extra in [
            "",
            "\n  ● main",
            "\n  ● main\n  ◯ agent one",
            "\n\n  ● main\n  ◯ a\n  ◯ b",
        ] {
            let screen = format!("{INPUT_BOX}  -- INSERT -- ⏵⏵ bypass{extra}\n");
            assert_eq!(claude.shape_for_screen(&screen), set(Some(6)), "{extra:?}");
        }
        let normal = format!("{INPUT_BOX}  ⏵⏵ bypass\n  ● main\n");
        assert_eq!(claude.shape_for_screen(&normal), set(Some(2)));
    }

    #[test]
    fn after_last_line_containing_ignores_text_above_the_input_box() {
        let claude = claude();
        let screen = "-- INSERT -- in conversation\n────────\n❯ hi\n────────\n  ⏵⏵ bypass\n";
        assert_eq!(claude.shape_for_screen(screen), set(Some(2)));
    }

    #[test]
    fn after_last_line_containing_needs_the_separator_on_screen() {
        let claude = claude();
        assert_eq!(claude.shape_for_screen("  -- INSERT --\n"), set(Some(2)));
    }

    #[test]
    fn bottom_non_empty_lines_counts_inside_the_area() {
        let mut claude = claude();
        claude.rules[0].bottom_non_empty_lines = NonZeroUsize::new(2);
        let near = format!("{INPUT_BOX}  -- INSERT --\n  ● main\n\n");
        assert_eq!(claude.shape_for_screen(&near), set(Some(6)));
        let far = format!("{INPUT_BOX}  -- INSERT --\n  ● a\n  ● b\n");
        assert_eq!(claude.shape_for_screen(&far), set(Some(2)));
    }

    #[test]
    fn keep_rule_yields_keep_decision() {
        let claude = claude();
        let screen = format!("{INPUT_BOX}  Press Ctrl-C again to exit\n");
        assert_eq!(
            claude.shape_for_screen(&screen),
            ScreenShapeDecision::Keep { fallback: Some(2) }
        );
    }

    fn claude_top() -> ScreenCursorShapeConfig {
        parse(
            r#"
[ui.cursor_shape_by_screen.claude]
default = "steady_block"
rules = [
  { contains = "-- INSERT --", shape = "steady_bar", after_last_line_containing = "────", top_non_empty_lines = 1 },
  { contains = "! for shell mode", shape = "keep", after_last_line_containing = "────", top_non_empty_lines = 1 },
]
"#,
        )
        .unwrap()
        .ui
        .cursor_shape_by_screen
        .remove("claude")
        .unwrap()
    }

    #[test]
    fn top_non_empty_lines_reads_only_the_status_line() {
        let claude = claude_top();
        let insert = format!("{INPUT_BOX}  -- INSERT -- ⏵⏵\n\n  ● main\n  ◯ a\n  ◯ b\n");
        assert_eq!(claude.shape_for_screen(&insert), set(Some(6)));
        let dialog = "────────\n Apply edit?\n  -- INSERT --\n+ diff\n";
        assert_eq!(claude.shape_for_screen(dialog), set(Some(2)));
        let help = format!("{INPUT_BOX}  ! for shell mode\n  / for commands\n");
        assert_eq!(
            claude.shape_for_screen(&help),
            ScreenShapeDecision::Keep { fallback: Some(2) }
        );
    }

    #[test]
    fn rejects_invalid_top_non_empty_lines() {
        for (case, expected) in [
            (
                "[ui.cursor_shape_by_screen.claude]\nrules = [{ contains = \"x\", shape = \"bar\", top_non_empty_lines = 1 }]",
                "requires `after_last_line_containing`",
            ),
            (
                "[ui.cursor_shape_by_screen.claude]\nrules = [{ contains = \"x\", shape = \"bar\", after_last_line_containing = \"-\", top_non_empty_lines = 1, bottom_non_empty_lines = 1 }]",
                "mutually exclusive",
            ),
            (
                "[ui.cursor_shape_by_screen.claude]\nrules = [{ contains = \"x\", shape = \"beam\" }]",
                "`keep`",
            ),
        ] {
            let message = parse(case).unwrap_err().to_string();
            assert!(message.contains(expected), "{case}: {message}");
        }
        let ok = claude_top();
        assert_eq!(ok.rules[0].top_non_empty_lines, NonZeroUsize::new(1));
    }

    #[test]
    fn parses_keep_and_area_fields() {
        let claude = claude();
        assert_eq!(claude.rules[1].shape, ScreenRuleShape::Keep);
        assert_eq!(
            claude.rules[0].after_last_line_containing.as_deref(),
            Some("────")
        );
    }

    #[test]
    fn rejects_keep_default_and_invalid_area_text() {
        for (case, expected) in [
            (
                "[ui.cursor_shape_by_screen.claude]\ndefault = \"keep\"",
                "unknown variant `keep`",
            ),
            (
                "[ui.cursor_shape_by_screen.claude]\nrules = [{ contains = \"x\", shape = \"bar\", after_last_line_containing = \"\" }]",
                "after_last_line_containing",
            ),
            (
                "[ui.cursor_shape_by_screen.claude]\nrules = [{ contains = \"x\", shape = \"bar\", after_last_line_containing = \"a\\nb\" }]",
                "single line",
            ),
        ] {
            let message = parse(case).unwrap_err().to_string();
            assert!(message.contains(expected), "{case}: {message}");
        }
    }

    #[test]
    fn parses_rules_by_agent() {
        let config = parse(
            r#"
[ui.cursor_shape_by_screen.claude]
default = "steady_block"
rules = [
  { contains = "marker", shape = "steady_bar", bottom_non_empty_lines = 2 },
  { contains = "other", shape = "underline" },
]
"#,
        )
        .unwrap();
        let claude = &config.ui.cursor_shape_by_screen["claude"];
        assert_eq!(claude.default, Some(ImeCursorShape::SteadyBlock));
        assert_eq!(claude.rules.len(), 2);
        assert_eq!(
            claude.rules[0].shape,
            ScreenRuleShape::Shape(ImeCursorShape::SteadyBar)
        );
        assert_eq!(claude.rules[0].bottom_non_empty_lines, NonZeroUsize::new(2));
        assert_eq!(claude.rules[1].bottom_non_empty_lines, None);
    }

    #[test]
    fn missing_table_means_no_rules() {
        assert!(parse("").unwrap().ui.cursor_shape_by_screen.is_empty());
    }

    #[test]
    fn rejects_invalid_rules_with_a_clear_message() {
        for (case, expected) in [
            (
                "[ui.cursor_shape_by_screen.claude]\nrules = [{ contains = \"\", shape = \"bar\" }]",
                "must not be empty",
            ),
            (
                "[ui.cursor_shape_by_screen.claude]\nrules = [{ contains = \"a\\nb\", shape = \"bar\" }]",
                "single line",
            ),
            (
                "[ui.cursor_shape_by_screen.claude]\nrules = [{ contains = \"x\", shape = \"beam\" }]",
                "unknown variant `beam`",
            ),
            (
                "[ui.cursor_shape_by_screen.claude]\ndefault = \"beam\"",
                "unknown variant `beam`",
            ),
            (
                "[ui.cursor_shape_by_screen.claude]\nrules = [{ contains = \"x\", shape = \"bar\", regex = true }]",
                "unknown field `regex`",
            ),
            (
                "[ui.cursor_shape_by_screen.claude]\nfallback = \"bar\"",
                "unknown field `fallback`",
            ),
            (
                "[ui.cursor_shape_by_screen.claude]\nrules = [{ contains = \"x\", shape = \"bar\", bottom_non_empty_lines = 0 }]",
                "bottom_non_empty_lines",
            ),
            (
                "[ui.cursor_shape_by_screen.nonsense]\ndefault = \"bar\"",
                "unknown canonical agent id `nonsense`",
            ),
        ] {
            let message = parse(case).unwrap_err().to_string();
            assert!(message.contains(expected), "{case}: {message}");
        }
    }

    #[test]
    fn first_matching_rule_wins_over_later_rules_and_default() {
        let config = rules(
            Some(ImeCursorShape::SteadyBlock),
            &[
                ("first", ImeCursorShape::SteadyBar),
                ("second", ImeCursorShape::Underline),
            ],
        );
        assert_eq!(config.shape_for_screen("x second\ny first\n"), set(Some(6)));
        assert_eq!(config.shape_for_screen("x second\n"), set(Some(3)));
        assert_eq!(config.shape_for_screen("nothing\n"), set(Some(2)));
    }

    #[test]
    fn without_match_or_default_the_shape_is_untouched() {
        let config = rules(None, &[("marker", ImeCursorShape::Bar)]);
        assert_eq!(config.shape_for_screen("nothing\n"), set(None));
        assert_eq!(config.shape_for_screen(""), set(None));
    }

    #[test]
    fn substring_does_not_match_across_lines() {
        let config = rules(None, &[("a b", ImeCursorShape::Bar)]);
        assert_eq!(config.shape_for_screen("a\nb\n"), set(None));
    }

    #[test]
    fn bottom_non_empty_lines_ignores_text_above_and_blank_tail() {
        let mut config = rules(None, &[("marker", ImeCursorShape::Bar)]);
        config.rules[0].bottom_non_empty_lines = NonZeroUsize::new(2);
        assert_eq!(config.shape_for_screen("marker\nb\nc\n"), set(None));
        assert_eq!(config.shape_for_screen("a\nmarker\nc\n"), set(Some(5)));
        assert_eq!(
            config.shape_for_screen("a\nmarker\n   \n\nc\n\n"),
            set(Some(5))
        );
    }

    #[test]
    fn replaced_rules_are_distinct_for_panes() {
        let by_agent = BTreeMap::from([("claude".to_string(), ScreenCursorShapeConfig::default())]);
        let first = ScreenCursorRules::from_config(&by_agent);
        let second = ScreenCursorRules::from_config(&by_agent);
        let agent = crate::detect::Agent::Claude;
        assert!(!Arc::ptr_eq(
            &first.for_agent(agent).unwrap(),
            &second.for_agent(agent).unwrap()
        ));
        assert!(first.for_agent(crate::detect::Agent::Codex).is_none());
        assert!(ScreenCursorRules::default().is_empty());
    }
}
