//! `xtask` — release automation for `deepseek-harness-sdk`.
//!
//! Thin dispatcher shell. Exit codes: 0 ok / 1 failure (guards) / 2 usage.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use semver::Version;

mod fragments;
mod notes;
mod prepare;
mod validate;
mod version;

const USAGE: &str = "\
usage: cargo xtask <subcommand>

subcommands:
  release-prepare [--auto | <version>]  resolve version, assemble fragments,
                                        bump Cargo.toml, archive fragments
  release-validate <v-version>          check tag format, Cargo.toml version,
                                        git tag absence, CHANGELOG section
  release-notes <version>               print the CHANGELOG section
                                        (exit 1 when missing)";

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
        "release-validate" => release_validate(&mut args),
        "release-notes" => release_notes(&mut args),
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

/// `cargo xtask release-validate <v-version>`.
///
/// Exit 0 when every guard passes; exit 1 on any validation failure
/// (bad tag format included); exit 2 on usage.
fn release_validate(args: &mut impl Iterator<Item = String>) -> ExitCode {
    let Some(version_tag) = args.next() else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    if args.next().is_some() {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    }
    match validate::validate(&version_tag, &repo_root()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("release-validate: {err:#}");
            ExitCode::from(1)
        }
    }
}

/// `cargo xtask release-notes <version>`.
///
/// Prints the version's CHANGELOG section (header line included) on
/// stdout. Exit 1 when the section is missing — the Release workflow
/// refuses to create an empty GitHub Release based on this exit code.
fn release_notes(args: &mut impl Iterator<Item = String>) -> ExitCode {
    let Some(version) = args.next() else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    if args.next().is_some() {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    }
    let changelog = repo_root().join("CHANGELOG.md");
    let text = match fs::read_to_string(&changelog) {
        Ok(text) => text,
        Err(err) => {
            eprintln!("release-notes: reading {}: {err}", changelog.display());
            return ExitCode::from(1);
        }
    };
    match notes::extract(&text, &version) {
        Some(section) => {
            println!("{section}");
            ExitCode::SUCCESS
        }
        None => {
            eprintln!(
                "release-notes: {} has no `## [{version}]` section",
                changelog.display()
            );
            ExitCode::from(1)
        }
    }
}
