//! Workspace `symbol` and `color` tokens assigned by Herdr from
//! `[ui.workspace_symbols]`.
//!
//! A root workspace shows the `main` symbol of its directory. It is derived
//! when the workspace is projected from the cached Git identity, so a `cd` that
//! the existing identity refresh admits, or a config reload, changes it without
//! extra state. A child workspace receives a free `tasks` symbol and a free
//! `colors` entry of its parent's directory when it gets a parent or moves to
//! another one; the pair is stored on the workspace and saved with the
//! session. A restored child saved without a pair gets one after restore.
//!
//! Workspace metadata tokens reported through the API (for example by the duty
//! plugin with `--source duty`) take precedence over these tokens key by key.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub(crate) const SYMBOL_TOKEN: &str = "symbol";
pub(crate) const COLOR_TOKEN: &str = "color";

/// One resolved symbol set.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SymbolSet {
    pub(crate) main: Option<String>,
    pub(crate) tasks: Vec<String>,
    pub(crate) colors: Vec<String>,
}

impl SymbolSet {
    fn from_parts(
        main: Option<&crate::config::WorkspaceSymbol>,
        tasks: &[crate::config::WorkspaceSymbol],
        colors: &[crate::config::WorkspaceColor],
    ) -> Self {
        Self {
            main: main.map(|symbol| symbol.as_str().to_owned()),
            tasks: tasks.iter().map(|s| s.as_str().to_owned()).collect(),
            colors: colors.iter().map(|c| c.as_str().to_owned()).collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DirRule {
    /// The expanded path and, when it differs, its canonical form.
    prefixes: Vec<PathBuf>,
    set: SymbolSet,
}

/// `[ui.workspace_symbols]` with `~` expanded, ready for prefix matching.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct WorkspaceSymbolRules {
    dirs: Vec<DirRule>,
    outside: Option<SymbolSet>,
}

impl WorkspaceSymbolRules {
    /// Expands and canonicalizes the configured paths once, at config load.
    pub(crate) fn from_config(config: &crate::config::WorkspaceSymbolsConfig) -> Self {
        let dirs = config
            .dirs
            .iter()
            .map(|dir| {
                let expanded = crate::worktree::expand_tilde_path(dir.path.as_str());
                let canonical = crate::worktree::canonical_or_original(&expanded);
                let mut prefixes = vec![expanded];
                if canonical != prefixes[0] {
                    prefixes.push(canonical);
                }
                DirRule {
                    prefixes,
                    set: SymbolSet::from_parts(dir.main.as_ref(), &dir.tasks, &dir.colors),
                }
            })
            .collect();
        let outside = config
            .outside
            .as_ref()
            .map(|set| SymbolSet::from_parts(set.main.as_ref(), &set.tasks, &set.colors));
        Self { dirs, outside }
    }

    /// The set of the longest configured directory containing `dir`, compared
    /// by path components; `outside` when none matches.
    pub(crate) fn set_for(&self, dir: &Path) -> Option<&SymbolSet> {
        self.dirs
            .iter()
            .filter_map(|rule| {
                rule.prefixes
                    .iter()
                    .filter(|prefix| dir.starts_with(prefix))
                    .map(|prefix| prefix.components().count())
                    .max()
                    .map(|depth| (depth, &rule.set))
            })
            // `max_by_key` keeps the last maximum; reverse so the first listed
            // entry wins a tie between equally long paths.
            .rev()
            .max_by_key(|(depth, _)| *depth)
            .map(|(_, set)| set)
            .or(self.outside.as_ref())
    }

    pub(crate) fn main_for(&self, dir: &Path) -> Option<&str> {
        self.set_for(dir)?.main.as_deref()
    }
}

/// Tokens Herdr pinned to a child workspace when it got its parent.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssignedWorkspaceTokens {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

/// Picks the first `tasks` symbol not in `used_symbols` (none when the set is
/// exhausted, as duty's `free_symbol` refuses) and the first color not in
/// `used_colors`; when every color is taken, the least used one, as duty's
/// `free_color` falls back to the whole palette.
pub(crate) fn allocate_child_tokens(
    set: &SymbolSet,
    used_symbols: &HashSet<&str>,
    used_colors: &[&str],
) -> AssignedWorkspaceTokens {
    let symbol = set
        .tasks
        .iter()
        .find(|symbol| !used_symbols.contains(symbol.as_str()))
        .cloned();
    let color = set
        .colors
        .iter()
        .enumerate()
        .min_by_key(|(index, color)| {
            let uses = used_colors
                .iter()
                .filter(|used| **used == color.as_str())
                .count();
            (uses, *index)
        })
        .map(|(_, color)| color.clone());
    AssignedWorkspaceTokens { symbol, color }
}

impl crate::workspace::Workspace {
    /// Directory that selects the symbol set: the Git checkout root of the
    /// first tab's root pane, or its cwd outside Git, as last admitted by the
    /// workspace identity refresh.
    pub(crate) fn symbol_dir(&self) -> &Path {
        self.cached_git_space
            .as_ref()
            .map_or(self.cached_identity_cwd.as_path(), |space| {
                space.repo_root.as_path()
            })
    }

    /// Herdr's own `(symbol, color)` for this workspace.
    pub(crate) fn herdr_tokens<'a>(
        &'a self,
        rules: &'a WorkspaceSymbolRules,
    ) -> (Option<&'a str>, Option<&'a str>) {
        if let Some(assigned) = &self.assigned_tokens {
            return (assigned.symbol.as_deref(), assigned.color.as_deref());
        }
        if self.parent_workspace_id.is_none() {
            return (rules.main_for(self.symbol_dir()), None);
        }
        (None, None)
    }

    /// Workspace tokens as published: reported metadata tokens override
    /// Herdr's own `symbol` and `color` key by key.
    pub(crate) fn effective_tokens(&self, rules: &WorkspaceSymbolRules) -> HashMap<String, String> {
        let mut tokens = self.metadata_tokens.values();
        let (symbol, color) = self.herdr_tokens(rules);
        for (key, value) in [(SYMBOL_TOKEN, symbol), (COLOR_TOKEN, color)] {
            if let Some(value) = value {
                tokens
                    .entry(key.to_owned())
                    .or_insert_with(|| value.to_owned());
            }
        }
        tokens
    }

    /// The published `symbol` token: a reported token, otherwise Herdr's own.
    pub(crate) fn effective_symbol(&self, rules: &WorkspaceSymbolRules) -> Option<String> {
        let (symbol, _) = self.herdr_tokens(rules);
        self.effective_token(SYMBOL_TOKEN, symbol)
    }

    /// One effective token without building the whole map.
    fn effective_token<'a>(&'a self, key: &str, herdr: Option<&'a str>) -> Option<String> {
        self.metadata_tokens
            .value(key)
            .map(str::to_owned)
            .or_else(|| herdr.map(str::to_owned))
    }
}

