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
#[serde(deny_unknown_fields)]
pub struct ScreenCursorShapeRule {
    #[serde(deserialize_with = "deserialize_rule_text")]
    pub contains: String,
    pub shape: ImeCursorShape,
    /// Restricts the match to this many bottom non-empty lines of the screen,
    /// so conversation text above the status line cannot match. Default: all lines.
    #[serde(default)]
    pub bottom_non_empty_lines: Option<NonZeroUsize>,
}

fn deserialize_rule_text<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    if value.trim().is_empty() {
        return Err(serde::de::Error::custom(
            "cursor shape rule `contains` must not be empty",
        ));
    }
    if value.contains('\n') {
        return Err(serde::de::Error::custom(
            "cursor shape rule `contains` must be a single line",
        ));
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
    /// DECSCUSR parameter this screen selects; `None` leaves the pane's own shape.
    pub fn shape_for_screen(&self, screen: &str) -> Option<u8> {
        self.rules
            .iter()
            .find(|rule| rule.matches(screen))
            .map(|rule| rule.shape)
            .or(self.default)
            .map(ImeCursorShape::to_decscusr)
    }
}

impl ScreenCursorShapeRule {
    fn matches(&self, screen: &str) -> bool {
        let needle = self.contains.as_str();
        match self.bottom_non_empty_lines {
            Some(limit) => screen
                .lines()
                .rev()
                .filter(|line| !line.trim().is_empty())
                .take(limit.get())
                .any(|line| line.contains(needle)),
            None => screen.lines().any(|line| line.contains(needle)),
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
                    shape: *shape,
                    bottom_non_empty_lines: None,
                })
                .collect(),
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
        assert_eq!(claude.rules[0].shape, ImeCursorShape::SteadyBar);
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
        assert_eq!(config.shape_for_screen("x second\ny first\n"), Some(6));
        assert_eq!(config.shape_for_screen("x second\n"), Some(3));
        assert_eq!(config.shape_for_screen("nothing\n"), Some(2));
    }

    #[test]
    fn without_match_or_default_the_shape_is_untouched() {
        let config = rules(None, &[("marker", ImeCursorShape::Bar)]);
        assert_eq!(config.shape_for_screen("nothing\n"), None);
        assert_eq!(config.shape_for_screen(""), None);
    }

    #[test]
    fn substring_does_not_match_across_lines() {
        let config = rules(None, &[("a b", ImeCursorShape::Bar)]);
        assert_eq!(config.shape_for_screen("a\nb\n"), None);
    }

    #[test]
    fn bottom_non_empty_lines_ignores_text_above_and_blank_tail() {
        let mut config = rules(None, &[("marker", ImeCursorShape::Bar)]);
        config.rules[0].bottom_non_empty_lines = NonZeroUsize::new(2);
        assert_eq!(config.shape_for_screen("marker\nb\nc\n"), None);
        assert_eq!(config.shape_for_screen("a\nmarker\nc\n"), Some(5));
        assert_eq!(config.shape_for_screen("a\nmarker\n   \n\nc\n\n"), Some(5));
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
