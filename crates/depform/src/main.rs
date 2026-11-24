//! Command line of the dependency graph checker.
//!
//! The binary selects the workspace to inspect and reports what the library
//! found. All graph and rule logic lives in the library, so the binary only
//! maps arguments to calls and results to streams and exit codes.

use std::{env, ffi::OsString, path::PathBuf, process::ExitCode};

use clap::{Parser, Subcommand};
use depform::DepformError;

/// Dependency graph architecture checker for Cargo workspaces.
#[derive(Debug, Parser)]
#[command(version)]
struct Cli {
    /// The workspace graph to print or check.
    #[command(subcommand)]
    command: Command,
}

/// The inspection subcommands.
#[derive(Debug, Subcommand)]
enum Command {
    /// Print the workspace-internal dependency graph.
    Graph {
        /// Manifest of the workspace to load; defaults to the current
        /// directory.
        #[arg(long, value_name = "PATH")]
        manifest_path: Option<PathBuf>,
    },
    /// Check the workspace against its `depform.toml`.
    Check {
        /// Manifest of the workspace to check; defaults to the current
        /// directory.
        #[arg(long, value_name = "PATH")]
        manifest_path: Option<PathBuf>,
        /// Config to read instead of `<workspace-root>/depform.toml`.
        #[arg(long, value_name = "PATH")]
        config: Option<PathBuf>,
    },
}

fn main() -> ExitCode {
    let args = strip_cargo_subcommand(env::args_os().collect(), invoked_by_cargo());
    let cli = Cli::parse_from(args);
    match cli.command {
        Command::Graph { manifest_path } => match depform::load_workspace(manifest_path.as_deref())
        {
            Ok(workspace) => {
                print!("{}", depform::render_graph(workspace.graph()));
                ExitCode::SUCCESS
            }
            Err(error) => fail(error),
        },
        Command::Check {
            manifest_path,
            config,
        } => match depform::check_workspace(manifest_path.as_deref(), config.as_deref()) {
            Ok(violations) if violations.is_empty() => {
                println!("depform check passed");
                ExitCode::SUCCESS
            }
            Ok(violations) => {
                eprint!("{}", depform::render_violations(&violations));
                eprintln!("depform check failed: {}", render_count(violations.len()));
                ExitCode::from(1)
            }
            Err(error) => fail(error),
        },
    }
}

/// Render a violation count with its singular or plural noun.
fn render_count(total: usize) -> String {
    match total {
        1 => "1 violation".to_owned(),
        total => format!("{total} violations"),
    }
}

/// Report a failure on the standard error stream and exit with status 2.
fn fail(error: DepformError) -> ExitCode {
    eprintln!("cargo-depform: {error}");
    ExitCode::from(2)
}

/// Whether Cargo launched this process as an external subcommand. Cargo sets
/// `CARGO` for both `cargo depform` and `cargo run`, but only the latter gives
/// its child a manifest directory, so the absence of `CARGO_MANIFEST_DIR`
/// distinguishes the subcommand convention.
fn invoked_by_cargo() -> bool {
    env::var_os("CARGO").is_some() && env::var_os("CARGO_MANIFEST_DIR").is_none()
}

/// Drop the subcommand name Cargo inserts before the user's arguments. A
/// direct invocation keeps a subcommand word that happens to be `depform`, and
/// `cargo run -- depform` keeps it too because the manifest directory marks a
/// direct child invocation.
fn strip_cargo_subcommand(mut args: Vec<OsString>, invoked_by_cargo: bool) -> Vec<OsString> {
    if invoked_by_cargo && args.get(1).is_some_and(|arg| arg == "depform") {
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
        let direct = args(&["cargo-depform", "depform"]);
        assert_eq!(strip_cargo_subcommand(direct.clone(), false), direct);
        assert_eq!(
            strip_cargo_subcommand(direct, true),
            args(&["cargo-depform"])
        );

        let with_subcommand = args(&["cargo-depform", "depform", "graph"]);
        assert_eq!(
            strip_cargo_subcommand(with_subcommand, true),
            args(&["cargo-depform", "graph"])
        );

        let unrelated = args(&["cargo-depform", "graph"]);
        assert_eq!(strip_cargo_subcommand(unrelated.clone(), true), unrelated);
    }
}
