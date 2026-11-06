use serde::{Deserialize, Serialize};

/// Following agent dialogs: the client switches its active workspace to a
/// workspace whose agent pane waits in `blocked`, one dialog after another.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct FollowBlockedConfig {
    /// Follow agent panes that enter `blocked`. Default: false.
    pub enabled: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn follow_blocked_defaults_off_and_rejects_unknown_keys() {
        assert!(!FollowBlockedConfig::default().enabled);
        let parsed: FollowBlockedConfig = toml::from_str("enabled = true").expect("parses");
        assert!(parsed.enabled);
        assert!(toml::from_str::<FollowBlockedConfig>("enable = true").is_err());
    }
}