impl crate::app::AppState {
    /// Pins a free `tasks` symbol and color of the parent's directory to the
    /// child. Symbols and colors count as taken when another open workspace
    /// shows them, whichever source set them.
    pub(crate) fn assign_child_workspace_tokens(&mut self, child: usize, parent: usize) {
        let (Some(child_ws), Some(parent_ws)) =
            (self.workspaces.get(child), self.workspaces.get(parent))
        else {
            return;
        };
        if child_ws.assigned_tokens.is_some() {
            return;
        }
        let rules = &self.workspace_symbols;
        let assigned = match rules.set_for(parent_ws.symbol_dir()) {
            Some(set) => {
                let mut used_symbols = Vec::new();
                let mut used_colors = Vec::new();
                for (index, ws) in self.workspaces.iter().enumerate() {
                    if index == child {
                        continue;
                    }
                    let (symbol, color) = ws.herdr_tokens(rules);
                    used_symbols.extend(ws.effective_token(SYMBOL_TOKEN, symbol));
                    used_colors.extend(ws.effective_token(COLOR_TOKEN, color));
                }
                let used_symbols = used_symbols.iter().map(String::as_str).collect();
                let used_colors = used_colors.iter().map(String::as_str).collect::<Vec<_>>();
                allocate_child_tokens(set, &used_symbols, &used_colors)
            }
            None => AssignedWorkspaceTokens::default(),
        };
        self.workspaces[child].assigned_tokens = Some(assigned);
        self.mark_session_dirty();
    }

