//! Workspace `symbol` and `color` tokens assigned by Herdr from
//! `[ui.workspace_symbols]`.
//!
//! A root workspace shows the `main` symbol of its directory. It is derived
//! when the workspace is projected from the cached Git identity, so a `cd` that
//! the existing identity refresh admits, or a config reload, changes it without
//! extra state. A child workspace receives a free `tasks` symbol of its
//! parent's directory and a random color by `color_rule` when it gets a parent
//! or moves to another one; the pair is stored on the workspace and saved with
//! the session. A restored child saved without a pair gets one after restore.
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
}

impl SymbolSet {
    fn from_parts(
        main: Option<&crate::config::WorkspaceSymbol>,
        tasks: &[crate::config::WorkspaceSymbol],
    ) -> Self {
        Self {
            main: main.map(|symbol| symbol.as_str().to_owned()),
            tasks: tasks.iter().map(|s| s.as_str().to_owned()).collect(),
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
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct WorkspaceSymbolRules {
    dirs: Vec<DirRule>,
    outside: Option<SymbolSet>,
    color_rule: crate::config::WorkspaceColorRule,
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
                    set: SymbolSet::from_parts(dir.main.as_ref(), &dir.tasks),
                }
            })
            .collect();
        let outside = config
            .outside
            .as_ref()
            .map(|set| SymbolSet::from_parts(set.main.as_ref(), &set.tasks));
        Self {
            dirs,
            outside,
            color_rule: config.color_rule.clone(),
        }
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

/// Source of randomness for child colors: splitmix64, seeded from the
/// per-process random keys of `RandomState` (no extra dependency); tests seed
/// it directly.
#[derive(Debug, Clone)]
pub(crate) struct ColorRng(u64);

impl ColorRng {
    pub(crate) fn from_entropy() -> Self {
        use std::hash::{BuildHasher, Hasher};
        Self(
            std::collections::hash_map::RandomState::new()
                .build_hasher()
                .finish(),
        )
    }

    #[cfg(test)]
    pub(crate) fn seeded(seed: u64) -> Self {
        Self(seed)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Uniform in `[0, 1)`.
    fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Uniform in `[0, n)`; `n` must be non-zero.
    fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }
}

/// HSV (`h` degrees, `s` and `v` in `0..=1`) to `#rrggbb`.
pub(crate) fn hsv_to_hex(h: f64, s: f64, v: f64) -> String {
    let c = v * s;
    let min = v - c;
    let hp = (h.rem_euclid(360.0)) / 60.0;
    let mid = min + c * (1.0 - (hp % 2.0 - 1.0).abs());
    let (r, g, b) = match hp as u32 {
        0 => (v, mid, min),
        1 => (mid, v, min),
        2 => (min, v, mid),
        3 => (min, mid, v),
        4 => (mid, min, v),
        _ => (v, min, mid),
    };
    let byte = |x: f64| (x * 255.0).round_ties_even().clamp(0.0, 255.0) as u8;
    format!("#{:02x}{:02x}{:02x}", byte(r), byte(g), byte(b))
}

/// Hue in degrees of a `#rrggbb` color; none for other strings and grays.
fn hex_hue(color: &str) -> Option<f64> {
    let hex = color.strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    let channel = |i: usize| {
        u8::from_str_radix(hex.get(i..i + 2)?, 16)
            .ok()
            .map(|x| f64::from(x) / 255.0)
    };
    let (r, g, b) = (channel(0)?, channel(2)?, channel(4)?);
    let max = r.max(g).max(b);
    let delta = max - r.min(g).min(b);
    if delta <= 0.0 {
        return None;
    }
    let hue = if max == r {
        ((g - b) / delta).rem_euclid(6.0)
    } else if max == g {
        (b - r) / delta + 2.0
    } else {
        (r - g) / delta + 4.0
    };
    Some(hue * 60.0)
}

/// A random color by `rule`: the hue is a multiple of `hue_step` not within
/// half a step of any `used_colors` hue (any hue when all are taken);
/// saturation and value are drawn uniformly from their percent ranges.
fn random_color(
    rule: &crate::config::WorkspaceColorRule,
    used_colors: &[&str],
    rng: &mut ColorRng,
) -> String {
    let step = f64::from(rule.hue_step);
    let buckets = usize::from(360 / rule.hue_step);
    let taken: HashSet<usize> = used_colors
        .iter()
        .filter_map(|color| hex_hue(color))
        .map(|hue| (hue / step).round() as usize % buckets)
        .collect();
    let free: Vec<usize> = (0..buckets).filter(|b| !taken.contains(b)).collect();
    let bucket = if free.is_empty() {
        rng.below(buckets)
    } else {
        free[rng.below(free.len())]
    };
    let lerp = |[min, max]: [f64; 2], t: f64| min + (max - min) * t;
    let saturation = lerp(rule.saturation, rng.unit());
    let value = lerp(rule.value, rng.unit());
    hsv_to_hex(bucket as f64 * step, saturation / 100.0, value / 100.0)
}

/// Picks the first `tasks` symbol not in `used_symbols` (none when the set is
/// exhausted, as duty's `free_symbol` refuses) and a random color by the
/// `color_rule` whose hue no other open workspace uses.
pub(crate) fn allocate_child_tokens(
    set: &SymbolSet,
    rule: &crate::config::WorkspaceColorRule,
    used_symbols: &HashSet<&str>,
    used_colors: &[&str],
    rng: &mut ColorRng,
) -> AssignedWorkspaceTokens {
    let symbol = set
        .tasks
        .iter()
        .find(|symbol| !used_symbols.contains(symbol.as_str()))
        .cloned();
    let color = Some(random_color(rule, used_colors, rng));
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
        self.assign_child_workspace_tokens_with(child, parent, &mut ColorRng::from_entropy());
    }

