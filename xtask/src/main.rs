//! `xtask` — release automation for `deepseek-harness-sdk`.
//!
//! This task ships the thin dispatcher shell only; the subcommands
//! (`release-prepare` / `release-validate` / `release-notes`) are wired in
//! by later tasks. Every subcommand currently errors `unimplemented`
//! with exit code 2.

use std::process::ExitCode;

// Tasks 2–3 wire the subcommands; until then the version utilities are
// unused and would otherwise fail clippy's `-D warnings` gate.
#[allow(dead_code)]
mod version;

fn main() -> ExitCode {
    let Some(subcommand) = std::env::args().nth(1) else {
        eprintln!("usage: cargo xtask <subcommand>");
        return ExitCode::from(2);
    };
    eprintln!("unimplemented: {subcommand}");
    ExitCode::from(2)
}
