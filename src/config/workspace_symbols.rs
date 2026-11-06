//! `[ui.workspace_symbols]`: the symbols and colors Herdr assigns to workspaces.
//!
//! ```toml
//! [[ui.workspace_symbols.dirs]]
//! path = "~/work/pets/duty-herdr"
//! main = "🦁"
//! tasks = ["🦄", "🦌", "🦊"]
//! colors = ["#f38ba8", "#a6e3a1"]
//!
//! [ui.workspace_symbols.outside]
//! main = "🏠"
//! tasks = ["🐟", "🐠"]
//! ```
//!
//! Values are validated while parsing, so an invalid entry rejects the `[ui]`
//! section the same way an invalid `[ui.status_icons]` entry does.

use serde::Deserialize;

/// Symbol sets keyed by directory plus the set for workspaces outside them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct WorkspaceSymbolsConfig {
    /// Per-directory sets; the longest matching `path` wins.
    pub dirs: Vec<WorkspaceSymbolDirConfig>,
    /// Set for workspaces whose directory matches no `dirs` entry.
    pub outside: Option<WorkspaceSymbolSetConfig>,
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
    /// `#rrggbb` colors handed to child workspaces opened from this directory.
    #[serde(default)]
    pub colors: Vec<WorkspaceColor>,
}

/// `[ui.workspace_symbols.outside]`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct WorkspaceSymbolSetConfig {
    /// Symbol of a root workspace outside the listed directories.
    pub main: Option<WorkspaceSymbol>,
    /// Symbols handed to child workspaces.
    pub tasks: Vec<WorkspaceSymbol>,
    /// `#rrggbb` colors handed to child workspaces.
    pub colors: Vec<WorkspaceColor>,
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

/// A `#rrggbb` color.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub struct WorkspaceColor(String);

impl WorkspaceColor {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for WorkspaceColor {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let valid = value
            .strip_prefix('#')
            .is_some_and(|hex| hex.len() == 6 && hex.chars().all(|ch| ch.is_ascii_hexdigit()));
        if !valid {
            return Err(format!(
                "workspace color {value:?} must be a #rrggbb hex color"
            ));
        }
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
colors = ["#f38ba8", "#A6E3A1"]

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
        assert_eq!(
            first.colors.iter().map(|c| c.as_str()).collect::<Vec<_>>(),
            ["#f38ba8", "#A6E3A1"]
        );
        assert_eq!(symbols.dirs[1].main, None);
        assert!(symbols.dirs[1].colors.is_empty());
        let outside = symbols.outside.as_ref().unwrap();
        assert_eq!(outside.main.as_ref().map(|s| s.as_str()), Some("🏠"));
        assert_eq!(outside.tasks.len(), 1);
        assert!(outside.colors.is_empty());
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
    fn workspace_symbols_reject_invalid_colors() {
        for value in ["red", "#fff", "#12345g", "123456", "#1234567"] {
            let toml = format!("[ui.workspace_symbols.outside]\ncolors = [\"{value}\"]\n");
            let err = parse(&toml).unwrap_err();
            assert!(err.contains("#rrggbb"), "{value}: {err}");
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
            "[ui.workspace_symbols.outside]\ncolors = [\"red\"]\n",
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
                .any(|diagnostic| diagnostic.contains("#rrggbb")),
            "{:?}",
            loaded.diagnostics
        );
    }
}
