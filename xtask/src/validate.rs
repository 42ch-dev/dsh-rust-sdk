//! `release-validate`: pre-tag guards for a `v<version>` release tag
//! (spec §5): tag format, `Cargo.toml` version equality, git tag absence,
//! and the CHANGELOG section. Read-only; never mutates the tree.

use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};
use semver::Version;

use crate::notes;
use crate::version::read_package_version;

/// Validate that release tag `version_tag` (format `v<semver>`) is ready
/// to ship from the repository at `root`.
pub fn validate(version_tag: &str, root: &Path) -> Result<()> {
    let version = parse_tag(version_tag)?;

    let cargo_version = read_package_version(&root.join("Cargo.toml"))?;
    if cargo_version != version {
        bail!("Cargo.toml version `{cargo_version}` does not match tag version `{version}`");
    }

    let tag = format!("v{version}");
    match git_tag_status(root, &tag)? {
        GitTagStatus::NotARepo => {
            eprintln!("release-validate: not a git repository; skipping git tag check");
        }
        GitTagStatus::Absent => {}
        GitTagStatus::Exists => bail!("git tag `{tag}` already exists"),
    }

    let changelog = root.join("CHANGELOG.md");
    let text = fs::read_to_string(&changelog)
        .with_context(|| format!("reading {}", changelog.display()))?;
    if notes::extract(&text, &version.to_string()).is_none() {
        bail!("{} has no `## [{version}]` section", changelog.display());
    }
    Ok(())
}

/// Parse a release tag argument as `v` + SemVer.
fn parse_tag(version_tag: &str) -> Result<Version> {
    let stripped = version_tag
        .strip_prefix('v')
        .with_context(|| format!("release tag `{version_tag}` must start with `v`"))?;
    Version::parse(stripped)
        .with_context(|| format!("release tag `{version_tag}` is not `v` + SemVer"))
}

/// Git tag probe result: outside a repository the check is skipped with a
/// note (the guard only matters on a real checkout, like the fixtures-free
/// `prepare` path).
enum GitTagStatus {
    NotARepo,
    Absent,
    Exists,
}

/// Probe whether git tag `tag` exists in the repository containing `root`.
/// Same probe style as `prepare::tag_exists` (`git rev-parse --verify
/// --quiet` on `refs/tags/<tag>`), plus an explicit work-tree check so a
/// non-repository can be distinguished from "tag absent".
fn git_tag_status(root: &Path, tag: &str) -> Result<GitTagStatus> {
    let in_repo = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "--is-inside-work-tree"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .with_context(|| format!("running `git rev-parse` in {}", root.display()))?
        .success();
    if !in_repo {
        return Ok(GitTagStatus::NotARepo);
    }
    let exists = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "--verify", "--quiet"])
        .arg(format!("refs/tags/{tag}"))
        .stderr(Stdio::null())
        .status()
        .with_context(|| format!("running `git rev-parse` in {}", root.display()))?
        .success();
    Ok(if exists {
        GitTagStatus::Exists
    } else {
        GitTagStatus::Absent
    })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::process::Command;

    use tempfile::tempdir;

    use super::*;

    const FAKE_CARGO: &str = "[package]\nname = \"deepseek-harness-sdk\"\nversion = \"0.1.0-alpha.1\"\nedition = \"2021\"\n";

    const CHANGELOG: &str = "\
# Changelog

## [Unreleased]
- unreleased work

## [0.1.0-alpha.1] - 2026-08-16
- first release
";

    fn fixture_tree() -> tempfile::TempDir {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("Cargo.toml"), FAKE_CARGO).unwrap();
        fs::write(dir.path().join("CHANGELOG.md"), CHANGELOG).unwrap();
        dir
    }

    /// Make `dir` a git repository with one commit (no tags).
    fn git_init(dir: &std::path::Path) {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["init", "-q"])
            .output()
            .unwrap();
        assert!(out.status.success(), "git init failed");
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.email=test@example.com", "-c", "user.name=test"])
            .args(["commit", "--allow-empty", "-q", "-m", "init"])
            .output()
            .unwrap();
        assert!(out.status.success(), "git commit failed");
    }

    #[test]
    fn validate_ok_when_all_checks_pass() {
        // Non-repo fixture: the git-tag check is skipped with a note.
        let dir = fixture_tree();
        validate("v0.1.0-alpha.1", dir.path()).unwrap();
    }

    #[test]
    fn validate_ok_in_repo_when_tag_absent() {
        let dir = fixture_tree();
        git_init(dir.path());
        validate("v0.1.0-alpha.1", dir.path()).unwrap();
    }

    #[test]
    fn validate_fails_when_tag_already_exists() {
        let dir = fixture_tree();
        git_init(dir.path());
        let out = Command::new("git")
            .arg("-C")
            .arg(dir.path())
            .args(["-c", "tag.gpgSign=false"])
            .args(["tag", "v0.1.0-alpha.1"])
            .output()
            .unwrap();
        assert!(out.status.success(), "git tag failed");
        let err = validate("v0.1.0-alpha.1", dir.path())
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("already exists"),
            "error should mention the tag: {err}"
        );
    }

    #[test]
    fn validate_fails_on_cargo_version_mismatch() {
        let dir = fixture_tree();
        fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"deepseek-harness-sdk\"\nversion = \"0.1.0-alpha.2\"\nedition = \"2021\"\n",
        )
        .unwrap();
        let err = validate("v0.1.0-alpha.1", dir.path())
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("does not match"),
            "error should compare versions: {err}"
        );
    }

    #[test]
    fn validate_fails_on_missing_changelog_section() {
        let dir = fixture_tree();
        fs::write(
            dir.path().join("CHANGELOG.md"),
            "# Changelog\n\n## [Unreleased]\n- work\n",
        )
        .unwrap();
        let err = validate("v0.1.0-alpha.1", dir.path())
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("## [0.1.0-alpha.1]"),
            "error should name the missing section: {err}"
        );
    }

    #[test]
    fn validate_fails_when_changelog_missing() {
        let dir = fixture_tree();
        fs::remove_file(dir.path().join("CHANGELOG.md")).unwrap();
        let err = validate("v0.1.0-alpha.1", dir.path())
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("CHANGELOG.md"),
            "error should name the file: {err}"
        );
    }

    #[test]
    fn validate_fails_on_bad_tag_format() {
        let dir = fixture_tree();
        for bad in ["0.1.0-alpha.1", "vabc", "v1.2", "vv0.1.0"] {
            let err = validate(bad, dir.path()).unwrap_err().to_string();
            assert!(!err.is_empty(), "expected a format error for `{bad}`");
        }
    }
}
