//! Dependency graph architecture checker for Cargo workspaces.
//!
//! The crate loads the workspace members through `cargo metadata` and projects
//! them into a package graph of workspace-internal dependency declarations.
//! The binary is a thin wrapper that maps arguments to these calls and results
//! to exit codes.

mod error;
mod graph;
mod metadata;
mod report;

pub use error::DepformError;
pub use graph::{DependencyKind, Graph};
pub use metadata::{Workspace, load_workspace};
pub use report::render_graph;