    fn assign_child_workspace_tokens_with(
        &mut self,
        child: usize,
        parent: usize,
        rng: &mut ColorRng,
    ) {
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
                allocate_child_tokens(set, &rules.color_rule, &used_symbols, &used_colors, rng)
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

[[ui.workspace_symbols.dirs]]
path = "/work/pets/duty"
main = "🦁"
tasks = ["🦄", "🦌", "🦊"]

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
        make_child_seeded(state, child, parent, child as u64);
    }

    fn make_child_seeded(state: &mut AppState, child: usize, parent: usize, seed: u64) {
        state.workspaces[child].parent_workspace_id = Some(state.workspaces[parent].id.clone());
        state.assign_child_workspace_tokens_with(child, parent, &mut ColorRng::seeded(seed));
    }

    fn color_of(state: &AppState, index: usize) -> String {
        state.workspaces[index]
            .herdr_tokens(&state.workspace_symbols)
            .1
            .expect("child has a color")
            .to_owned()
    }

    /// Hue bucket (step 10) of a `#rrggbb` color.
    fn bucket(color: &str) -> usize {
        (hex_hue(color).expect("colored") / 10.0).round() as usize % 36
    }

    fn hsv_of(color: &str) -> (f64, f64, f64) {
        let channel = |i: usize| f64::from(u8::from_str_radix(&color[i..i + 2], 16).unwrap());
        let (r, g, b) = (channel(1), channel(3), channel(5));
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        (
            hex_hue(color).unwrap(),
            (max - min) / max * 100.0,
            max / 255.0 * 100.0,
        )
    }

    fn assert_in_ranges(color: &str) {
        let (hue, saturation, value) = hsv_of(color);
        // Bytes are rounded, so allow a point of slack on the percentages.
        assert!(
            (hue / 10.0 - (hue / 10.0).round()).abs() < 0.1,
            "{color}: hue {hue}"
        );
        assert!(
            (49.0..=71.0).contains(&saturation),
            "{color}: s {saturation}"
        );
        assert!((69.0..=100.5).contains(&value), "{color}: v {value}");
    }

    #[test]
    fn hsv_to_hex_matches_the_reference_palette() {
        assert_eq!(hsv_to_hex(0.0, 0.7, 1.0), "#ff4d4d");
        assert_eq!(hsv_to_hex(120.0, 0.5, 0.7), "#59b259");
        assert_eq!(hsv_to_hex(240.0, 0.7, 0.7), "#3636b2");
        assert_eq!(hsv_to_hex(0.0, 0.5, 0.7), "#b25959");
        assert_eq!(hsv_to_hex(60.0, 0.0, 1.0), "#ffffff");
        assert_eq!(hsv_to_hex(350.0, 1.0, 1.0), "#ff002b");
    }

