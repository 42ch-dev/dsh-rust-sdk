//! Version utilities for the release pipeline.
//!
//! Provides format-preserving read/write access to the root `Cargo.toml`
//! `[package] version` (via `toml_edit`) and the version-resolution helpers
//! used by `release-prepare`: auto-bump and the strictly-greater guard.
//!
//! Every function takes an explicit path so tests can point them at
//! `tempfile` fixture trees; the CLI layer resolves the repo root once via
//! `env!("CARGO_MANIFEST_DIR")`'s parent (cwd-independent).

use std::fs;
use std::path::Path;

use anyhow::{bail, Context, Result};
use semver::{BuildMetadata, Prerelease, Version};
use toml_edit::{DocumentMut, Value};

/// Read `[package] version` from the Cargo.toml at `path`.
pub fn read_package_version(path: &Path) -> Result<Version> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("reading Cargo.toml at {}", path.display()))?;
    let doc: DocumentMut = text
        .parse()
        .with_context(|| format!("parsing Cargo.toml at {}", path.display()))?;
    let raw = doc
        .get("package")
        .and_then(|p| p.get("version"))
        .and_then(toml_edit::Item::as_str)
        .with_context(|| format!("`[package] version` missing in {}", path.display()))?;
    Version::parse(raw)
        .with_context(|| format!("invalid `[package] version` `{raw}` in {}", path.display()))
}

/// Write `[package] version` in the Cargo.toml at `path`,
/// preserving the rest of the file's formatting.
pub fn write_package_version(path: &Path, version: &Version) -> Result<()> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("reading Cargo.toml at {}", path.display()))?;
    let mut doc: DocumentMut = text
        .parse()
        .with_context(|| format!("parsing Cargo.toml at {}", path.display()))?;
    let value = doc
        .get_mut("package")
        .and_then(|p| p.get_mut("version"))
        .and_then(toml_edit::Item::as_value_mut)
        .with_context(|| format!("`[package] version` missing in {}", path.display()))?;
    // Carry the old value's decor (surrounding whitespace/comments) over to
    // the replacement so the file changes only in the version string itself.
    let prefix = value.decor().prefix().cloned();
    let suffix = value.decor().suffix().cloned();
    *value = Value::from(version.to_string());
    if let Some(prefix) = prefix {
        value.decor_mut().set_prefix(prefix);
    }
    if let Some(suffix) = suffix {
        value.decor_mut().set_suffix(suffix);
    }
    fs::write(path, doc.to_string())
        .with_context(|| format!("writing Cargo.toml at {}", path.display()))
}

/// Auto-bump per the release spec: a prerelease with a numeric tail bumps
/// that tail in line (`0.1.0-alpha.1` -> `0.1.0-alpha.2`); a stable version
/// bumps the patch (`0.1.0` -> `0.1.1`); a prerelease with a non-numeric
/// tail is an error demanding an explicit version. Never jumps channels.
///
/// Build metadata is dropped: it is ignored in precedence and does not
/// belong in a released version.
pub fn auto_bump(current: &Version) -> Result<Version> {
    let mut next = current.clone();
    next.build = BuildMetadata::EMPTY;

    if current.pre.is_empty() {
        next.patch += 1;
        return Ok(next);
    }

    // Operate on the raw prerelease text: `semver`'s `Identifier` type is
    // private, and the string form is exactly what we must reproduce.
    let pre = current.pre.as_str();
    let (prefix, tail) = match pre.rsplit_once('.') {
        Some((prefix, tail)) => (Some(prefix), tail),
        None => (None, pre),
    };
    if !is_numeric_identifier(tail) {
        bail!(
            "cannot auto-bump `{current}`: prerelease tail `{tail}` is not numeric; pass an explicit version"
        );
    }
    let n: u64 = tail
        .parse()
        .context("numeric prerelease tail does not fit u64; pass an explicit version")?;
    let bumped = n
        .checked_add(1)
        .context("numeric prerelease tail overflows u64; pass an explicit version")?;
    let next_pre = match prefix {
        Some(prefix) => format!("{prefix}.{bumped}"),
        None => bumped.to_string(),
    };
    next.pre = Prerelease::new(&next_pre)
        .with_context(|| format!("bumped prerelease `{next_pre}` is invalid"))?;
    Ok(next)
}

