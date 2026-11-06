//! Automatic labels of workspaces and tabs from session facts.
//!
//! The rule is shared by the server (outer window title) and the client
//! (spaces, tab bar, agent panel, navigation): a manual label is shown only
//! while an agent is detected in the tab (for a workspace: in its first tab);
//! otherwise the automatic label `<symbol> <place> · <app>` is shown. Without
//! place and app facts the existing label stays.

use std::borrow::Cow;

/// Facts an automatic label is built from.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct LabelFacts<'a> {
    /// Workspace symbol token.
    pub(crate) symbol: Option<&'a str>,
    /// Repository name of the root pane's Git checkout, or its cwd with `~`.
    pub(crate) place: Option<&'a str>,
    /// Detected agent while there is one, otherwise the foreground process of
    /// the focused pane.
    pub(crate) app: Option<&'a str>,
}

impl<'a> LabelFacts<'a> {
    /// Facts with empty values treated as missing.
    pub(crate) fn new(
        symbol: Option<&'a str>,
        place: Option<&'a str>,
        agent: Option<&'a str>,
        foreground_process: Option<&'a str>,
    ) -> Self {
        Self {
            symbol: non_empty(symbol),
            place: non_empty(place),
            app: non_empty(agent).or_else(|| non_empty(foreground_process)),
        }
    }

    /// `<symbol> <place> · <app>`; missing parts are left out with their
    /// separator. `None` when there is neither a place nor an app.
    pub(crate) fn auto_label(self) -> Option<String> {
        if self.place.is_none() && self.app.is_none() {
            return None;
        }
        let head = [self.symbol, self.place]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" ");
        Some(match self.app {
            Some(app) if head.is_empty() => app.to_owned(),
            Some(app) => format!("{head} · {app}"),
            None => head,
        })
    }
}

/// The manual label while an agent is alive, otherwise the automatic label,
/// otherwise `fallback`.
pub(crate) fn display_label<'a>(
    manual: Option<&'a str>,
    agent_alive: bool,
    auto: Option<Cow<'a, str>>,
    fallback: &'a str,
) -> Cow<'a, str> {
    match (manual, auto) {
        (Some(manual), _) if agent_alive => Cow::Borrowed(manual),
        (_, Some(auto)) => auto,
        (_, None) => Cow::Borrowed(fallback),
    }
}

pub(crate) fn non_empty(value: Option<&str>) -> Option<&str> {
    value.filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_label_joins_symbol_place_and_app_and_drops_missing_parts() {
        let facts = |symbol, place, app| LabelFacts { symbol, place, app };
        assert_eq!(
            facts(Some("🦦"), Some("herdr"), Some("nvim"))
                .auto_label()
                .as_deref(),
            Some("🦦 herdr · nvim")
        );
        assert_eq!(
            facts(None, Some("herdr"), Some("nvim"))
                .auto_label()
                .as_deref(),
            Some("herdr · nvim")
        );
        assert_eq!(
            facts(Some("🦦"), Some("~/notes"), None)
                .auto_label()
                .as_deref(),
            Some("🦦 ~/notes")
        );
        assert_eq!(
            facts(None, None, Some("nvim")).auto_label().as_deref(),
            Some("nvim")
        );
        assert_eq!(facts(Some("🦦"), None, None).auto_label(), None);
    }

    #[test]
    fn app_prefers_agent_and_ignores_empty_values() {
        let facts = LabelFacts::new(Some(""), Some("herdr"), Some(""), Some("nvim"));
        assert_eq!(facts.symbol, None);
        assert_eq!(facts.app, Some("nvim"));
        let facts = LabelFacts::new(None, Some("herdr"), Some("opus"), Some("nvim"));
        assert_eq!(facts.app, Some("opus"));
    }

    #[test]
    fn manual_label_shows_only_while_agent_is_alive() {
        assert_eq!(
            display_label(Some("mine"), true, Some("auto".into()), "x"),
            "mine"
        );
        assert_eq!(
            display_label(Some("mine"), false, Some("auto".into()), "x"),
            "auto"
        );
        assert_eq!(display_label(None, true, Some("auto".into()), "x"), "auto");
        assert_eq!(display_label(Some("mine"), false, None, "mine"), "mine");
        assert_eq!(display_label(None, false, None, "2"), "2");
    }
}