    #[test]
    fn hex_hue_reads_back_the_hue_and_ignores_grays_and_garbage() {
        assert_eq!(hex_hue("#ff0000"), Some(0.0));
        assert_eq!(hex_hue("#00ff00"), Some(120.0));
        assert_eq!(hex_hue("#0000ff"), Some(240.0));
        assert_eq!(hex_hue("#111111"), None);
        assert_eq!(hex_hue("red"), None);
        assert_eq!(hex_hue("#12345"), None);
    }

    #[test]
    fn random_colors_stay_in_ranges_and_are_seeded() {
        let rule = crate::config::WorkspaceColorRule::default();
        let mut rng = ColorRng::seeded(7);
        for _ in 0..500 {
            assert_in_ranges(&random_color(&rule, &[], &mut rng));
        }
        assert_eq!(
            random_color(&rule, &[], &mut ColorRng::seeded(3)),
            random_color(&rule, &[], &mut ColorRng::seeded(3))
        );
    }

    #[test]
    fn taken_hues_are_not_repeated_until_all_are_taken() {
        let rule = crate::config::WorkspaceColorRule::default();
        let all: Vec<String> = (0..36)
            .map(|i| hsv_to_hex(f64::from(i) * 10.0, 0.6, 0.9))
            .collect();
        for seed in 0..200 {
            let free = (seed % 36) as usize;
            let used: Vec<&str> = all
                .iter()
                .enumerate()
                .filter(|(i, _)| *i != free)
                .map(|(_, c)| c.as_str())
                .collect();
            let color = random_color(&rule, &used, &mut ColorRng::seeded(seed));
            assert_eq!(bucket(&color), free, "seed {seed}: {color}");
        }
        // Every hue taken: any of the 36 hues, still in ranges.
        let used: Vec<&str> = all.iter().map(String::as_str).collect();
        let mut seen = HashSet::new();
        let mut rng = ColorRng::seeded(1);
        for _ in 0..2000 {
            let color = random_color(&rule, &used, &mut rng);
            assert_in_ranges(&color);
            seen.insert(bucket(&color));
        }
        assert_eq!(seen.len(), 36);
    }