/// SemVer numeric identifier: ASCII digits, no leading zeros (`0` alone ok).
fn is_numeric_identifier(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) && !(s.len() > 1 && s.starts_with('0'))
}

/// Assert `new` is strictly greater than `current` under SemVer 2.0.0
/// precedence (`semver` crate `Version` ordering): a release outranks any
/// of its own prereleases (`0.1.0` > `0.1.0-alpha.2`, the explicit channel
/// jump case), and prerelease identifiers compare numerically
/// (`alpha.10` > `alpha.9`). Equal versions are refused.
pub fn assert_greater(new: &Version, current: &Version) -> Result<()> {
    if new > current {
        Ok(())
    } else {
        bail!("version `{new}` is not strictly greater than current `{current}`");
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    fn v(s: &str) -> Version {
        Version::parse(s).expect("valid test semver")
    }

    #[test]
    fn auto_bump_bumps_numeric_prerelease_tail_in_line() {
        assert_eq!(auto_bump(&v("0.1.0-alpha.1")).unwrap(), v("0.1.0-alpha.2"));
    }

    #[test]
    fn auto_bump_bumps_dot_separated_numeric_tail() {
        assert_eq!(auto_bump(&v("0.1.0-rc.3")).unwrap(), v("0.1.0-rc.4"));
    }

    #[test]
    fn auto_bump_stable_bumps_patch() {
        assert_eq!(auto_bump(&v("0.1.0")).unwrap(), v("0.1.1"));
    }

    #[test]
    fn auto_bump_non_numeric_tail_errors() {
        let err = auto_bump(&v("0.1.0-alpha")).unwrap_err();
        assert!(
            err.to_string().contains("explicit"),
            "non-numeric tail must demand an explicit version, got: {err}"
        );
    }

    #[test]
    fn assert_greater_orders_prereleases() {
        assert!(assert_greater(&v("0.1.0-alpha.2"), &v("0.1.0-alpha.1")).is_ok());
        assert!(assert_greater(&v("0.1.0-alpha.1"), &v("0.1.0-alpha.2")).is_err());
    }

    #[test]
    fn assert_greater_release_outranks_prerelease() {
        assert!(assert_greater(&v("0.1.0"), &v("0.1.0-alpha.2")).is_ok());
        assert!(assert_greater(&v("0.1.0-alpha.2"), &v("0.1.0")).is_err());
    }

    #[test]
    fn assert_greater_compares_prerelease_tails_numerically() {
        assert!(assert_greater(&v("0.1.0-alpha.10"), &v("0.1.0-alpha.9")).is_ok());
    }

    #[test]
    fn assert_greater_refuses_equal_versions() {
        assert!(assert_greater(&v("0.1.0-alpha.2"), &v("0.1.0-alpha.2")).is_err());
    }

    #[test]
    fn read_write_round_trip_preserves_formatting() {
        // Fixture: the real repo manifest. The version is read dynamically
        // so the test stays green no matter which release version the repo
        // currently carries (it used to hardcode a constant, which broke as
        // soon as the live version moved past it).
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let original = fs::read_to_string(root.join("Cargo.toml")).unwrap();
        let dir = tempdir().unwrap();
        let manifest = dir.path().join("Cargo.toml");
        fs::write(&manifest, &original).unwrap();

        let version = read_package_version(&manifest).unwrap();

        // Writing the same version back leaves the file byte-identical.
        write_package_version(&manifest, &version).unwrap();
        let rewritten = fs::read_to_string(&manifest).unwrap();
        assert_eq!(rewritten, original, "round-trip must be byte-identical");

        // Writing a *different*, known version changes only the version
        // value: formatting must be preserved byte-for-byte apart from it,
        // and the new value must read back (no coupling to the live
        // version, which contains the release-verify regression).
        let known = v("9.9.9");
        write_package_version(&manifest, &known).unwrap();
        let rewritten = fs::read_to_string(&manifest).unwrap();
        assert_eq!(
            rewritten,
            original.replace(&version.to_string(), "9.9.9"),
            "only the version value may change; formatting must be preserved"
        );
        assert_eq!(read_package_version(&manifest).unwrap(), known);
    }
}
