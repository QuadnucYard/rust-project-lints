//! Public API for the layout style checker.
//!
//! [`check_tree`] is the complete engine: it reads the config at the scanned
//! root, walks the configured trees and applies every rule, returning sorted
//! violations. [`check_tree_with_config`] is the same engine reading its
//! config from a caller-selected file instead. The binary is a thin front end
//! over the engine, and another project can depend on this crate as a library
//! without inheriting any repository-specific layout: the config supplies the
//! scan roots and the root dependency aspects.
//!
//! The complete, human-maintained policy lives in this crate's `README.md`.
//! The rule modules contain only executable policy and focused tests; keeping
//! prose in one place prevents the command and documentation drifting.

mod check;
mod config;
mod diagnostic;
pub mod report;
pub mod rules;

pub use check::{CONFIG_FILE, CheckError, ROOT_MANIFEST, check_tree, check_tree_with_config};
pub use config::{ConfigError, StyleConfig};
pub use diagnostic::{ColumnSpan, ContextLine, SourceContext, Violation};
pub use report::{render_reports, write_reports};
pub use rules::{Level, Rule, RuleGroup};
