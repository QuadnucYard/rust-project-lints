//! Fixtures for the integration tests: a temporary Cargo workspace on disk.
//!
//! Each fixture owns a temporary directory holding a virtual workspace
//! manifest, one directory per member, and whatever extra files a test writes
//! (for example its `depform.toml`). The manifest lists its members
//! explicitly, so unrelated directories never become accidental members.

use std::{
    fs,
    path::{Path, PathBuf},
};

use tempfile::TempDir;

/// A temporary workspace root with members and files written on demand.
pub struct Fixture {
    dir: TempDir,
    members: Vec<String>,
}

impl Fixture {
    /// Create a workspace root with no members.
    pub fn new() -> Self {
        Self {
            dir: TempDir::new().expect("temporary directory"),
            members: Vec::new(),
        }
    }

    /// Add a member named `name` whose manifest carries `extra` TOML after the
    /// minimal package section, plus an empty `src/lib.rs`.
    pub fn member(&mut self, name: &str, extra: &str) {
        let mut manifest =
            format!("[package]\nname = {name:?}\nversion = \"0.1.0\"\nedition = \"2024\"\n");
        if !extra.is_empty() {
            manifest.push('\n');
            manifest.push_str(extra);
            if !manifest.ends_with('\n') {
                manifest.push('\n');
            }
        }
        self.write(&format!("{name}/Cargo.toml"), &manifest);
        self.write(&format!("{name}/src/lib.rs"), "");

        self.members.push(name.to_owned());
        self.members.sort();
        let mut root = String::from("[workspace]\nresolver = \"3\"\nmembers = [");
        for (index, member) in self.members.iter().enumerate() {
            if index > 0 {
                root.push_str(", ");
            }
            root.push_str(&format!("{member:?}"));
        }
        root.push_str("]\n");
        self.write("Cargo.toml", &root);
    }

    /// Write `text` to `relative`, creating parent directories as needed.
    pub fn write(&self, relative: &str, text: &str) {
        let path = self.dir.path().join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("parent directory");
        }
        fs::write(path, text).expect("fixture file");
    }

    /// The workspace root directory.
    pub fn root(&self) -> &Path {
        self.dir.path()
    }

    /// The workspace manifest path, for `--manifest-path`.
    pub fn manifest_path(&self) -> PathBuf {
        self.root().join("Cargo.toml")
    }
}
