use serde::{Deserialize, Serialize};

use super::{ColorTokens, SidebarTokenColor, SidebarTokenStyle};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "RawRule", into = "RawRule")]
pub struct SidebarTokenRule {
    condition: Condition,
    ignore_case: bool,
    style: SidebarTokenStyle,
    hide: Option<bool>,
}

// Deserialization rejects non-finite thresholds, so equality is reflexive.
impl Eq for SidebarTokenRule {}

#[derive(Debug, Clone, PartialEq)]
enum Condition {
    Equals(String),
    Contains(String),
    StartsWith(String),
    GreaterThan(f64),
    LessThan(f64),
    /// The tab shown by the token is (or is not) active in its workspace.
    Active(bool),
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawRule {
    #[serde(skip_serializing_if = "Option::is_none")]
    equals: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    contains: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    starts_with: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    gt: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    lt: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    active: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ignore_case: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    fg: Option<SidebarTokenColor>,
    #[serde(skip_serializing_if = "Option::is_none")]
    bg: Option<SidebarTokenColor>,
    #[serde(skip_serializing_if = "Option::is_none")]
    bold: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    dim: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    hide: Option<bool>,
}

impl TryFrom<RawRule> for SidebarTokenRule {
    type Error = String;

    fn try_from(raw: RawRule) -> Result<Self, Self::Error> {
        let count = [
            raw.equals.is_some(),
            raw.contains.is_some(),
            raw.starts_with.is_some(),
            raw.gt.is_some(),
            raw.lt.is_some(),
            raw.active.is_some(),
        ]
        .into_iter()
        .filter(|present| *present)
        .count();
        if count != 1 {
            return Err(
                "sidebar rule requires exactly one of equals, contains, starts_with, gt, lt, active"
                    .into(),
            );
        }
        let condition = if let Some(value) = raw.equals {
            Condition::Equals(value)
        } else if let Some(value) = raw.contains {
            Condition::Contains(value)
        } else if let Some(value) = raw.starts_with {
            Condition::StartsWith(value)
        } else if let Some(active) = raw.active {
            if raw.ignore_case.is_some() {
                return Err("ignore_case applies only to sidebar text conditions".into());
            }
            Condition::Active(active)
        } else {
            let (value, greater) = match (raw.gt, raw.lt) {
                (Some(value), _) => (value, true),
                (_, Some(value)) => (value, false),
                _ => unreachable!("validated condition count"),
            };
            if !value.is_finite() {
                return Err("sidebar numeric rule threshold must be finite".into());
            }
            if raw.ignore_case.is_some() {
                return Err("ignore_case applies only to sidebar text conditions".into());
            }
            if greater {
                Condition::GreaterThan(value)
            } else {
                Condition::LessThan(value)
            }
        };
        Ok(Self {
            condition,
            ignore_case: raw.ignore_case.unwrap_or(false),
            hide: raw.hide,
            style: SidebarTokenStyle {
                fg: raw.fg,
                bg: raw.bg,
                bold: raw.bold,
                dim: raw.dim,
            },
        })
    }
}

impl From<SidebarTokenRule> for RawRule {
    fn from(rule: SidebarTokenRule) -> Self {
        let mut raw = Self {
            ignore_case: rule.ignore_case.then_some(true),
            fg: rule.style.fg,
            bg: rule.style.bg,
            bold: rule.style.bold,
            dim: rule.style.dim,
            hide: rule.hide,
            ..Self::default()
        };
        match rule.condition {
            Condition::Equals(value) => raw.equals = Some(value),
            Condition::Contains(value) => raw.contains = Some(value),
            Condition::StartsWith(value) => raw.starts_with = Some(value),
            Condition::GreaterThan(value) => raw.gt = Some(value),
            Condition::LessThan(value) => raw.lt = Some(value),
            Condition::Active(value) => raw.active = Some(value),
        }
        raw
    }
}

impl SidebarTokenRule {
    fn matches(
        &self,
        value: &str,
        numeric: &mut Option<Option<f64>>,
        active: Option<bool>,
    ) -> bool {
        match &self.condition {
            Condition::Active(expected) => active == Some(*expected),
            Condition::Equals(expected) => {
                if self.ignore_case {
                    value.eq_ignore_ascii_case(expected)
                } else {
                    value == expected
                }
            }
            Condition::StartsWith(expected) => {
                if self.ignore_case {
                    value
                        .as_bytes()
                        .get(..expected.len())
                        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(expected.as_bytes()))
                } else {
                    value.starts_with(expected)
                }
            }
            Condition::Contains(expected) => {
                if self.ignore_case {
                    expected.is_empty()
                        || value
                            .as_bytes()
                            .windows(expected.len())
                            .any(|part| part.eq_ignore_ascii_case(expected.as_bytes()))
                } else {
                    value.contains(expected)
                }
            }
            Condition::GreaterThan(threshold) | Condition::LessThan(threshold) => {
                let parsed = numeric.get_or_insert_with(|| {
                    value
                        .parse::<f64>()
                        .ok()
                        .filter(|number| number.is_finite())
                });
                parsed.is_some_and(|number| match self.condition {
                    Condition::GreaterThan(_) => number > *threshold,
                    _ => number < *threshold,
                })
            }
        }
    }
}

