# Unreleased fragments

Each file in this directory is one **unreleased change** for the crate.
`cargo xtask release-prepare` collects these files, renders them into
`CHANGELOG.md` under a new `## [<version>] - <UTC date>` section, then
archives them to `.changes/archive/<version>/`.

## Format

One file per user-visible change group; slug filename ending `.md`:

````md
---
category: Added
---
- New crate consumer API: `Config::runtime_bin` override.
- Acquisition hints when the runtime is missing.
````

- **Frontmatter (optional):** a single `category:` key fenced by `---`.
  Defaults to `Changed`. Recommended values: `Added`, `Changed`,
  `Deprecated`, `Removed`, `Fixed`, `Security`. Other values are allowed
  and render after the canonical six, in first-seen filename order.
- **Body:** English, crate-consumer facing. At least one `- ` bullet line
  is required; non-bullet lines render verbatim but do not count toward
  that gate.

`README.md` itself and dotfiles (such as `.gitkeep`) are ignored by
collection.
