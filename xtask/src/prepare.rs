//! `release-prepare`: version resolution + guards, `Cargo.toml` bump,
//! CHANGELOG section insertion, and fragment archival (spec §2/§3).
//!
//! Mutates the worktree in place; never commits and never refuses a dirty
//! tree (the workflow commits). Every function takes the repo root
//! explicitly so tests can pass `tempfile` fixture trees.

use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use semver::Version;

use crate::fragments::{build_section_body, collect};
use crate::version::{assert_greater, auto_bump, read_package_version, write_package_version};

/// The version a `prepare` run resolved and shipped into the tree.
#[derive(Debug)]
pub struct ResolvedVersion {
    pub version: Version,
}

/// Resolve the release version (explicit or auto-bumped), run the guards,
/// bump `Cargo.toml`, insert the `## [<version>] - <UTC date>` section
/// under `## [Unreleased]`, and archive the consumed fragments to
/// `.changes/archive/<version>/`.
pub fn prepare(root: &Path, version: Option<Version>) -> Result<ResolvedVersion> {
    let cargo_toml = root.join("Cargo.toml");
    let changelog = root.join("CHANGELOG.md");
    let unreleased = root.join(".changes/unreleased");
    let archive = root.join(".changes/archive");

    let current = read_package_version(&cargo_toml)?;
    let version = match version {
        Some(v) => {
            assert_greater(&v, &current)?;
            v
        }
        None => auto_bump(&current)?,
    };

    let tag = format!("v{version}");
    if tag_exists(root, &tag)? {
        bail!("git tag `{tag}` already exists; refusing to re-release");
    }

    let fragments = collect(&unreleased)?;
    if fragments.is_empty() {
        bail!(
            "no fragments to release in {}; write at least one `- ` bullet fragment first",
            unreleased.display()
        );
    }

    // Changelog guard: fail before mutating anything. A missing file is
    // fine (created below); an existing file without the `## [Unreleased]`
    // header is a hard error — never guess the insertion point after the
    // version has already been written.
    if changelog.exists() {
        let text = fs::read_to_string(&changelog)
            .with_context(|| format!("reading {}", changelog.display()))?;
        if !text.lines().any(|l| l.trim_end() == "## [Unreleased]") {
            bail!("{} has no `## [Unreleased]` header", changelog.display());
        }
    }

    write_package_version(&cargo_toml, &version)?;

    let section = format!(
        "## [{version}] - {}\n\n{}",
        utc_date(SystemTime::now()),
        build_section_body(&fragments)
    );
    insert_changelog_section(&changelog, &section)?;

    let dest = archive.join(version.to_string());
    fs::create_dir_all(&dest)
        .with_context(|| format!("creating archive dir {}", dest.display()))?;
    for fragment in &fragments {
        let from = unreleased.join(&fragment.name);
        let to = dest.join(&fragment.name);
        fs::rename(&from, &to).with_context(|| {
            format!("archiving fragment {} to {}", from.display(), to.display())
        })?;
    }

    Ok(ResolvedVersion { version })
}

/// Check whether git tag `tag` exists in the repository containing `root`.
///
/// `git rev-parse --verify --quiet` exits 0 when the ref exists. Outside a
/// git repository it exits non-zero, which we treat as "no such tag": the
/// guard only matters on a real checkout, and fixture trees in tests are
/// not necessarily repositories.
fn tag_exists(root: &Path, tag: &str) -> Result<bool> {
    let status = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "--verify", "--quiet"])
        .arg(format!("refs/tags/{tag}"))
        .stderr(Stdio::null())
        .status()
        .with_context(|| format!("running `git rev-parse` in {}", root.display()))?;
    Ok(status.success())
}

/// Insert `section` directly under the `## [Unreleased]` header of the
/// CHANGELOG at `path` — i.e. at the end of the Unreleased section,
/// before the next `## ` header or at EOF. A missing file is created as
/// `# Changelog` + an empty `## [Unreleased]`; an existing file without
/// the header is an error — we never guess an insertion point.
fn insert_changelog_section(path: &Path, section: &str) -> Result<()> {
    if !path.exists() {
        fs::write(path, "# Changelog\n\n## [Unreleased]\n")
            .with_context(|| format!("creating {}", path.display()))?;
    }
    let text = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let lines: Vec<&str> = text.lines().collect();

    let header = lines
        .iter()
        .position(|l| l.trim_end() == "## [Unreleased]")
        .with_context(|| format!("{} has no `## [Unreleased]` header", path.display()))?;
    let next = lines
        .iter()
        .enumerate()
        .skip(header + 1)
        .find(|(_, l)| l.starts_with("## "))
        .map(|(i, _)| i)
        .unwrap_or(lines.len());

    let mut insert: Vec<&str> = Vec::new();
    if next == 0 || !lines[next - 1].trim().is_empty() {
        insert.push("");
    }
    insert.extend(section.trim_end_matches('\n').split('\n'));
    if next < lines.len() {
        insert.push("");
    }

    let mut out = Vec::with_capacity(lines.len() + insert.len());
    out.extend_from_slice(&lines[..next]);
    out.extend(insert);
    out.extend_from_slice(&lines[next..]);

    let content = out.join("\n") + "\n";
    fs::write(path, content).with_context(|| format!("writing {}", path.display()))
}