    /// Gives a pair to restored children saved before Herdr pinned tokens
    /// (a parent link without `assigned_tokens`). Runs once all workspaces are
    /// restored so every open workspace counts as taken, in workspace number
    /// order; the assignment marks the session for saving.
    pub(crate) fn assign_missing_child_workspace_tokens(&mut self) {
        let mut pending = self
            .workspaces
            .iter()
            .enumerate()
            .filter(|(_, ws)| ws.parent_workspace_id.is_some() && ws.assigned_tokens.is_none())
            .map(|(index, ws)| {
                let number = crate::workspace::public_workspace_number(&ws.id);
                (number.unwrap_or(usize::MAX), index)
            })
            .collect::<Vec<_>>();
        pending.sort_unstable();
        for (_, child) in pending {
            let parent = self.workspaces[child]
                .parent_workspace_id
                .as_deref()
                .and_then(|parent_id| self.workspaces.iter().position(|ws| ws.id == parent_id));
            if let Some(parent) = parent {
                self.assign_child_workspace_tokens(child, parent);
            }
        }
    }

    /// Re-pins the child's tokens after its parent changed: a new parent gives
    /// a free pair of the new parent's directory, with the child's former pair
    /// not counted as taken; a detach makes it a root showing `main` again.
    /// Callers skip this when the parent stays the same.
    pub(crate) fn reparent_workspace_tokens(&mut self, child: usize, parent: Option<usize>) {
        let Some(child_ws) = self.workspaces.get_mut(child) else {
            return;
        };
        child_ws.assigned_tokens = None;
        match parent {
            Some(parent) => self.assign_child_workspace_tokens(child, parent),
            None => self.mark_session_dirty(),
        }
    }

