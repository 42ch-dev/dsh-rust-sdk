//! Fragment collection and changelog section assembly (spec §2).
//!
//! A fragment is one user-visible change group under
//! `.changes/unreleased/`: a slug `.md` file with optional `---`-fenced
//! frontmatter (`category:` key, default `Changed`) and a body of English
//! lines, at least one of which is a `- ` bullet. `README.md` and dotfiles
//! (like `.gitkeep`) are ignored by collection.

use std::fs;
use std::path::Path;

use anyhow::{bail, Context, Result};

/// One collected fragment. `body` is the file content after frontmatter
/// with blank padding at the edges trimmed and exactly one trailing `\n`;
/// interior lines are verbatim.
#[derive(Debug)]
pub struct Fragment {
    pub name: String,
    pub category: String,
    pub body: String,
}

/// Default category when a fragment has no `category:` frontmatter.
const DEFAULT_CATEGORY: &str = "Changed";

/// Canonical category order (spec §2); other categories render after these,
/// in first-seen (filename-sorted) order.
const CANONICAL_CATEGORIES: [&str; 6] = [
    "Added",
    "Changed",
    "Deprecated",
    "Removed",
    "Fixed",
    "Security",
];

/// Collect fragments from `dir`, skipping `README.md` and dotfiles.
/// The returned order is unspecified; `build_section_body` owns ordering.
pub fn collect(dir: &Path) -> Result<Vec<Fragment>> {
    let mut fragments = Vec::new();
    let entries =
        fs::read_dir(dir).with_context(|| format!("reading fragment dir {}", dir.display()))?;
    for entry in entries {
        let entry = entry.with_context(|| format!("reading entry in {}", dir.display()))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == "README.md" || name.starts_with('.') {
            continue;
        }
        if !entry.file_type().context("stat fragment")?.is_file() {
            continue;
        }
        let path = entry.path();
        let content = fs::read_to_string(&path)
            .with_context(|| format!("reading fragment {}", path.display()))?;
        fragments.push(parse_fragment(&name, &content)?);
    }
    Ok(fragments)
}

/// Assemble the rendered `### <Category>` section body for the release
/// section. Categories appear in canonical order, then non-canonical
/// categories in first-seen order over fragments sorted by filename
/// (lexicographic byte order, Rust `str` `Ord` — platform-independent).
/// Fragments sharing a category merge under one heading; fragment order
/// inside a category is that same filename sort. Body lines render
/// verbatim. Returns `""` for an empty slice.
pub fn build_section_body(fragments: &[Fragment]) -> String {
    let mut sorted: Vec<&Fragment> = fragments.iter().collect();
    sorted.sort_by(|a, b| a.name.cmp(&b.name));

    // Category order: canonical first, then non-canonical in first-seen
    // order (stable sort keeps first-seen among the `usize::MAX` group).
    let mut categories: Vec<String> = Vec::new();
    for fragment in &sorted {
        if !categories.contains(&fragment.category) {
            categories.push(fragment.category.clone());
        }
    }
    categories.sort_by_key(|c| {
        CANONICAL_CATEGORIES
            .iter()
            .position(|k| *k == c.as_str())
            .unwrap_or(usize::MAX)
    });

    let mut blocks: Vec<String> = Vec::new();
    for category in &categories {
        let mut block = format!("### {category}\n");
        for fragment in sorted.iter().filter(|f| f.category == *category) {
            block.push_str(&fragment.body);
        }
        if block.ends_with('\n') {
            block.pop();
        }
        blocks.push(block);
    }
    if blocks.is_empty() {
        return String::new();
    }
    blocks.join("\n\n") + "\n"
}

/// Parse one fragment file: optional `---`-fenced frontmatter with a
/// single `category:` key, then the body. Errors name the file.
fn parse_fragment(name: &str, content: &str) -> Result<Fragment> {
    let (category, body) = split_frontmatter(name, content)?;
    let body = normalize_body(&body);
    if !body.lines().any(|l| l.trim_start().starts_with("- ")) {
        bail!("fragment `{name}` has no `- ` bullet line");
    }
    Ok(Fragment {
        name: name.to_string(),
        category,
        body,
    })
}

/// Split `content` into `(category, body)`. A file whose first line is
/// `---` opens a frontmatter block that must close on a later `---` line;
/// the `category:` key inside defaults to `Changed` when absent. Any other
/// file has no frontmatter and defaults to `Changed`.
fn split_frontmatter(name: &str, content: &str) -> Result<(String, String)> {
    let lines: Vec<&str> = content.split('\n').collect();
    if lines.first().map(|l| l.trim_end()) != Some("---") {
        return Ok((DEFAULT_CATEGORY.to_string(), content.to_string()));
    }
    let close = lines
        .iter()
        .enumerate()
        .skip(1)
        .find(|(_, l)| l.trim_end() == "---")
        .map(|(i, _)| i)
        .with_context(|| format!("fragment `{name}`: opening `---` has no closing fence"))?;

    let mut category = DEFAULT_CATEGORY.to_string();
    for line in &lines[1..close] {
        if let Some(value) = line.trim().strip_prefix("category:") {
            let value = value.trim();
            if value.is_empty() {
                bail!("fragment `{name}`: empty `category:` value");
            }
            category = value.to_string();
            break; // single key per spec; first occurrence wins
        }
    }
    Ok((category, lines[close + 1..].join("\n")))
}

