//! Whole-tree orchestration for the style checker.
//!
//! The library owns the complete check: it reads the config at the scanned
//! root, walks the configured trees, gathers the workspace facts that need the
//! manifests, and applies every rule. Nothing here knows the layout of a
//! particular repository; the config supplies the scan roots and the root
//! dependency aspects.

use std::{
    collections::BTreeSet,
    fs,
    io::{Error as IoError, ErrorKind},
    path::{Path, PathBuf},
};

use thiserror::Error;

use crate::{
    StyleConfig, Violation,
    rules::{
        WorkspaceMembers, check_manifest_with_workspace_members, check_rust_source,
        check_toml_file, external_crate_names,
    },
};

/// The style config filename at the scanned root.
pub const CONFIG_FILE: &str = "stylecheck.toml";

/// The manifest filename that marks the root of the scanned Cargo workspace.
pub const ROOT_MANIFEST: &str = "Cargo.toml";

/// A tree-walk or config failure; a rule violation is a [`Violation`], not an
/// error.
#[derive(Debug, Error)]
pub enum CheckError {
    /// A configured tree, source or manifest could not be read.
    #[error("failed to read {path}: {source}")]
    Read {
        /// The path that failed, as shown in the report.
        path: String,
        /// The underlying filesystem failure.
        #[source]
        source: IoError,
    },
    /// The style config exists but is not valid.
    #[error("failed to parse style config {path}: {detail}")]
    Config {
        /// The offending config path.
        path: String,
        /// The TOML parse or schema diagnostic.
        detail: String,
    },
}

/// Check the tree rooted at `root`.
///
/// The config at `root` (named [`CONFIG_FILE`]) supplies the scanned trees and
/// the root dependency aspects; a missing config means the defaults — scan the
/// root itself, with no declared aspects. The TOML files sitting at the root
/// are always checked. The root manifest is optional: a tree without one is
/// checked without workspace-level manifest rules.
///
/// Every violation is returned sorted by path, line and rule; an empty vector
/// means the tree passes.
pub fn check_tree(root: &Path) -> Result<Vec<Violation>, CheckError> {
    check_tree_with_config(root, None)
}

/// [`check_tree`] with the style config file selected explicitly:
/// `Some(path)` reads exactly that file — a missing one is a hard error, so a
/// mistyped override never silently widens the check to the built-in
/// defaults — while `None` keeps the optional [`CONFIG_FILE`] at the scanned
/// root.
pub fn check_tree_with_config(
    root: &Path,
    config_path: Option<&Path>,
) -> Result<Vec<Violation>, CheckError> {
    let config = match config_path {
        Some(path) => load_explicit_config(path)?,
        None => load_config(root)?,
    };
    let mut files = BTreeSet::new();
    for scan_path in config.scan_paths() {
        collect_files(&root.join(scan_path), &mut files)?;
    }
    collect_root_toml(root, &mut files)?;

    let mut sources = Vec::with_capacity(files.len());
    for path in files {
        let text = fs::read_to_string(&path).map_err(|source| CheckError::Read {
            path: path.display().to_string(),
            source,
        })?;
        sources.push((path, text));
    }

    let root_manifest = root.join(ROOT_MANIFEST);
    let root_text = sources
        .iter()
        .find(|(path, _)| *path == root_manifest)
        .map_or("", |(_, text)| text.as_str());
    let relative: Vec<String> = sources
        .iter()
        .map(|(path, _)| relative_path(root, path))
        .collect();
    let member_manifests: Vec<(&str, &str)> = sources
        .iter()
        .zip(&relative)
        .filter(|((path, _), _)| path != &root_manifest && is_cargo_manifest(path))
        .map(|((_, text), relative)| (relative.as_str(), text.as_str()))
        .collect();

    let workspace_members = WorkspaceMembers::from_root_manifest(root_text, &config)
        .with_member_manifests(&member_manifests);
    let external = external_crate_names(root_text, &member_manifests, &workspace_members);
    let config = config.with_external_crates(external);

    let mut found = Vec::new();
    for ((path, text), relative) in sources.iter().zip(&relative) {
        if path.extension().is_some_and(|extension| extension == "rs") {
            found.extend(check_rust_source(relative, text, &config));
        } else if is_cargo_manifest(path) {
            found.extend(check_manifest_with_workspace_members(
                relative,
                text,
                relative == ROOT_MANIFEST,
                &workspace_members,
            ));
        } else {
            found.extend(check_toml_file(relative, text));
        }
    }
    found.sort();
    Ok(found)
}