    /// Applies new `[ui.workspace_symbols]` rules. Root workspaces derive
    /// `main` from the rules at projection time; pinned child tokens stay.
    /// Returns whether any published token changed.
    pub(crate) fn set_workspace_symbol_rules(&mut self, rules: WorkspaceSymbolRules) -> bool {
        if self.workspace_symbols == rules {
            return false;
        }
        let changed = self
            .workspaces
            .iter()
            .any(|ws| ws.herdr_tokens(&self.workspace_symbols) != ws.herdr_tokens(&rules));
        self.workspace_symbols = rules;
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::AppState;
    use crate::workspace::{GitSpaceMetadata, Workspace};
    use std::time::Instant;

    fn rules(toml: &str) -> WorkspaceSymbolRules {
        let config = toml::from_str::<crate::config::Config>(toml).unwrap();
        WorkspaceSymbolRules::from_config(&config.ui.workspace_symbols)
    }

    const RULES: &str = r##"
[[ui.workspace_symbols.dirs]]
path = "/work"
main = "W"
tasks = ["a", "b"]
colors = ["#000001", "#000002"]

[[ui.workspace_symbols.dirs]]
path = "/work/pets/duty"
main = "🦁"
tasks = ["🦄", "🦌", "🦊"]
colors = ["#111111", "#222222"]

[ui.workspace_symbols.outside]
main = "🏠"
tasks = ["o"]
"##;

    fn workspace_in(dir: &str) -> Workspace {
        let mut ws = Workspace::test_new("ws");
        ws.custom_name = None;
        ws.cached_identity_cwd = PathBuf::from(dir);
        ws.cached_git_space = None;
        ws
    }

    fn state_with(rules_toml: &str, dirs: &[&str]) -> AppState {
        let mut state = AppState::test_new();
        state.workspace_symbols = rules(rules_toml);
        state.workspaces = dirs.iter().map(|dir| workspace_in(dir)).collect();
        state
    }

    fn report(ws: &mut Workspace, key: &str, value: &str) {
        ws.metadata_tokens.patch(
            HashMap::from([(key.to_owned(), Some(value.to_owned()))]),
            None,
            Instant::now(),
        );
    }

    fn make_child(state: &mut AppState, child: usize, parent: usize) {
        state.workspaces[child].parent_workspace_id = Some(state.workspaces[parent].id.clone());
        state.assign_child_workspace_tokens(child, parent);
    }

    #[test]
    fn longest_component_prefix_wins() {
        let rules = rules(RULES);
        assert_eq!(rules.main_for(Path::new("/work/pets/duty")), Some("🦁"));
        assert_eq!(rules.main_for(Path::new("/work/pets/duty/src")), Some("🦁"));
        assert_eq!(rules.main_for(Path::new("/work/pets")), Some("W"));
        // A string prefix is not a path prefix.
        assert_eq!(
            rules.main_for(Path::new("/work/pets/duty-herdr")),
            Some("W")
        );
        assert_eq!(rules.main_for(Path::new("/workshop")), Some("🏠"));
    }

    #[test]
    fn outside_applies_only_when_configured() {
        let rules = rules("[[ui.workspace_symbols.dirs]]\npath = \"/work\"\nmain = \"W\"\n");
        assert_eq!(rules.main_for(Path::new("/elsewhere")), None);
        assert!(rules.set_for(Path::new("/elsewhere")).is_none());
        assert_eq!(
            WorkspaceSymbolRules::default().main_for(Path::new("/work")),
            None
        );
    }

    #[test]
    fn tilde_paths_expand_to_home() {
        let rules = rules("[[ui.workspace_symbols.dirs]]\npath = \"~/proj\"\nmain = \"P\"\n");
        let home = crate::worktree::expand_tilde_path("~");
        assert_eq!(rules.main_for(&home.join("proj/sub")), Some("P"));
    }

    #[test]
    fn root_workspace_gets_main_of_its_directory_without_color() {
        let rules = rules(RULES);
        let ws = workspace_in("/work/pets/duty/src");
        let tokens = ws.effective_tokens(&rules);
        assert_eq!(tokens.get(SYMBOL_TOKEN).map(String::as_str), Some("🦁"));
        assert_eq!(tokens.get(COLOR_TOKEN), None);
    }

    #[test]
    fn root_directory_is_the_git_checkout_root() {
        let rules = rules(RULES);
        let mut ws = workspace_in("/elsewhere/deep");
        ws.cached_git_space = Some(GitSpaceMetadata {
            key: "/work/pets/duty/.git".into(),
            checkout_key: "/work/pets/duty".into(),
            repo_name: "duty".into(),
            repo_root: PathBuf::from("/work/pets/duty"),
            is_linked_worktree: false,
        });
        assert_eq!(ws.herdr_tokens(&rules).0, Some("🦁"));
    }

    #[test]
    fn root_main_follows_the_admitted_directory() {
        let rules = rules(RULES);
        let mut ws = workspace_in("/work/pets/duty");
        assert_eq!(ws.herdr_tokens(&rules).0, Some("🦁"));
        ws.cached_identity_cwd = PathBuf::from("/tmp");
        assert_eq!(ws.herdr_tokens(&rules).0, Some("🏠"));
    }

    #[test]
    fn workspace_without_matching_rule_has_no_tokens() {
        let ws = workspace_in("/work");
        assert!(ws
            .effective_tokens(&WorkspaceSymbolRules::default())
            .is_empty());
    }

    #[test]
    fn child_gets_free_symbol_and_color_of_parent_directory() {
        let mut state = state_with(RULES, &["/work/pets/duty", "/tmp", "/tmp"]);
        make_child(&mut state, 1, 0);
        make_child(&mut state, 2, 0);
        let first = state.workspaces[1].effective_tokens(&state.workspace_symbols);
        let second = state.workspaces[2].effective_tokens(&state.workspace_symbols);
        assert_eq!(first.get(SYMBOL_TOKEN).map(String::as_str), Some("🦄"));
        assert_eq!(first.get(COLOR_TOKEN).map(String::as_str), Some("#111111"));
        assert_eq!(second.get(SYMBOL_TOKEN).map(String::as_str), Some("🦌"));
        assert_eq!(second.get(COLOR_TOKEN).map(String::as_str), Some("#222222"));
    }

    #[test]
    fn symbols_and_colors_taken_by_duty_tokens_are_skipped() {
        let mut state = state_with(RULES, &["/work/pets/duty", "/tmp", "/tmp"]);
        report(&mut state.workspaces[1], SYMBOL_TOKEN, "🦄");
        report(&mut state.workspaces[1], COLOR_TOKEN, "#111111");
        make_child(&mut state, 2, 0);
        let tokens = state.workspaces[2].effective_tokens(&state.workspace_symbols);
        assert_eq!(tokens.get(SYMBOL_TOKEN).map(String::as_str), Some("🦌"));
        assert_eq!(tokens.get(COLOR_TOKEN).map(String::as_str), Some("#222222"));
    }

    #[test]
    fn exhausted_symbols_give_no_symbol_and_colors_wrap_to_least_used() {
        let mut state = state_with(RULES, &["/work/pets/duty", "/tmp", "/tmp", "/tmp", "/tmp"]);
        for child in 1..=4 {
            make_child(&mut state, child, 0);
        }
        let assigned = |index: usize| state.workspaces[index].assigned_tokens.clone().unwrap();
        assert_eq!(assigned(3).symbol.as_deref(), Some("🦊"));
        assert_eq!(assigned(4).symbol, None);
        assert_eq!(assigned(3).color.as_deref(), Some("#111111"));
        assert_eq!(assigned(4).color.as_deref(), Some("#222222"));
        assert!(!state.workspaces[4]
            .effective_tokens(&state.workspace_symbols)
            .contains_key(SYMBOL_TOKEN));
    }

    #[test]
    fn child_tokens_are_pinned() {
        let mut state = state_with(RULES, &["/work/pets/duty", "/tmp"]);
        make_child(&mut state, 1, 0);
        let pinned = state.workspaces[1].assigned_tokens.clone();
        // A second assignment and losing the parent keep the pinned pair.
        state.assign_child_workspace_tokens(1, 0);
        state.workspaces[1].parent_workspace_id = None;
        state.set_workspace_symbol_rules(rules(""));
        assert_eq!(state.workspaces[1].assigned_tokens, pinned);
        assert_eq!(
            state.workspaces[1].herdr_tokens(&state.workspace_symbols),
            (Some("🦄"), Some("#111111"))
        );
    }

    #[test]
    fn moving_to_another_parent_takes_a_free_pair_of_the_new_parent_directory() {
        let mut state = state_with(RULES, &["/work/pets/duty", "/work", "/tmp", "/tmp"]);
        make_child(&mut state, 2, 0);
        make_child(&mut state, 3, 1);
        assert_eq!(
            state.workspaces[2].herdr_tokens(&state.workspace_symbols),
            (Some("🦄"), Some("#111111"))
        );
        assert_eq!(
            state.workspaces[3].herdr_tokens(&state.workspace_symbols),
            (Some("a"), Some("#000001"))
        );

        // A -> B: the pair comes from B's set; "a" and "#000001" are taken.
        state.workspaces[2].parent_workspace_id = Some(state.workspaces[1].id.clone());
        state.reparent_workspace_tokens(2, Some(1));
        assert_eq!(
            state.workspaces[2].herdr_tokens(&state.workspace_symbols),
            (Some("b"), Some("#000002"))
        );

        // B -> A: the child's own former pair does not count as taken.
        state.workspaces[3].parent_workspace_id = Some(state.workspaces[0].id.clone());
        state.reparent_workspace_tokens(3, Some(0));
        assert_eq!(
            state.workspaces[3].herdr_tokens(&state.workspace_symbols),
            (Some("🦄"), Some("#111111"))
        );

        // Detach: a root again, showing `main` of its own directory.
        state.workspaces[3].parent_workspace_id = None;
        state.reparent_workspace_tokens(3, None);
        assert_eq!(state.workspaces[3].assigned_tokens, None);
        assert_eq!(
            state.workspaces[3].herdr_tokens(&state.workspace_symbols),
            (Some("🏠"), None)
        );
    }

    #[test]
    fn restored_children_without_pinned_tokens_get_pairs_in_number_order() {
        let mut state = state_with(RULES, &["/work/pets/duty", "/tmp", "/tmp", "/tmp", "/tmp"]);
        let id = |number: usize| format!("w{}", crate::workspace::encode_public_number(number));
        for (ws, number) in state.workspaces.iter_mut().zip([1, 5, 3, 4, 2]) {
            ws.id = id(number);
        }
        for child in 1..=3 {
            state.workspaces[child].parent_workspace_id = Some(id(1));
        }
        // w4 already has its pair from the snapshot; w2 is a root.
        state.workspaces[3].assigned_tokens = Some(AssignedWorkspaceTokens {
            symbol: Some("🦄".into()),
            color: Some("#111111".into()),
        });
        state.session_dirty = false;

        state.assign_missing_child_workspace_tokens();

        let pair = |index: usize| state.workspaces[index].herdr_tokens(&state.workspace_symbols);
        // w3 before w5; both skip w4's pair.
        assert_eq!(pair(2), (Some("🦌"), Some("#222222")));
        assert_eq!(pair(1), (Some("🦊"), Some("#111111")));
        assert_eq!(pair(3), (Some("🦄"), Some("#111111")));
        assert_eq!(state.workspaces[4].assigned_tokens, None);
        assert!(state.session_dirty);

        // A second pass changes nothing.
        let before = state
            .workspaces
            .iter()
            .map(|ws| ws.assigned_tokens.clone())
            .collect::<Vec<_>>();
        state.assign_missing_child_workspace_tokens();
        let after = state
            .workspaces
            .iter()
            .map(|ws| ws.assigned_tokens.clone())
            .collect::<Vec<_>>();
        assert_eq!(before, after);
    }

    #[test]
    fn child_without_parent_rule_gets_no_tokens() {
        let mut state = state_with(
            "[[ui.workspace_symbols.dirs]]\npath = \"/work\"\ntasks = [\"a\"]\n",
            &["/elsewhere", "/work"],
        );
        make_child(&mut state, 1, 0);
        assert_eq!(
            state.workspaces[1].assigned_tokens,
            Some(AssignedWorkspaceTokens::default())
        );
        assert!(state.workspaces[1]
            .effective_tokens(&state.workspace_symbols)
            .is_empty());
    }

    #[test]
    fn reported_tokens_override_herdr_tokens() {
        let mut state = state_with(RULES, &["/work/pets/duty", "/tmp"]);
        make_child(&mut state, 1, 0);
        report(&mut state.workspaces[0], SYMBOL_TOKEN, "🥑");
        report(&mut state.workspaces[1], COLOR_TOKEN, "#abcdef");
        let root = state.workspaces[0].effective_tokens(&state.workspace_symbols);
        let child = state.workspaces[1].effective_tokens(&state.workspace_symbols);
        assert_eq!(root.get(SYMBOL_TOKEN).map(String::as_str), Some("🥑"));
        assert_eq!(child.get(SYMBOL_TOKEN).map(String::as_str), Some("🦄"));
        assert_eq!(child.get(COLOR_TOKEN).map(String::as_str), Some("#abcdef"));
    }

    #[test]
    fn rule_reload_reports_changed_root_tokens_only() {
        let mut state = state_with(RULES, &["/work/pets/duty", "/tmp"]);
        make_child(&mut state, 1, 0);
        assert!(!state.set_workspace_symbol_rules(rules(RULES)));
        let replaced = RULES.replace("main = \"🦁\"", "main = \"🐯\"");
        assert!(state.set_workspace_symbol_rules(rules(&replaced)));
        assert_eq!(
            state.workspaces[0].herdr_tokens(&state.workspace_symbols).0,
            Some("🐯")
        );
        assert_eq!(
            state.workspaces[1].herdr_tokens(&state.workspace_symbols).0,
            Some("🦄")
        );
    }

    #[test]
    fn assigned_tokens_serialize_compactly() {
        let empty = serde_json::to_value(AssignedWorkspaceTokens::default()).unwrap();
        assert_eq!(empty, serde_json::json!({}));
        let read: AssignedWorkspaceTokens = serde_json::from_str("{}").unwrap();
        assert_eq!(read, AssignedWorkspaceTokens::default());
    }
}