pub(super) fn matching_style(
    rules: &[SidebarTokenRule],
    base: SidebarTokenStyle,
    value: &str,
    tokens: ColorTokens<'_>,
    active: Option<bool>,
) -> Option<SidebarTokenStyle> {
    let base = base.resolved(tokens);
    let mut numeric = None;
    for rule in rules {
        if rule.matches(value, &mut numeric, active) {
            if rule.hide == Some(true) {
                return None;
            }
            return Some(SidebarTokenStyle {
                fg: rule.style.fg.and_then(|fg| fg.resolve(tokens)).or(base.fg),
                bg: rule.style.bg.and_then(|bg| bg.resolve(tokens)).or(base.bg),
                bold: rule.style.bold.or(base.bold),
                dim: rule.style.dim.or(base.dim),
            });
        }
    }
    Some(base)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn string_conditions_use_exact_case_or_ascii_folding() {
        for (condition, yes, no) in [
            ("equals = 'Local'", "Local", "Localhost"),
            ("contains = 'Local'", "myLocalbox", "remote"),
            ("starts_with = 'Local'", "Localhost", "myLocal"),
        ] {
            let rule: SidebarTokenRule = toml::from_str(condition).unwrap();
            assert!(rule.matches(yes, &mut None, None));
            assert!(!rule.matches(no, &mut None, None));
            assert!(!rule.matches(&yes.to_ascii_lowercase(), &mut None, None));
            let folded: SidebarTokenRule =
                toml::from_str(&format!("{condition}\nignore_case = true")).unwrap();
            assert!(folded.matches(&yes.to_ascii_lowercase(), &mut None, None));
        }
        for condition in ["equals", "contains", "starts_with"] {
            let rule: SidebarTokenRule =
                toml::from_str(&format!("{condition} = 'ÉA'\nignore_case = true")).unwrap();
            assert!(rule.matches("Éa", &mut None, None));
            assert!(!rule.matches("éa", &mut None, None));
        }
        let empty: SidebarTokenRule = toml::from_str("contains = ''\nignore_case = true").unwrap();
        assert!(empty.matches("", &mut None, None));
    }

    #[test]
    fn numeric_conditions_require_full_finite_numbers_and_strict_comparison() {
        for (condition, yes, no) in [
            ("gt = 80", ["90", "8.1e1", "+90"], "70"),
            ("lt = 80", ["70", "7.9e1", "-90"], "90"),
        ] {
            let rule: SidebarTokenRule = toml::from_str(condition).unwrap();
            for value in yes {
                assert!(rule.matches(value, &mut None, None), "{condition}: {value}");
            }
            for value in [
                no, "80", "90%", " 90", "90 ", "", "NaN", "inf", "-inf", "1e999",
            ] {
                assert!(
                    !rule.matches(value, &mut None, None),
                    "{condition}: {value}"
                );
            }
        }
    }

    #[test]
    fn active_condition_matches_only_tokens_that_show_a_tab() {
        let active: SidebarTokenRule = toml::from_str("active = true").unwrap();
        let inactive: SidebarTokenRule = toml::from_str("active = false").unwrap();
        assert!(active.matches("x", &mut None, Some(true)));
        assert!(!active.matches("x", &mut None, Some(false)));
        assert!(inactive.matches("x", &mut None, Some(false)));
        assert!(!inactive.matches("x", &mut None, Some(true)));
        for rule in [&active, &inactive] {
            assert!(!rule.matches("x", &mut None, None));
        }
        let encoded = toml::to_string(&active).unwrap();
        assert_eq!(
            toml::from_str::<SidebarTokenRule>(&encoded).unwrap(),
            active
        );
    }

    #[test]
    fn active_condition_is_exclusive_and_takes_no_ignore_case() {
        for input in [
            "active = true\nequals = 'a'",
            "active = false\ngt = 1",
            "active = true\nignore_case = true",
        ] {
            assert!(
                toml::from_str::<SidebarTokenRule>(input).is_err(),
                "{input}"
            );
        }
    }

    #[test]
    fn rules_merge_bg_and_hide_works_with_active() {
        let rules: Vec<SidebarTokenRule> = vec![
            toml::from_str("active = true\nbg = '$panel'").unwrap(),
            toml::from_str("active = false\nhide = true").unwrap(),
        ];
        let base = SidebarTokenStyle {
            bg: Some(SidebarTokenColor::Rgb { r: 1, g: 2, b: 3 }),
            ..Default::default()
        };
        let tokens = ColorTokens::default();
        assert_eq!(
            matching_style(&rules, base, "x", tokens, Some(true))
                .unwrap()
                .bg,
            Some(SidebarTokenColor::Panel)
        );
        assert_eq!(matching_style(&rules, base, "x", tokens, Some(false)), None);
        assert_eq!(matching_style(&rules, base, "x", tokens, None), Some(base));
    }
}
