//! Loading a workspace through `cargo metadata`.
//!
//! The projection keeps only what the graph needs: package names of workspace
//! members and their dependency declarations. External crates never enter the
//! graph, and the metadata command runs with `--no-deps`, so the loader is
//! offline-friendly and independent of the resolved dependency tree.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use cargo_metadata::{DependencyKind as MetadataKind, MetadataCommand, Package};

use crate::{DependencyKind, DepformError, Graph};

/// A loaded Cargo workspace: its internal dependency graph and its root.
#[derive(Debug, Clone)]
pub struct Workspace {
    pub(crate) graph: Graph,
    pub(crate) root: PathBuf,
}

impl Workspace {
    /// The declared workspace-internal dependency graph.
    pub fn graph(&self) -> &Graph {
        &self.graph
    }

    /// The workspace root reported by `cargo metadata`.
    pub fn root(&self) -> &Path {
        &self.root
    }
}

/// Load the workspace whose manifest is `manifest_path`, or the workspace
/// containing the current directory.
///
/// Every workspace member becomes a node, named by its manifest `package.name`
/// with dashes preserved. A dependency declaration becomes an edge when it
/// has a path and that path is a member's manifest directory; declarations
/// that point outside the workspace are ignored, and so are metadata kinds
/// this crate does not model. All declared kinds are included regardless of
/// optionality or target specificity.
pub fn load_workspace(manifest_path: Option<&Path>) -> Result<Workspace, DepformError> {
    let mut command = MetadataCommand::new();
    command.no_deps();
    if let Some(path) = manifest_path {
        command.manifest_path(path);
    }
    let metadata = command.exec()?;

    let packages = metadata.workspace_packages();
    let names: BTreeSet<String> = packages
        .iter()
        .map(|package| package.name.as_ref().to_owned())
        .collect();
    let by_name: BTreeMap<&str, &Package> = packages
        .iter()
        .map(|package| (package.name.as_ref(), *package))
        .collect();
    let mut edges = Vec::new();
    for package in &packages {
        let from = package.name.as_ref();
        for dependency in &package.dependencies {
            let Some(kind) = kind_of(dependency.kind) else {
                continue;
            };
            let Some(target) = by_name.get(dependency.name.as_str()) else {
                continue;
            };
            let Some(path) = dependency.path.as_deref() else {
                continue;
            };
            if target.manifest_path.parent() != Some(path) {
                continue;
            }
            edges.push((from.to_owned(), dependency.name.clone(), kind));
        }
    }
    Ok(Workspace {
        graph: Graph::new(names, edges),
        root: metadata.workspace_root.as_std_path().to_path_buf(),
    })
}

/// Map a metadata dependency kind onto the kinds this crate models; the
/// catch-all metadata variant has no graph meaning.
fn kind_of(kind: MetadataKind) -> Option<DependencyKind> {
    match kind {
        MetadataKind::Normal => Some(DependencyKind::Normal),
        MetadataKind::Development => Some(DependencyKind::Dev),
        MetadataKind::Build => Some(DependencyKind::Build),
        MetadataKind::Unknown => None,
    }
}
