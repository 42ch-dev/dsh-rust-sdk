//! `release-notes`: extract a version's CHANGELOG section to feed the
//! GitHub Release body (spec §5).

/// Extract the full section for `version` from `changelog`: the
/// `## [<version>]` header line and everything after it up to (but not
/// including) the next `## ` header. `### ` sub-headings inside the
/// section do not terminate it. Returns `None` when the version has no
/// section. Header matching is bracket-bounded: looking up `0.1.0` never
/// matches `## [0.1.0-alpha.1]`. Separator blank lines before the next
/// header are trimmed from the returned section.
pub fn extract(changelog: &str, version: &str) -> Option<String> {
    let header = format!("## [{version}]");
    let lines: Vec<&str> = changelog.lines().collect();
    let start = lines
        .iter()
        .position(|l| l.trim_end().starts_with(&header))?;
    // Same header convention as `prepare::insert_changelog_section`: a
    // section runs until the next `## `-prefixed line (`### ` sub-headings
    // do not terminate it) or EOF.
    let end = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find(|(_, l)| l.starts_with("## "))
        .map(|(i, _)| i)
        .unwrap_or(lines.len());
    // The separator blank line(s) before the next header are not part of
    // this section.
    let mut section: Vec<&str> = lines[start..end].to_vec();
    while section.last().is_some_and(|l| l.trim().is_empty()) {
        section.pop();
    }
    Some(section.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHANGELOG: &str = "\
# Changelog

## [Unreleased]
- unreleased work

## [0.1.0-alpha.1] - 2026-08-16

### Added
- first release

## [0.1.0-alpha.2] - 2026-08-17

### Changed
- second release
- more work
";

    #[test]
    fn extracts_first_released_section_with_header() {
        let section = extract(CHANGELOG, "0.1.0-alpha.1").unwrap();
        assert_eq!(
            section,
            "## [0.1.0-alpha.1] - 2026-08-16\n\n### Added\n- first release"
        );
    }

    #[test]
    fn extracts_middle_section_up_to_next_header() {
        let section = extract(CHANGELOG, "0.1.0-alpha.2").unwrap();
        assert_eq!(
            section,
            "## [0.1.0-alpha.2] - 2026-08-17\n\n### Changed\n- second release\n- more work"
        );
    }

    #[test]
    fn missing_section_returns_none() {
        assert_eq!(extract(CHANGELOG, "9.9.9"), None);
    }

    #[test]
    fn sub_headings_do_not_terminate_the_section() {
        let section = extract(CHANGELOG, "0.1.0-alpha.1").unwrap();
        assert!(section.contains("### Added"));
        assert!(!section.contains("## [0.1.0-alpha.2]"));
    }

    #[test]
    fn version_match_is_bracket_bounded() {
        // Looking up `0.1.0` must not match the `0.1.0-alpha.1` header,
        // and vice versa.
        assert_eq!(extract(CHANGELOG, "0.1.0"), None);
        assert_eq!(
            extract("## [0.1.0] - 2026-08-16\n- x\n", "0.1.0-alpha.1"),
            None
        );
    }

    #[test]
    fn section_at_eof_extends_to_end() {
        let changelog = "# Changelog\n\n## [0.1.0] - 2026-08-16\n- only\n";
        assert_eq!(
            extract(changelog, "0.1.0").unwrap(),
            "## [0.1.0] - 2026-08-16\n- only"
        );
    }
}