/// Current UTC date as `YYYY-MM-DD` (release section header format).
fn utc_date(now: SystemTime) -> String {
    let secs = now
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let days = secs.div_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02}")
}

/// Days since 1970-01-01 to `(year, month, day)` in the proleptic Gregorian
/// calendar (Howard Hinnant's `civil_from_days` algorithm). `std` has no
/// calendar arithmetic; a dependency is not worth it for one date.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m as u32, d as u32)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::process::Command;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use tempfile::tempdir;

    use super::*;

    const FAKE_CARGO: &str = "[package]\nname = \"deepseek-harness-sdk\"\nversion = \"0.1.0-alpha.1\"\nedition = \"2021\"\n";

    const CHANGELOG_WITH_UNRELEASED: &str = "\
# Changelog

## [Unreleased]
- unreleased work

## [0.1.0-alpha.1] - 2026-08-16
- old stuff
";

    fn fixture_tree() -> tempfile::TempDir {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join(".changes/unreleased")).unwrap();
        fs::create_dir_all(dir.path().join(".changes/archive")).unwrap();
        fs::write(dir.path().join("Cargo.toml"), FAKE_CARGO).unwrap();
        dir
    }

    fn write(dir: &std::path::Path, name: &str, content: &str) {
        fs::write(dir.join(name), content).unwrap();
    }

    /// Make `dir` a git repository containing an annotated-capable tag.
    fn git_repo_with_tag(dir: &std::path::Path, tag: &str) {
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
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "tag.gpgSign=false"])
            .args(["tag", tag])
            .output()
            .unwrap();
        assert!(out.status.success(), "git tag failed");
    }

    fn cargo_version(dir: &std::path::Path) -> Version {
        read_package_version(&dir.join("Cargo.toml")).unwrap()
    }

    fn unreleased_names(dir: &std::path::Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir.join(".changes/unreleased"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn prepare_auto_bumps_and_assembles_full_release() {
        let dir = fixture_tree();
        write(dir.path(), "CHANGELOG.md", CHANGELOG_WITH_UNRELEASED);
        // Ignored files stay put; only collectable fragments are archived.
        write(
            &dir.path().join(".changes/unreleased"),
            "README.md",
            "explainer\n",
        );
        write(&dir.path().join(".changes/unreleased"), ".gitkeep", "");
        write(
            &dir.path().join(".changes/unreleased"),
            "a-feature.md",
            "---\ncategory: Added\n---\n- add a\n",
        );
        write(
            &dir.path().join(".changes/unreleased"),
            "b-fix.md",
            "- fix b\n",
        );

        let resolved = prepare(dir.path(), None).unwrap();
        assert_eq!(resolved.version.to_string(), "0.1.0-alpha.2");

        // Cargo.toml bumped, rest of the file intact.
        assert_eq!(cargo_version(dir.path()).to_string(), "0.1.0-alpha.2");
        let cargo = fs::read_to_string(dir.path().join("Cargo.toml")).unwrap();
        assert!(cargo.contains("name = \"deepseek-harness-sdk\""));

        // Section inserted at the right line: after Unreleased content,
        // before the previous released section, with blank-line separation.
        let changelog = fs::read_to_string(dir.path().join("CHANGELOG.md")).unwrap();
        let lines: Vec<&str> = changelog.lines().collect();
        assert_eq!(lines[0], "# Changelog");
        assert_eq!(lines[2], "## [Unreleased]");
        assert_eq!(lines[3], "- unreleased work");
        assert_eq!(lines[4], "");
        assert!(
            lines[5].starts_with("## [0.1.0-alpha.2] - "),
            "{}",
            lines[5]
        );
        let date = &lines[5]["## [0.1.0-alpha.2] - ".len()..];
        assert_eq!(date.len(), 10, "UTC date should be YYYY-MM-DD: {date}");
        assert!(date
            .chars()
            .enumerate()
            .all(|(i, c)| c.is_ascii_digit() || (i == 4 || i == 7)));
        assert_eq!(lines[7], "### Added");
        assert_eq!(lines[8], "- add a");
        assert_eq!(lines[10], "### Changed");
        assert_eq!(lines[11], "- fix b");
        assert_eq!(lines[12], "");
        assert_eq!(lines[13], "## [0.1.0-alpha.1] - 2026-08-16");
        assert_eq!(lines[14], "- old stuff");

        // Fragments archived preserving names and content.
        let archive = dir.path().join(".changes/archive/0.1.0-alpha.2");
        assert_eq!(
            fs::read_to_string(archive.join("a-feature.md")).unwrap(),
            "---\ncategory: Added\n---\n- add a\n"
        );
        assert_eq!(
            fs::read_to_string(archive.join("b-fix.md")).unwrap(),
            "- fix b\n"
        );

        // Unreleased dir left with only the ignored files.
        assert_eq!(unreleased_names(dir.path()), vec![".gitkeep", "README.md"]);
    }

    #[test]
    fn prepare_creates_changelog_when_missing() {
        let dir = fixture_tree();
        write(
            &dir.path().join(".changes/unreleased"),
            "a-feature.md",
            "- add a\n",
        );

        let resolved = prepare(dir.path(), None).unwrap();
        assert_eq!(resolved.version.to_string(), "0.1.0-alpha.2");

        let changelog = fs::read_to_string(dir.path().join("CHANGELOG.md")).unwrap();
        let lines: Vec<&str> = changelog.lines().collect();
        assert_eq!(lines[0], "# Changelog");
        assert_eq!(lines[1], "");
        assert_eq!(lines[2], "## [Unreleased]");
        assert_eq!(lines[3], "");
        assert!(lines[4].starts_with("## [0.1.0-alpha.2] - "));
        assert_eq!(lines[6], "### Changed");
        assert_eq!(lines[7], "- add a");
    }

    #[test]
    fn prepare_errors_when_changelog_has_no_unreleased_header() {
        let dir = fixture_tree();
        write(
            dir.path(),
            "CHANGELOG.md",
            "# Changelog\n\n## [0.1.0-alpha.1] - 2026-08-16\n- old\n",
        );
        write(
            &dir.path().join(".changes/unreleased"),
            "a-feature.md",
            "- add a\n",
        );

        let err = prepare(dir.path(), None).unwrap_err().to_string();
        assert!(
            err.contains("## [Unreleased]"),
            "error should name the header: {err}"
        );
        assert_eq!(cargo_version(dir.path()).to_string(), "0.1.0-alpha.1");
    }

    #[test]
    fn prepare_errors_on_no_fragments() {
        let dir = fixture_tree();
        write(dir.path(), "CHANGELOG.md", CHANGELOG_WITH_UNRELEASED);
        // Only ignored files in unreleased/.
        write(
            &dir.path().join(".changes/unreleased"),
            "README.md",
            "explainer\n",
        );
        write(&dir.path().join(".changes/unreleased"), ".gitkeep", "");

        let err = prepare(dir.path(), None).unwrap_err().to_string();
        assert!(
            err.contains("fragment"),
            "error should mention fragments: {err}"
        );
        assert_eq!(cargo_version(dir.path()).to_string(), "0.1.0-alpha.1");
    }

    #[test]
    fn prepare_errors_when_tag_already_exists() {
        let dir = fixture_tree();
        git_repo_with_tag(dir.path(), "v0.1.0-alpha.2");
        write(dir.path(), "CHANGELOG.md", CHANGELOG_WITH_UNRELEASED);
        write(
            &dir.path().join(".changes/unreleased"),
            "a-feature.md",
            "- add a\n",
        );

        let err = prepare(dir.path(), None).unwrap_err().to_string();
        assert!(
            err.contains("already exists"),
            "error should mention the tag: {err}"
        );
        assert_eq!(cargo_version(dir.path()).to_string(), "0.1.0-alpha.1");
    }

    #[test]
    fn prepare_errors_when_explicit_version_is_not_greater() {
        let dir = fixture_tree();
        write(dir.path(), "CHANGELOG.md", CHANGELOG_WITH_UNRELEASED);
        write(
            &dir.path().join(".changes/unreleased"),
            "a-feature.md",
            "- add a\n",
        );

        for bad in ["0.1.0-alpha.1", "0.1.0-alpha.0"] {
            let version = Version::parse(bad).unwrap();
            let err = prepare(dir.path(), Some(version)).unwrap_err().to_string();
            assert!(
                err.contains("not strictly greater"),
                "expected greater-guard error for {bad}: {err}"
            );
        }
        assert_eq!(cargo_version(dir.path()).to_string(), "0.1.0-alpha.1");
    }

    #[test]
    fn prepare_accepts_explicit_channel_jump() {
        let dir = fixture_tree();
        write(dir.path(), "CHANGELOG.md", CHANGELOG_WITH_UNRELEASED);
        write(
            &dir.path().join(".changes/unreleased"),
            "a-feature.md",
            "- add a\n",
        );

        let resolved = prepare(dir.path(), Some(Version::parse("0.2.0").unwrap())).unwrap();
        assert_eq!(resolved.version.to_string(), "0.2.0");
        assert_eq!(cargo_version(dir.path()).to_string(), "0.2.0");
        let changelog = fs::read_to_string(dir.path().join("CHANGELOG.md")).unwrap();
        assert!(changelog.contains("## [0.2.0] - "));
        assert!(dir
            .path()
            .join(".changes/archive/0.2.0/a-feature.md")
            .exists());
    }

    #[test]
    fn utc_date_renders_fixed_epochs() {
        assert_eq!(utc_date(UNIX_EPOCH), "1970-01-01");
        assert_eq!(
            utc_date(UNIX_EPOCH + Duration::from_secs(946_684_800)),
            "2000-01-01"
        );
        assert_eq!(
            utc_date(UNIX_EPOCH + Duration::from_secs(1_786_838_400)),
            "2026-08-16"
        );
        assert_eq!(utc_date(SystemTime::now()).len(), 10);
    }
}
