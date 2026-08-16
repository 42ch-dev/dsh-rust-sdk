//! `xtask` — release automation for `deepseek-harness-sdk`.
//!
//! Thin dispatcher shell. `release-prepare` is implemented here;
//! `release-validate` / `release-notes` are wired in by a later task and
//! still error `unimplemented` with exit code 2. Exit codes: 0 ok /
//! 1 failure (guards) / 2 usage.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use semver::Version;

mod fragments;
mod prepare;
mod version;

const USAGE: &str = "\
usage: cargo xtask <subcommand>

subcommands:
  release-prepare [--auto | <version>]  resolve version, assemble fragments,
                                        bump Cargo.toml, archive fragments
  release-validate <v-version>          (unimplemented)
  release-notes <version>               (unimplemented)";

/// Repo root, resolved at compile time from the xtask manifest parent
/// (cwd-independent).
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask is a workspace member directly below the repo root")
        .to_path_buf()
}

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let Some(subcommand) = args.next() else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    match subcommand.as_str() {
        "release-prepare" => release_prepare(&mut args),
        other => {
            eprintln!("unimplemented: {other}\n{USAGE}");
            ExitCode::from(2)
        }
    }
}

/// `cargo xtask release-prepare --auto | <version>`.
///
/// Prints the resolved version on stdout (the workflow parses it to run
/// `release-validate v<version>`); progress goes to stderr.
fn release_prepare(args: &mut impl Iterator<Item = String>) -> ExitCode {
    let version = match args.next() {
        None => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
        Some(arg) if arg == "--auto" => {
            if args.next().is_some() {
                eprintln!("{USAGE}");
                return ExitCode::from(2);
            }
            None
        }
        Some(arg) => {
            if args.next().is_some() {
                eprintln!("{USAGE}");
                return ExitCode::from(2);
            }
            match Version::parse(&arg) {
                Ok(version) => Some(version),
                Err(err) => {
                    eprintln!("invalid version `{arg}`: {err}\n{USAGE}");
                    return ExitCode::from(2);
                }
            }
        }
    };
    match prepare::prepare(&repo_root(), version) {
        Ok(resolved) => {
            println!("{}", resolved.version);
            eprintln!(
                "release-prepare: bumped Cargo.toml, inserted CHANGELOG section, archived fragments"
            );
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("release-prepare: {err:#}");
            ExitCode::from(1)
        }
    }
}
