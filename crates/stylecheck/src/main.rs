//! Command line of the style checker.
//!
//! The binary only selects the scanned root and the optional config file
//! override, and reports what [`stylecheck::check_tree_with_config`] returns.
//! The walk, the config and every rule live in the library, so another
//! project can reuse them without this repository's layout.

use std::{env, ffi::OsString, path::PathBuf, process::ExitCode};

use clap::Parser;

/// Layout style checker for Rust sources and Cargo/TOML manifests.
#[derive(Debug, Parser)]
#[command(version)]
struct Cli {
    /// Read the style config from this file instead of `stylecheck.toml` at
    /// the scanned root; the file must exist.
    #[arg(short, long, value_name = "FILE")]
    config: Option<PathBuf>,
    /// The tree to check; defaults to the current directory.
    root: Option<PathBuf>,
}

fn main() -> ExitCode {
    let args = strip_cargo_subcommand(env::args_os().collect(), invoked_by_cargo());
    let cli = Cli::parse_from(args);
    let root = match cli.root {
        Some(root) => root,
        None => match env::current_dir() {
            Ok(root) => root,
            Err(error) => {
                eprintln!("cargo-stylecheck: failed to resolve the scanned root: {error}");
                return ExitCode::from(2);
            }
        },
    };
    match stylecheck::check_tree_with_config(&root, cli.config.as_deref()) {
        Ok(found) => {
            if found.is_empty() {
                println!("style check passed");
                ExitCode::SUCCESS
            } else {
                for violation in &found {
                    println!("{}", violation.report());
                }
                println!("style check failed: {} violation(s)", found.len());
                ExitCode::from(1)
            }
        }
        Err(error) => {
            eprintln!("cargo-stylecheck: {error}");
            ExitCode::from(2)
        }
    }
}

/// Whether Cargo launched this process as an external subcommand. Cargo sets
/// `CARGO` for both `cargo stylecheck` and `cargo run`, but only the latter
/// gives its child a manifest directory, so the absence of
/// `CARGO_MANIFEST_DIR` distinguishes the subcommand convention.
fn invoked_by_cargo() -> bool {
    env::var_os("CARGO").is_some() && env::var_os("CARGO_MANIFEST_DIR").is_none()
}

/// Drop the subcommand name Cargo inserts before the user's arguments. A
/// direct invocation keeps a root directory named `stylecheck`, and `cargo run
/// -- stylecheck` keeps it too because the manifest directory marks a direct
/// child invocation.
fn strip_cargo_subcommand(mut args: Vec<OsString>, invoked_by_cargo: bool) -> Vec<OsString> {
    if invoked_by_cargo && args.get(1).is_some_and(|arg| arg == "stylecheck") {
        args.remove(1);
    }
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[test]
    fn cargo_subcommand_token_is_stripped_only_for_external_invocation() {
        let direct = args(&["cargo-stylecheck", "stylecheck"]);
        assert_eq!(strip_cargo_subcommand(direct.clone(), false), direct);
        assert_eq!(
            strip_cargo_subcommand(direct, true),
            args(&["cargo-stylecheck"])
        );

        let with_root = args(&["cargo-stylecheck", "stylecheck", "root"]);
        assert_eq!(
            strip_cargo_subcommand(with_root, true),
            args(&["cargo-stylecheck", "root"])
        );

        let unrelated = args(&["cargo-stylecheck", "root"]);
        assert_eq!(strip_cargo_subcommand(unrelated.clone(), true), unrelated);
    }
}
