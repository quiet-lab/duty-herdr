use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const PENDING_RELEASE_NOTES_PATH: &str = "release-notes.json";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseNotes {
    pub version: String,
    pub body: String,
    pub preview: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct StoredReleaseNotes {
    version: String,
    body: String,
    #[serde(default = "default_show_on_startup")]
    show_on_startup: bool,
}

fn default_show_on_startup() -> bool {
    true
}

pub fn pending_path() -> PathBuf {
    let mut path = crate::config::config_path();
    path.set_file_name(PENDING_RELEASE_NOTES_PATH);
    path
}

pub fn save_pending(version: &str, body: &str) -> std::io::Result<()> {
    save_pending_to_path(&pending_path(), version, body)
}

fn save_pending_to_path(path: &Path, version: &str, body: &str) -> std::io::Result<()> {
    let body = normalize_body(body);
    if body.is_empty() {
        return clear_pending_at(path);
    }

    write_stored_to_path(
        path,
        &StoredReleaseNotes {
            version: version.to_string(),
            body,
            show_on_startup: true,
        },
    )
}

fn write_stored_to_path(path: &Path, stored: &StoredReleaseNotes) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let json = serde_json::to_string_pretty(stored).map_err(std::io::Error::other)?;
    let tmp_path = path.with_extension(format!("json.tmp.{}", std::process::id()));
    fs::write(&tmp_path, json)?;
    if let Err(err) = fs::rename(&tmp_path, path) {
        let _ = fs::remove_file(&tmp_path);
        return Err(err);
    }
    Ok(())
}