    #[test]
    fn custom_color_rule_changes_step_and_ranges() {
        let rule = crate::config::WorkspaceColorRule {
            hue_step: 90,
            saturation: [100.0, 100.0],
            value: [100.0, 100.0],
        };
        let mut rng = ColorRng::seeded(5);
        let colors: HashSet<String> = (0..100)
            .map(|_| random_color(&rule, &[], &mut rng))
            .collect();
        let expected = ["#ff0000", "#80ff00", "#00ffff", "#8000ff"];
        assert!(
            colors.iter().all(|c| expected.contains(&c.as_str())),
            "{colors:?}"
        );
        assert_eq!(colors.len(), 4);
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
    fn child_gets_free_symbol_of_parent_directory_and_a_rule_color() {
        let mut state = state_with(RULES, &["/work/pets/duty", "/tmp", "/tmp"]);
        make_child(&mut state, 1, 0);
        make_child(&mut state, 2, 0);
        let first = state.workspaces[1].effective_tokens(&state.workspace_symbols);
        let second = state.workspaces[2].effective_tokens(&state.workspace_symbols);
        assert_eq!(first.get(SYMBOL_TOKEN).map(String::as_str), Some("🦄"));
        assert_eq!(second.get(SYMBOL_TOKEN).map(String::as_str), Some("🦌"));
        let (a, b) = (
            first.get(COLOR_TOKEN).unwrap(),
            second.get(COLOR_TOKEN).unwrap(),
        );
        assert_in_ranges(a);
        assert_in_ranges(b);
        assert_ne!(bucket(a), bucket(b));
    }

    #[test]
    fn symbols_and_hues_taken_by_duty_tokens_are_skipped() {
        for seed in 0..50 {
            let mut state = state_with(RULES, &["/work/pets/duty", "/tmp", "/tmp"]);
            report(&mut state.workspaces[1], SYMBOL_TOKEN, "🦄");
            // Hue 0 reported by duty; 5 degrees off still rounds to bucket 0 or 1.
            report(&mut state.workspaces[1], COLOR_TOKEN, "#ff4d4d");
            make_child_seeded(&mut state, 2, 0, seed);
            let tokens = state.workspaces[2].effective_tokens(&state.workspace_symbols);
            assert_eq!(tokens.get(SYMBOL_TOKEN).map(String::as_str), Some("🦌"));
            assert_ne!(bucket(tokens.get(COLOR_TOKEN).unwrap()), 0, "seed {seed}");
        }
    }

    #[test]
    fn exhausted_symbols_give_no_symbol_but_still_a_color() {
        let mut state = state_with(RULES, &["/work/pets/duty", "/tmp", "/tmp", "/tmp", "/tmp"]);
        for child in 1..=4 {
            make_child(&mut state, child, 0);
        }
        let assigned = |index: usize| state.workspaces[index].assigned_tokens.clone().unwrap();
        assert_eq!(assigned(3).symbol.as_deref(), Some("🦊"));
        assert_eq!(assigned(4).symbol, None);
        assert_in_ranges(assigned(3).color.as_deref().unwrap());
        assert_in_ranges(assigned(4).color.as_deref().unwrap());
        let hues = (1..=4)
            .map(|i| bucket(assigned(i).color.as_deref().unwrap()))
            .collect::<HashSet<_>>();
        assert_eq!(hues.len(), 4);
        let tokens = state.workspaces[4].effective_tokens(&state.workspace_symbols);
        assert!(!tokens.contains_key(SYMBOL_TOKEN));
        assert!(tokens.contains_key(COLOR_TOKEN));
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
        let (symbol, color) = state.workspaces[1].herdr_tokens(&state.workspace_symbols);
        assert_eq!(symbol, Some("🦄"));
        assert_eq!(color, pinned.as_ref().and_then(|p| p.color.as_deref()));
    }

    #[test]
    fn moving_to_another_parent_takes_a_free_pair_of_the_new_parent_directory() {
        let mut state = state_with(RULES, &["/work/pets/duty", "/work", "/tmp", "/tmp"]);
        make_child(&mut state, 2, 0);
        make_child(&mut state, 3, 1);
        let symbol = |state: &AppState, i: usize| {
            state.workspaces[i]
                .herdr_tokens(&state.workspace_symbols)
                .0
                .map(str::to_owned)
        };
        assert_eq!(symbol(&state, 2).as_deref(), Some("🦄"));
        assert_eq!(symbol(&state, 3).as_deref(), Some("a"));
        let color3 = color_of(&state, 3);

        // A -> B: the symbol comes from B's set ("a" is taken); the hue of
        // the other child is not repeated.
        state.workspaces[2].parent_workspace_id = Some(state.workspaces[1].id.clone());
        state.reparent_workspace_tokens(2, Some(1));
        assert_eq!(symbol(&state, 2).as_deref(), Some("b"));
        assert_ne!(bucket(&color_of(&state, 2)), bucket(&color3));

        // B -> A: the child's own former symbol does not count as taken.
        state.workspaces[3].parent_workspace_id = Some(state.workspaces[0].id.clone());
        state.reparent_workspace_tokens(3, Some(0));
        assert_eq!(symbol(&state, 3).as_deref(), Some("🦄"));
        assert_in_ranges(&color_of(&state, 3));

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
            color: Some("#ff4d4d".into()),
        });
        state.session_dirty = false;

        state.assign_missing_child_workspace_tokens();

        let pair = |index: usize| state.workspaces[index].herdr_tokens(&state.workspace_symbols);
        // w3 before w5; both skip w4's pair.
        assert_eq!(pair(2).0, Some("🦌"));
        assert_eq!(pair(1).0, Some("🦊"));
        // The color saved in the snapshot is not changed by restore.
        assert_eq!(pair(3), (Some("🦄"), Some("#ff4d4d")));
        let (c2, c1) = (color_of(&state, 2), color_of(&state, 1));
        assert_ne!(bucket(&c2), 0);
        assert_ne!(bucket(&c1), 0);
        assert_ne!(bucket(&c1), bucket(&c2));
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

    #[test]
    fn rule_reload_keeps_reported_tokens_and_recomputes_herdr_ones() {
        let mut state = state_with(RULES, &["/work/pets/duty"]);
        report(&mut state.workspaces[0], "color", "#ffffff");
        assert!(state.set_workspace_symbol_rules(rules(
            r#"
[ui.workspace_symbols.outside]
main = "🏠"
tasks = ["o"]
"#
        )));
        let tokens = state.workspaces[0].effective_tokens(&state.workspace_symbols);
        assert_eq!(tokens.get("color").map(String::as_str), Some("#ffffff"));
        assert_eq!(tokens.get("symbol").map(String::as_str), Some("🏠"));
    }
}
