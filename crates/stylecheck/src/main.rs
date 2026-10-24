//! Command line of the style checker.
//!
//! The binary only resolves the scanned root and reports what
//! [`stylecheck::check_tree`] returns. The walk, the config and every
//! rule live in the library, so another project can reuse them without this
//! repository's layout.

use std::{env, path::PathBuf, process::ExitCode};

fn main() -> ExitCode {
    let root = match scanned_root() {
        Ok(root) => root,
        Err(message) => {
            eprintln!("cargo-stylecheck: {message}");
            return ExitCode::from(2);
        }
    };
    match stylecheck::check_tree(&root) {
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

/// The scanned root: the optional first argument, else the current directory.
fn scanned_root() -> Result<PathBuf, String> {
    let mut arguments = env::args_os().skip(1);
    let root = arguments.next().map(PathBuf::from);
    if arguments.next().is_some() {
        return Err("usage: cargo-stylecheck [root]".to_owned());
    }
    match root {
        Some(root) => Ok(root),
        None => env::current_dir()
            .map_err(|error| format!("failed to resolve the scanned root: {error}")),
    }
}
