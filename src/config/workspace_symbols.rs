//! `[ui.workspace_symbols]`: the symbols and colors Herdr assigns to workspaces.
//!
//! ```toml
//! [[ui.workspace_symbols.dirs]]
//! path = "~/work/pets/duty-herdr"
//! main = "🦁"
//! tasks = ["🦄", "🦌", "🦊"]
//!
//! [ui.workspace_symbols.outside]
//! main = "🏠"
//! tasks = ["🐟", "🐠"]
//!
//! [ui.workspace_symbols.color_rule]
//! hue_step = 10
//! saturation = [50, 70]
//! value = [70, 100]
//! ```
//!
//! Values are validated while parsing, so an invalid entry rejects the `[ui]`
//! section the same way an invalid `[ui.status_icons]` entry does.

use serde::Deserialize;

/// Symbol sets keyed by directory plus the set for workspaces outside them.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct WorkspaceSymbolsConfig {
    /// Per-directory sets; the longest matching `path` wins.
    pub dirs: Vec<WorkspaceSymbolDirConfig>,
    /// Set for workspaces whose directory matches no `dirs` entry.
    pub outside: Option<WorkspaceSymbolSetConfig>,
    /// How the color of a child workspace is chosen.
    pub color_rule: WorkspaceColorRule,
}

/// `[ui.workspace_symbols.color_rule]`: a child workspace gets a random HSV
/// color whose hue is a multiple of `hue_step` not shown by another workspace,
/// with saturation and value drawn from the given percent ranges.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, try_from = "WorkspaceColorRuleRaw")]
pub struct WorkspaceColorRule {
    /// Hue step in degrees; divides 360.
    pub hue_step: u16,
    /// Saturation range in percent.
    pub saturation: [f64; 2],
    /// Value (brightness) range in percent.
    pub value: [f64; 2],
}

impl Default for WorkspaceColorRule {
    fn default() -> Self {
        Self {
            hue_step: 10,
            saturation: [50.0, 70.0],
            value: [70.0, 100.0],
        }
    }
}

#[derive(Deserialize)]
#[serde(default)]
struct WorkspaceColorRuleRaw {
    hue_step: u16,
    saturation: [f64; 2],
    value: [f64; 2],
}

impl Default for WorkspaceColorRuleRaw {
    fn default() -> Self {
        let rule = WorkspaceColorRule::default();
        Self {
            hue_step: rule.hue_step,
            saturation: rule.saturation,
            value: rule.value,
        }
    }
}

impl TryFrom<WorkspaceColorRuleRaw> for WorkspaceColorRule {
    type Error = String;

    fn try_from(raw: WorkspaceColorRuleRaw) -> Result<Self, Self::Error> {
        if raw.hue_step == 0 || 360 % raw.hue_step != 0 {
            return Err(format!(
                "color_rule.hue_step {} must be a positive divisor of 360",
                raw.hue_step
            ));
        }
        for (name, [min, max]) in [("saturation", raw.saturation), ("value", raw.value)] {
            let in_range = |x: f64| (0.0..=100.0).contains(&x);
            if !in_range(min) || !in_range(max) || min > max {
                return Err(format!(
                    "color_rule.{name} [{min}, {max}] must satisfy 0 <= min <= max <= 100"
                ));
            }
        }
        Ok(Self {
            hue_step: raw.hue_step,
            saturation: raw.saturation,
            value: raw.value,
        })
    }
}

/// The removed `colors` field: present only to reject old configurations with
/// a clear message instead of ignoring them.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RemovedColors;

impl<'de> Deserialize<'de> for RemovedColors {
    fn deserialize<D: serde::Deserializer<'de>>(_: D) -> Result<Self, D::Error> {
        Err(serde::de::Error::custom(
            "colors is no longer supported: the color is computed by the rule \
             ui.workspace_symbols.color_rule",
        ))
    }
}

/// One `[[ui.workspace_symbols.dirs]]` entry.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct WorkspaceSymbolDirConfig {
    /// Directory prefix; `~` expands to the home directory.
    pub path: WorkspaceSymbolPath,
    /// Symbol of a root workspace in this directory.
    #[serde(default)]
    pub main: Option<WorkspaceSymbol>,
    /// Symbols handed to child workspaces opened from this directory.
    #[serde(default)]
    pub tasks: Vec<WorkspaceSymbol>,
    /// Rejects the removed `colors` field.
    #[serde(default)]
    #[allow(dead_code)] // never populated: any value is a parse error
    colors: Option<RemovedColors>,
}

/// `[ui.workspace_symbols.outside]`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct WorkspaceSymbolSetConfig {
    /// Symbol of a root workspace outside the listed directories.
    pub main: Option<WorkspaceSymbol>,
    /// Symbols handed to child workspaces.
    pub tasks: Vec<WorkspaceSymbol>,
    /// Rejects the removed `colors` field.
    #[allow(dead_code)] // never populated: any value is a parse error
    colors: Option<RemovedColors>,
}

/// A workspace symbol: one or two cells wide, without U+FE0F.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub struct WorkspaceSymbol(String);

impl WorkspaceSymbol {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for WorkspaceSymbol {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        super::model::validate_status_icon(&value)
            .map_err(|err| format!("workspace symbol: {err}"))?;
        Ok(Self(value))
    }
}

/// A non-empty directory path that is absolute or starts with `~`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub struct WorkspaceSymbolPath(String);

