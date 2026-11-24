//! Errors reported by the checker and the library.
//!
//! One variant per failure mode: a broken `cargo metadata` invocation, an
//! unreadable or missing config file, a config that violates the schema, and a
//! dependency graph that cannot answer a query (for example a cycle where a
//! height was requested).

use std::{io::Error as IoError, path::PathBuf};

use thiserror::Error;

/// Everything that can go wrong while loading and checking a workspace.
#[derive(Debug, Error)]
pub enum DepformError {
    /// `cargo metadata` failed; the underlying error explains why.
    #[error(transparent)]
    Metadata(#[from] cargo_metadata::Error),
    /// A config file that exists could not be read.
    #[error("failed to read `{path}`: {source}")]
    ReadConfig {
        /// The config file that was read.
        path: PathBuf,
        /// The I/O failure.
        source: IoError,
    },
    /// No config file sits at the default location in the workspace root.
    #[error("no `depform.toml` at `{path}`; create one or pass `--config`")]
    MissingConfig {
        /// The path that was looked at.
        path: PathBuf,
    },
    /// The config cannot be used: it fails to parse as TOML, or it parses but
    /// violates the schema.
    #[error("invalid depform.toml: {0}")]
    Config(String),
    /// The dependency graph cannot answer the requested query.
    #[error("invalid dependency graph: {0}")]
    Graph(String),
}