/// Load the optional style config at the scanned root. A missing file applies
/// the built-in defaults; an unreadable or malformed one is a hard error.
fn load_config(root: &Path) -> Result<StyleConfig, CheckError> {
    let path = root.join(CONFIG_FILE);
    match fs::read_to_string(&path) {
        Ok(text) => StyleConfig::from_toml(&text).map_err(|error| CheckError::Config {
            path: path.display().to_string(),
            detail: error.to_string(),
        }),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(StyleConfig::default()),
        Err(source) => Err(CheckError::Read {
            path: path.display().to_string(),
            source,
        }),
    }
}

/// Load a style config the caller selected by path. Unlike the root default,
/// a missing file is a hard error: an explicitly selected config must not
/// silently fall back to the built-in defaults.
fn load_explicit_config(path: &Path) -> Result<StyleConfig, CheckError> {
    let text = fs::read_to_string(path).map_err(|source| CheckError::Read {
        path: path.display().to_string(),
        source,
    })?;
    StyleConfig::from_toml(&text).map_err(|error| CheckError::Config {
        path: path.display().to_string(),
        detail: error.to_string(),
    })
}

/// Whether a directory is outside the walk: Cargo's build directory and hidden
/// tool state (VCS metadata, editor state) are never authored sources.
fn is_skipped_dir(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name == "target" || name.starts_with('.'))
}

/// Collect every Rust source and TOML file below a directory. The TOML set is
/// the manifests plus the authored data files, so one walk covers both the
/// manifest rules and the data-file line rules.
fn collect_files(dir: &Path, files: &mut BTreeSet<PathBuf>) -> Result<(), CheckError> {
    for entry in fs::read_dir(dir).map_err(|source| CheckError::Read {
        path: dir.display().to_string(),
        source,
    })? {
        let entry = entry.map_err(|source| CheckError::Read {
            path: dir.display().to_string(),
            source,
        })?;
        let path = entry.path();
        if path.is_dir() {
            if !is_skipped_dir(&path) {
                collect_files(&path, files)?;
            }
        } else if path
            .extension()
            .is_some_and(|extension| extension == "rs" || extension == "toml")
        {
            files.insert(path);
        }
    }
    Ok(())
}

/// Collect the TOML files sitting at the scanned root (the root manifest and
/// the tool configuration) without descending into the walked trees.
fn collect_root_toml(root: &Path, files: &mut BTreeSet<PathBuf>) -> Result<(), CheckError> {
    for entry in fs::read_dir(root).map_err(|source| CheckError::Read {
        path: root.display().to_string(),
        source,
    })? {
        let entry = entry.map_err(|source| CheckError::Read {
            path: root.display().to_string(),
            source,
        })?;
        let path = entry.path();
        if path.is_file()
            && path
                .extension()
                .is_some_and(|extension| extension == "toml")
        {
            files.insert(path);
        }
    }
    Ok(())
}

/// Whether a path is a Cargo manifest.
fn is_cargo_manifest(path: &Path) -> bool {
    path.file_name().is_some_and(|name| name == ROOT_MANIFEST)
}