/// Normalize a fragment body: keep interior lines verbatim, trim blank
/// padding at the edges, guarantee exactly one trailing `\n`.
fn normalize_body(body: &str) -> String {
    let mut lines: Vec<&str> = body.lines().collect();
    while lines.first().is_some_and(|l| l.trim().is_empty()) {
        lines.remove(0);
    }
    while lines.last().is_some_and(|l| l.trim().is_empty()) {
        lines.pop();
    }
    if lines.is_empty() {
        return String::new();
    }
    lines.join("\n") + "\n"
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    fn write(dir: &std::path::Path, name: &str, content: &str) {
        fs::write(dir.join(name), content).unwrap();
    }

    #[test]
    fn collect_default_category_and_verbatim_body() {
        let dir = tempdir().unwrap();
        write(dir.path(), "one.md", "- one\n");
        let fragments = collect(dir.path()).unwrap();
        assert_eq!(fragments.len(), 1);
        assert_eq!(fragments[0].name, "one.md");
        assert_eq!(fragments[0].category, "Changed");
        assert_eq!(fragments[0].body, "- one\n");
    }

    #[test]
    fn collect_explicit_category_from_frontmatter() {
        let dir = tempdir().unwrap();
        write(dir.path(), "two.md", "---\ncategory: Added\n---\n- two\n");
        let fragments = collect(dir.path()).unwrap();
        assert_eq!(fragments.len(), 1);
        assert_eq!(fragments[0].category, "Added");
        assert_eq!(fragments[0].body, "- two\n");
    }

    #[test]
    fn collect_trims_frontmatter_value() {
        let dir = tempdir().unwrap();
        write(
            dir.path(),
            "three.md",
            "---\ncategory:   Fixed   \n---\n- three\n",
        );
        let fragments = collect(dir.path()).unwrap();
        assert_eq!(fragments[0].category, "Fixed");
    }

    #[test]
    fn collect_ignores_readme_and_dotfiles() {
        let dir = tempdir().unwrap();
        write(dir.path(), "README.md", "explainer\n");
        write(dir.path(), ".gitkeep", "");
        write(dir.path(), ".hidden.md", "- hidden\n");
        write(dir.path(), "real.md", "- real\n");
        let fragments = collect(dir.path()).unwrap();
        assert_eq!(fragments.len(), 1);
        assert_eq!(fragments[0].name, "real.md");
    }

    #[test]
    fn collect_requires_at_least_one_bullet_naming_the_file() {
        let dir = tempdir().unwrap();
        write(dir.path(), "bad.md", "just prose, no bullet\n");
        let err = collect(dir.path()).unwrap_err().to_string();
        assert!(err.contains("bad.md"), "error should name the file: {err}");
    }

    #[test]
    fn collect_non_bullet_lines_do_not_satisfy_bullet_gate() {
        let dir = tempdir().unwrap();
        write(
            dir.path(),
            "comment-only.md",
            "---\ncategory: Added\n---\n<!-- CN -->\n",
        );
        let err = collect(dir.path()).unwrap_err().to_string();
        assert!(
            err.contains("comment-only.md"),
            "error should name the file: {err}"
        );
    }

    #[test]
    fn collect_body_keeps_non_bullet_lines_verbatim() {
        let dir = tempdir().unwrap();
        write(
            dir.path(),
            "mixed.md",
            "---\ncategory: Fixed\n---\n<!-- CN -->\n- real bullet\nsecond line\n",
        );
        let fragments = collect(dir.path()).unwrap();
        assert_eq!(fragments[0].category, "Fixed");
        assert_eq!(
            fragments[0].body,
            "<!-- CN -->\n- real bullet\nsecond line\n"
        );
    }

    #[test]
    fn collect_unclosed_frontmatter_errors_naming_the_file() {
        let dir = tempdir().unwrap();
        write(dir.path(), "badfm.md", "---\ncategory: Added\n- bullet\n");
        let err = collect(dir.path()).unwrap_err().to_string();
        assert!(
            err.contains("badfm.md"),
            "error should name the file: {err}"
        );
    }

    #[test]
    fn build_section_body_canonical_then_first_seen_order() {
        let fragments = vec![
            Fragment {
                name: "s-sec.md".into(),
                category: "Security".into(),
                body: "- harden auth\n".into(),
            },
            Fragment {
                name: "a-new.md".into(),
                category: "Added".into(),
                body: "- new api\n".into(),
            },
            Fragment {
                name: "also-upd.md".into(),
                category: "Changed".into(),
                body: "- also updated\n".into(),
            },
            Fragment {
                name: "c-upd.md".into(),
                category: "Changed".into(),
                body: "- updated docs\n".into(),
            },
            Fragment {
                name: "b-fix.md".into(),
                category: "Fixed".into(),
                body: "- fix crash\n".into(),
            },
            Fragment {
                name: "p-perf.md".into(),
                category: "Performance".into(),
                body: "- faster parse\n".into(),
            },
        ];
        let expected = "\
### Added
- new api

### Changed
- also updated
- updated docs

### Fixed
- fix crash

### Security
- harden auth

### Performance
- faster parse
";
        assert_eq!(build_section_body(&fragments), expected);
    }

    #[test]
    fn build_section_body_empty_slice_is_empty_string() {
        assert_eq!(build_section_body(&[]), "");
    }
}
