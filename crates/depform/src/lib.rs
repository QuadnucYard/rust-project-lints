//! Dependency graph architecture checker for Cargo workspaces.
//!
//! The crate loads the workspace members through `cargo metadata`, projects
//! them into a package graph of workspace-internal dependency declarations,
//! and evaluates declarative rules from `depform.toml` against that graph. The
//! binary is a thin wrapper that maps arguments to these calls and results to
//! exit codes.

mod check;
mod config;
mod error;
mod graph;
mod metadata;
mod report;

use std::path::Path;

pub use check::check_graph;
pub use config::Config;
pub use error::DepformError;
pub use graph::{DependencyKind, Graph};
pub use metadata::{Workspace, load_workspace};
pub use report::{Violation, render_graph, render_violations};

/// Load the workspace, read its config, and check the workspace against it.
///
/// Without `config_path` the config is `<workspace-root>/depform.toml`; a
/// missing config there is its own error, while a missing explicit config is
/// an I/O error.
pub fn check_workspace(
    manifest_path: Option<&Path>,
    config_path: Option<&Path>,
) -> Result<Vec<Violation>, DepformError> {
    let workspace = load_workspace(manifest_path)?;
    let config = Config::load(workspace.root(), config_path)?;
    check_graph(workspace.graph(), &config)
}