/// The path relative to the scanned root, with forward slashes.
fn relative_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Rule;

    /// Write one file, creating its parent directories.
    fn write(root: &Path, relative: &str, text: &str) {
        let path = root.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create the parent directory");
        }
        fs::write(path, text).expect("write the fixture file");
    }

    #[test]
    fn walk_uses_configured_scan_paths_and_aspects() {
        let dir = tempfile::tempdir().expect("temp dir");
        let root = dir.path();
        write(
            root,
            "stylecheck.toml",
            r#"scan-paths = ["crates"]

[[dependency-group-order.aspects]]
name = "foundation"
paths = ["crates/foundation/"]

[[dependency-group-order.aspects]]
name = "game"
paths = ["crates/game/"]
"#,
        );
        write(
            root,
            "Cargo.toml",
            "[workspace]\nresolver = \"3\"\n\n[workspace.dependencies]\ngame-a.path = \"crates/game/a\"\n\nfoundation-a.path = \"crates/foundation/a\"\n",
        );
        write(
            root,
            "crates/game/a/Cargo.toml",
            "[package]\nname = \"game-a\"\n",
        );
        write(
            root,
            "crates/foundation/a/Cargo.toml",
            "[package]\nname = \"foundation-a\"\n",
        );
        write(
            root,
            "crates/game/a/src/lib.rs",
            "use std::fmt;\n\nmod parser;\n",
        );
        // Outside every configured scan path, so its violation must not appear.
        write(
            root,
            "elsewhere/src/lib.rs",
            "use std::fmt;\n\nmod parser;\n",
        );

        let found = check_tree(root).expect("the tree checks");
        assert!(
            found
                .iter()
                .any(|violation| violation.rule == Rule::DependencyGroupOrder
                    && violation.path == "Cargo.toml"),
            "{found:?}"
        );
        assert!(
            found
                .iter()
                .any(|violation| violation.rule == Rule::ModBeforeUse
                    && violation.path == "crates/game/a/src/lib.rs"),
            "{found:?}"
        );
        assert!(
            !found
                .iter()
                .any(|violation| violation.path.starts_with("elsewhere/")),
            "{found:?}"
        );
    }

    #[test]
    fn default_config_scans_the_root_and_needs_no_manifest() {
        let dir = tempfile::tempdir().expect("temp dir");
        let root = dir.path();
        write(root, "notes/data.toml", "value = 1\r\n");

        let found = check_tree(root).expect("the tree checks");
        assert!(
            found
                .iter()
                .any(|violation| violation.rule == Rule::TomlLineEnding
                    && violation.path == "notes/data.toml"),
            "{found:?}"
        );
    }

    #[test]
    fn an_explicit_config_file_replaces_the_root_one() {
        let dir = tempfile::tempdir().expect("temp dir");
        let root = dir.path();
        // The root config narrows the walk to `narrowed/`; the explicitly
        // selected one selects `elsewhere/` instead, so the selected file
        // must win and the root config only names extra scan trees.
        write(root, "stylecheck.toml", "scan-paths = [\"narrowed\"]\n");
        write(root, "custom.toml", "scan-paths = [\"elsewhere\"]\n");
        let violating = "use std::fmt;\n\nmod parser;\n";
        write(root, "elsewhere/src/lib.rs", violating);
        write(root, "narrowed/src/lib.rs", violating);

        let found =
            check_tree_with_config(root, Some(&root.join("custom.toml"))).expect("the tree checks");
        assert!(
            found
                .iter()
                .any(|violation| violation.rule == Rule::ModBeforeUse
                    && violation.path == "elsewhere/src/lib.rs"),
            "{found:?}"
        );
        assert!(
            found
                .iter()
                .all(|violation| !violation.path.starts_with("narrowed/")),
            "{found:?}"
        );
    }

    #[test]
    fn an_explicit_config_file_must_exist() {
        let dir = tempfile::tempdir().expect("temp dir");
        let error = check_tree_with_config(dir.path(), Some(Path::new("missing.toml")))
            .expect_err("the missing config is a hard error");
        assert!(matches!(error, CheckError::Read { .. }), "{error:?}");
    }

    #[test]
    fn walk_skips_target_and_hidden_directories() {
        let dir = tempfile::tempdir().expect("temp dir");
        let root = dir.path();
        let source = "use std::fmt;\n\nmod parser;\n";
        write(root, "crates/a/src/lib.rs", source);
        // Build output and hidden tool state sit next to the authored sources
        // but are never part of the checked set.
        write(root, "crates/a/target/debug/skipped.rs", source);
        write(root, "crates/a/.git/hooks/skipped.rs", source);

        let found = check_tree(root).expect("the tree checks");
        assert!(
            found
                .iter()
                .any(|violation| violation.rule == Rule::ModBeforeUse
                    && violation.path == "crates/a/src/lib.rs"),
            "{found:?}"
        );
        assert!(
            found
                .iter()
                .all(|violation| !violation.path.contains("target/")
                    && !violation.path.contains(".git/")),
            "{found:?}"
        );
    }
}
