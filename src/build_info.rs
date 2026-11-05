//! Build identity helpers.

pub const BASE_VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn channel() -> &'static str {
    non_empty(option_env!("HERDR_BUILD_CHANNEL")).unwrap_or("stable")
}

pub fn build_id() -> Option<&'static str> {
    non_empty(option_env!("HERDR_BUILD_ID"))
}

pub fn commit() -> Option<&'static str> {
    non_empty(option_env!("HERDR_BUILD_COMMIT"))
}

/// duty-herdr fork: `herdr --version` line, e.g. `herdr 0.9.5 (duty-v0.9.5.1 1a2b3c4)`
/// for a release build and `herdr 0.9.5 (dev)` for a build without `HERDR_BUILD_ID`.
pub fn version_line() -> String {
    format_version_line(&version(), build_id(), commit())
}

fn format_version_line(version: &str, build_id: Option<&str>, commit: Option<&str>) -> String {
    let Some(build_id) = build_id else {
        return format!("herdr {version} (dev)");
    };
    match commit {
        Some(commit) => {
            let short: String = commit.chars().take(7).collect();
            format!("herdr {version} ({build_id} {short})")
        }
        None => format!("herdr {version} ({build_id})"),
    }
}

/// duty-herdr fork: identity of the running build for update notices and stored
/// release notes. A stable build with `HERDR_BUILD_ID` reads `0.9.5 (duty-v0.9.5.1)`,
/// so a rebuild of the same Cargo version is told apart from the installed one.
pub fn release_identity() -> String {
    match channel() {
        "stable" => format_release_identity(BASE_VERSION, build_id()),
        _ => version(),
    }
}

/// `0.9.5` without a build id, `0.9.5 (duty-v0.9.5.1)` with one.
pub fn format_release_identity(version: &str, build_id: Option<&str>) -> String {
    match build_id {
        Some(build_id) => format!("{version} ({build_id})"),
        None => version.to_string(),
    }
}

/// Splits an identity from [`format_release_identity`] into the version and the build id.
pub fn split_release_identity(identity: &str) -> (&str, Option<&str>) {
    identity
        .strip_suffix(')')
        .and_then(|rest| rest.split_once(" ("))
        .map_or((identity, None), |(version, build_id)| {
            (version, Some(build_id))
        })
}

pub fn version() -> String {
    match channel() {
        "stable" => BASE_VERSION.to_string(),
        channel => match build_id() {
            Some(build_id) => format!("{BASE_VERSION}-{channel}.{build_id}"),
            None => format!("{BASE_VERSION}-{channel}"),
        },
    }
}

pub fn is_preview() -> bool {
    channel() == "preview"
}

fn non_empty(value: Option<&'static str>) -> Option<&'static str> {
    value.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn stable_version_defaults_to_cargo_version() {
        assert!(!super::version().is_empty());
    }

    #[test]
    fn release_identity_round_trips_build_id() {
        let identity = super::format_release_identity("0.9.5", Some("duty-v0.9.5.1"));
        assert_eq!(identity, "0.9.5 (duty-v0.9.5.1)");
        assert_eq!(
            super::split_release_identity(&identity),
            ("0.9.5", Some("duty-v0.9.5.1"))
        );
        assert_eq!(super::format_release_identity("0.9.5", None), "0.9.5");
        assert_eq!(super::split_release_identity("0.9.5"), ("0.9.5", None));
        assert_eq!(
            super::split_release_identity("0.9.5-preview.abc"),
            ("0.9.5-preview.abc", None)
        );
    }

    #[test]
    fn version_line_names_fork_build_and_short_commit() {
        assert_eq!(
            super::format_version_line(
                "0.9.5",
                Some("duty-v0.9.5.1"),
                Some("1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f9a0b")
            ),
            "herdr 0.9.5 (duty-v0.9.5.1 1a2b3c4)"
        );
        assert_eq!(
            super::format_version_line("0.9.5", Some("duty-v0.9.5"), None),
            "herdr 0.9.5 (duty-v0.9.5)"
        );
        assert_eq!(
            super::format_version_line("0.9.5", None, Some("1a2b3c4")),
            "herdr 0.9.5 (dev)"
        );
        assert!(super::version_line().starts_with(&format!("herdr {} (", super::version())));
    }
}