fn load_stored_from_path(path: &Path) -> Option<StoredReleaseNotes> {
    let content = fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

pub fn load_latest() -> Option<ReleaseNotes> {
    load_latest_from_path(&pending_path(), &crate::build_info::release_identity())
}

fn load_latest_from_path(path: &Path, current_version: &str) -> Option<ReleaseNotes> {
    let stored = load_stored_from_path(path)?;
    release_notes_from_stored(stored, current_version)
}

fn release_notes_from_stored(
    stored: StoredReleaseNotes,
    current_version: &str,
) -> Option<ReleaseNotes> {
    let body = normalize_body(&stored.body);
    if body.is_empty() {
        return None;
    }

    // Versions are release identities (`build_info::release_identity`): notes for a
    // rebuild of the running Cargo version with another build id are still pending.
    let (stored_base, stored_build) = crate::build_info::split_release_identity(&stored.version);
    let (current_base, current_build) = crate::build_info::split_release_identity(current_version);
    let preview = match (
        crate::update::Version::parse(stored_base),
        crate::update::Version::parse(current_base),
    ) {
        (Some(stored_version), Some(current_version)) => {
            stored_version > current_version
                || (stored_version == current_version
                    && stored_build.is_some()
                    && stored_build != current_build)
        }
        _ => false,
    };

    Some(ReleaseNotes {
        preview,
        version: stored.version,
        body,
    })
}

pub fn mark_current_version_seen() -> std::io::Result<()> {
    mark_current_version_seen_at(&pending_path(), &crate::build_info::release_identity())
}

fn mark_current_version_seen_at(path: &Path, current_version: &str) -> std::io::Result<()> {
    let Some(mut stored) = load_stored_from_path(path) else {
        return Ok(());
    };
    if !describes_current_build(&stored.version, current_version) || !stored.show_on_startup {
        return Ok(());
    }
    stored.show_on_startup = false;
    write_stored_to_path(path, &stored)
}

/// Notes saved without a build id belong to every build of that version; notes saved
/// with one belong only to that build.
fn describes_current_build(stored_version: &str, current_version: &str) -> bool {
    let (stored_base, stored_build) = crate::build_info::split_release_identity(stored_version);
    let (current_base, current_build) = crate::build_info::split_release_identity(current_version);
    stored_base == current_base && (stored_build.is_none() || stored_build == current_build)
}

fn clear_pending_at(path: &Path) -> std::io::Result<()> {
    if path.exists() {
        fs::remove_file(path)
    } else {
        Ok(())
    }
}

pub fn load_preview_from_local_changelog(version: &str) -> Option<ReleaseNotes> {
    let path = Path::new("CHANGELOG.md");
    let content = fs::read_to_string(path).ok()?;
    let body = extract_version_section(&content, version)?;
    Some(ReleaseNotes {
        version: version.to_string(),
        body: normalize_body(&body),
        preview: true,
    })
}

fn extract_version_section(content: &str, version: &str) -> Option<String> {
    let header = format!("## [{version}]");
    let mut collecting = false;
    let mut lines = Vec::new();

    for line in content.lines() {
        if !collecting {
            if line.starts_with(&header) {
                collecting = true;
            }
            continue;
        }

        if line.starts_with("## [") {
            break;
        }

        lines.push(line);
    }

    let body = lines.join("\n").trim().to_string();
    (!body.is_empty()).then_some(body)
}

pub fn normalize_body(body: &str) -> String {
    body.lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_version_section() {
        let changelog = "# Changelog\n\n## [0.2.3] - 2026-03-31\n\n### Changed\n- One\n\n## [0.2.2] - 2026-03-30\n\n### Fixed\n- Two\n";
        assert_eq!(
            extract_version_section(changelog, "0.2.3").as_deref(),
            Some("### Changed\n- One")
        );
    }

    #[test]
    fn preserves_headings() {
        assert_eq!(
            normalize_body("### Changed\n- One\n\n### Fixed\n- Two"),
            "### Changed\n- One\n\n### Fixed\n- Two"
        );
    }

    #[test]
    fn load_latest_keeps_future_version_previewable_before_restart() {
        let path = std::env::temp_dir().join(format!(
            "herdr-release-notes-{}-{}.json",
            std::process::id(),
            "preview"
        ));
        let _ = clear_pending_at(&path);
        save_pending_to_path(&path, "0.3.2", "### Changed\n- One").unwrap();

        let notes = load_latest_from_path(&path, "0.3.1").expect("latest notes");
        assert_eq!(notes.version, "0.3.2");
        assert_eq!(notes.body, "### Changed\n- One");
        assert!(notes.preview);

        clear_pending_at(&path).unwrap();
    }

    #[test]
    fn load_latest_does_not_mark_older_saved_version_as_preview() {
        let path = std::env::temp_dir().join(format!(
            "herdr-release-notes-{}-{}.json",
            std::process::id(),
            "stale"
        ));
        let _ = clear_pending_at(&path);
        save_pending_to_path(&path, "0.3.0", "### Changed\n- One").unwrap();

        let notes = load_latest_from_path(&path, "0.3.1").expect("latest notes");
        assert_eq!(notes.version, "0.3.0");
        assert_eq!(notes.body, "### Changed\n- One");
        assert!(!notes.preview);

        clear_pending_at(&path).unwrap();
    }

    #[test]
    fn marking_current_version_seen_preserves_latest_notes() {
        let path = std::env::temp_dir().join(format!(
            "herdr-release-notes-{}-{}.json",
            std::process::id(),
            "seen"
        ));
        let _ = clear_pending_at(&path);
        save_pending_to_path(&path, "0.3.1", "### Changed\n- One").unwrap();

        mark_current_version_seen_at(&path, "0.3.1").unwrap();

        let stored = load_stored_from_path(&path).expect("stored notes");
        assert!(!stored.show_on_startup);
        let latest = load_latest_from_path(&path, "0.3.1").expect("latest notes");
        assert_eq!(latest.version, "0.3.1");
        assert!(!latest.preview);

        clear_pending_at(&path).unwrap();
    }

    #[test]
    fn legacy_notes_without_show_on_startup_remain_available_as_latest() {
        let path = std::env::temp_dir().join(format!(
            "herdr-release-notes-{}-{}.json",
            std::process::id(),
            "legacy"
        ));
        let _ = clear_pending_at(&path);
        fs::write(
            &path,
            "{\n  \"version\": \"0.3.1\",\n  \"body\": \"### Changed\\n- One\"\n}",
        )
        .unwrap();

        assert!(load_latest_from_path(&path, "0.3.1").is_some());

        clear_pending_at(&path).unwrap();
    }

    #[test]
    fn same_version_rebuild_notes_stay_pending_until_installed() {
        let path = std::env::temp_dir().join(format!(
            "herdr-release-notes-{}-{}.json",
            std::process::id(),
            "rebuild"
        ));
        let _ = clear_pending_at(&path);
        save_pending_to_path(&path, "0.9.5 (duty-v0.9.5.1)", "### Changed\n- One").unwrap();

        // Running the previous build of the same Cargo version: the update survives a restart.
        let notes = load_latest_from_path(&path, "0.9.5 (duty-v0.9.5)").expect("latest notes");
        assert_eq!(notes.version, "0.9.5 (duty-v0.9.5.1)");
        assert!(notes.preview);
        assert!(
            load_latest_from_path(&path, "0.9.5")
                .expect("dev build")
                .preview
        );

        // Dismissing notes before the install does not mark them seen.
        mark_current_version_seen_at(&path, "0.9.5 (duty-v0.9.5)").unwrap();
        assert!(
            load_stored_from_path(&path)
                .expect("stored")
                .show_on_startup
        );

        // After the install the notes are current and can be marked seen.
        let installed = load_latest_from_path(&path, "0.9.5 (duty-v0.9.5.1)").expect("installed");
        assert!(!installed.preview);
        mark_current_version_seen_at(&path, "0.9.5 (duty-v0.9.5.1)").unwrap();
        assert!(
            !load_stored_from_path(&path)
                .expect("stored")
                .show_on_startup
        );

        clear_pending_at(&path).unwrap();
    }

    #[test]
    fn version_only_notes_are_current_for_any_build_of_that_version() {
        let path = std::env::temp_dir().join(format!(
            "herdr-release-notes-{}-{}.json",
            std::process::id(),
            "version-only"
        ));
        let _ = clear_pending_at(&path);
        save_pending_to_path(&path, "0.9.6", "### Changed\n- One").unwrap();

        let notes = load_latest_from_path(&path, "0.9.6 (duty-v0.9.6.1)").expect("latest notes");
        assert!(!notes.preview);
        mark_current_version_seen_at(&path, "0.9.6 (duty-v0.9.6.1)").unwrap();
        assert!(
            !load_stored_from_path(&path)
                .expect("stored")
                .show_on_startup
        );

        clear_pending_at(&path).unwrap();
    }
}