impl WorkspaceSymbolPath {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for WorkspaceSymbolPath {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.trim().is_empty() {
            return Err("workspace symbol path must not be empty".into());
        }
        let home_relative = value == "~" || value.starts_with("~/") || value.starts_with("~\\");
        if !home_relative && !std::path::Path::new(&value).is_absolute() {
            return Err(format!(
                "workspace symbol path {value:?} must be absolute or start with ~"
            ));
        }
        Ok(Self(value))
    }
}

#[cfg(test)]
mod tests {
    use super::WorkspaceColorRule;
    use crate::config::Config;

    fn parse(toml: &str) -> Result<Config, String> {
        toml::from_str::<Config>(toml).map_err(|err| err.to_string())
    }

    #[test]
    fn workspace_symbols_default_to_empty() {
        let config = parse("").unwrap();
        assert!(config.ui.workspace_symbols.dirs.is_empty());
        assert_eq!(config.ui.workspace_symbols.outside, None);
    }

    #[test]
    fn workspace_symbols_parse_dirs_and_outside() {
        let config = parse(
            r##"
[[ui.workspace_symbols.dirs]]
path = "~/work/pets/duty-herdr"
main = "🦁"
tasks = ["🦄", "🦌"]

[[ui.workspace_symbols.dirs]]
path = "/srv/other"
tasks = ["x"]

[ui.workspace_symbols.outside]
main = "🏠"
tasks = ["🐟"]
"##,
        )
        .unwrap();
        let symbols = &config.ui.workspace_symbols;
        assert_eq!(symbols.dirs.len(), 2);
        let first = &symbols.dirs[0];
        assert_eq!(first.path.as_str(), "~/work/pets/duty-herdr");
        assert_eq!(first.main.as_ref().map(|s| s.as_str()), Some("🦁"));
        assert_eq!(
            first.tasks.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
            ["🦄", "🦌"]
        );
        assert_eq!(symbols.dirs[1].main, None);
        let outside = symbols.outside.as_ref().unwrap();
        assert_eq!(outside.main.as_ref().map(|s| s.as_str()), Some("🏠"));
        assert_eq!(outside.tasks.len(), 1);
    }

    #[test]
    fn workspace_symbols_reject_invalid_symbols() {
        for (value, expected) in [
            ("\"\"", "must not be empty"),
            ("\"⚙\u{FE0F}\"", "U+FE0F"),
            ("\"abc\"", "1 or 2 cells"),
        ] {
            let toml = format!("[ui.workspace_symbols.outside]\nmain = {value}\n");
            let err = parse(&toml).unwrap_err();
            assert!(err.contains(expected), "{value}: {err}");
            let toml =
                format!("[[ui.workspace_symbols.dirs]]\npath = \"/a\"\ntasks = [\"x\", {value}]\n");
            let err = parse(&toml).unwrap_err();
            assert!(err.contains(expected), "{value}: {err}");
        }
    }

    #[test]
    fn color_rule_defaults() {
        let rule = parse("").unwrap().ui.workspace_symbols.color_rule;
        assert_eq!(rule, WorkspaceColorRule::default());
        assert_eq!(rule.hue_step, 10);
        assert_eq!(rule.saturation, [50.0, 70.0]);
        assert_eq!(rule.value, [70.0, 100.0]);
        let rule = parse("[ui.workspace_symbols.color_rule]\nhue_step = 30\nvalue = [60, 90.5]\n")
            .unwrap()
            .ui
            .workspace_symbols
            .color_rule;
        assert_eq!(rule.hue_step, 30);
        assert_eq!(rule.saturation, [50.0, 70.0]);
        assert_eq!(rule.value, [60.0, 90.5]);
    }

    #[test]
    fn removed_colors_field_is_rejected_with_a_clear_message() {
        for toml in [
            "[ui.workspace_symbols.outside]\ncolors = [\"#112233\"]\n",
            "[[ui.workspace_symbols.dirs]]\npath = \"/a\"\ncolors = []\n",
        ] {
            let err = parse(toml).unwrap_err();
            assert!(
                err.contains("colors is no longer supported")
                    && err.contains("ui.workspace_symbols.color_rule"),
                "{err}"
            );
        }
    }

    #[test]
    fn color_rule_rejects_invalid_values() {
        for value in [
            "hue_step = 0",
            "hue_step = 7",
            "saturation = [70, 50]",
            "saturation = [-1, 50]",
            "saturation = [50, 101]",
            "value = [0, 120]",
            "value = [80, 70]",
        ] {
            let toml = format!("[ui.workspace_symbols.color_rule]\n{value}\n");
            assert!(parse(&toml).is_err(), "{value}");
        }
    }

    #[test]
    fn workspace_symbols_reject_invalid_paths() {
        for (value, expected) in [
            ("", "must not be empty"),
            ("   ", "must not be empty"),
            ("relative/dir", "absolute or start with ~"),
        ] {
            let toml = format!("[[ui.workspace_symbols.dirs]]\npath = \"{value}\"\n");
            let err = parse(&toml).unwrap_err();
            assert!(err.contains(expected), "{value:?}: {err}");
        }
        let err = parse("[[ui.workspace_symbols.dirs]]\nmain = \"x\"\n").unwrap_err();
        assert!(err.contains("path"), "{err}");
    }

    #[test]
    fn invalid_workspace_symbols_reject_the_live_ui_section() {
        let loaded = super::super::io::load_live_config_from_str(
            "[ui.workspace_symbols.color_rule]\nhue_step = 7\n",
        )
        .unwrap();
        assert!(loaded
            .invalid_sections
            .iter()
            .any(|section| section == "ui"));
        assert!(
            loaded
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.contains("hue_step")),
            "{:?}",
            loaded.diagnostics
        );
    }
}
