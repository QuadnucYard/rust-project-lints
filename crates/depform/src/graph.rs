//! The workspace dependency graph.
//!
//! Nodes are the package names of workspace members, and edges are their
//! workspace-internal dependency declarations. The module stays free of
//! metadata, configuration and reporting types so the algorithms are reusable
//! and testable on hand-built graphs.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::{Display, Formatter, Result as FmtResult},
};

/// The kind of a dependency declaration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DependencyKind {
    /// A regular dependency, used by the library and the binaries.
    Normal,
    /// A dependency only used by tests, examples and benches.
    Dev,
    /// A dependency only used by build scripts.
    Build,
}

impl Display for DependencyKind {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        f.write_str(match self {
            Self::Normal => "normal",
            Self::Dev => "dev",
            Self::Build => "build",
        })
    }
}

/// A directed graph of workspace packages and their internal dependencies.
///
/// An edge `u -> v` means the package `u` declares a dependency on the package
/// `v`. Declarations that target a non-member package are not edges; the graph
/// is workspace-only.
#[derive(Debug, Clone, Default)]
pub struct Graph {
    packages: BTreeSet<String>,
    direct: BTreeMap<String, BTreeMap<String, BTreeSet<DependencyKind>>>,
    reverse: BTreeMap<String, BTreeSet<String>>,
}

impl Graph {
    /// Build a graph from package names and `(from, to, kind)` edges.
    ///
    /// Duplicate `(from, to)` edges merge their kind sets, and an edge endpoint
    /// that was not listed as a package is added to the package set.
    pub fn new(
        packages: impl IntoIterator<Item = String>,
        edges: impl IntoIterator<Item = (String, String, DependencyKind)>,
    ) -> Self {
        let mut graph = Self::with_packages(packages);
        for (from, to, kind) in edges {
            graph.insert(from.as_str(), to.as_str(), [kind]);
        }
        graph
    }

    /// Start a graph from package names and no edges.
    fn with_packages(packages: impl IntoIterator<Item = String>) -> Self {
        let mut graph = Self::default();
        graph.packages.extend(packages);
        graph
    }

    /// Record one declared dependency, keeping adjacency consistent.
    fn insert(&mut self, from: &str, to: &str, kinds: impl IntoIterator<Item = DependencyKind>) {
        self.packages.insert(from.to_owned());
        self.packages.insert(to.to_owned());
        self.direct
            .entry(from.to_owned())
            .or_default()
            .entry(to.to_owned())
            .or_default()
            .extend(kinds);
        self.reverse
            .entry(to.to_owned())
            .or_default()
            .insert(from.to_owned());
    }

    /// A graph with the same packages and only edges of the selected kinds.
    ///
    /// Each edge keeps the intersection of its kind set with `kinds`, and an
    /// edge that loses every kind disappears; packages always remain, because
    /// a package without a selected declaration is still a member.
    pub fn filtered(&self, kinds: &BTreeSet<DependencyKind>) -> Graph {
        let mut graph = Self::with_packages(self.packages.iter().cloned());
        for (from, to, edge_kinds) in self.edges() {
            if edge_kinds.is_disjoint(kinds) {
                continue;
            }
            graph.insert(from, to, edge_kinds.intersection(kinds).copied());
        }
        graph
    }

    /// The kinds of the edge `from -> to`, or `None` when the edge is absent.
    pub(crate) fn edge_kinds(&self, from: &str, to: &str) -> Option<&BTreeSet<DependencyKind>> {
        self.direct
            .get(from)
            .and_then(|neighbors| neighbors.get(to))
    }

    /// The package names, sorted.
    pub fn packages(&self) -> impl Iterator<Item = &str> {
        self.packages.iter().map(String::as_str)
    }

    /// The number of packages.
    pub fn package_count(&self) -> usize {
        self.packages.len()
    }

    /// The number of distinct `(from, to)` edges, whatever their kinds.
    pub fn edge_count(&self) -> usize {
        self.direct.values().map(BTreeMap::len).sum()
    }

    /// Every edge with its kind set, ordered by `(from, to)`.
    pub fn edges(&self) -> impl Iterator<Item = (&str, &str, &BTreeSet<DependencyKind>)> {
        self.direct.iter().flat_map(|(from, neighbors)| {
            neighbors
                .iter()
                .map(move |(to, kinds)| (from.as_str(), to.as_str(), kinds))
        })
    }

    /// Whether `package` is a node of the graph.
    pub fn contains(&self, package: &str) -> bool {
        self.packages.contains(package)
    }

    /// The direct dependencies of `package`, sorted.
    pub fn dependencies(&self, package: &str) -> impl Iterator<Item = &str> {
        self.direct
            .get(package)
            .into_iter()
            .flatten()
            .map(|(to, _)| to.as_str())
    }

    /// The direct dependents of `package`, sorted.
    pub fn dependents(&self, package: &str) -> impl Iterator<Item = &str> {
        self.reverse
            .get(package)
            .into_iter()
            .flatten()
            .map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edge(from: &str, to: &str, kind: DependencyKind) -> (String, String, DependencyKind) {
        (from.to_owned(), to.to_owned(), kind)
    }

    #[test]
    fn duplicate_edges_merge_their_kinds() {
        let graph = Graph::new(
            ["alpha".to_owned(), "beta".to_owned()],
            [
                edge("beta", "alpha", DependencyKind::Normal),
                edge("beta", "alpha", DependencyKind::Dev),
            ],
        );
        assert_eq!(graph.edge_count(), 1);
        let edges: Vec<_> = graph.edges().collect();
        assert_eq!(edges.len(), 1);
        let (from, to, kinds) = edges[0];
        assert_eq!((from, to), ("beta", "alpha"));
        assert_eq!(
            kinds.iter().copied().collect::<Vec<_>>(),
            [DependencyKind::Normal, DependencyKind::Dev]
        );
    }

    #[test]
    fn adjacency_is_sorted_and_ignores_unknown_packages() {
        let graph = Graph::new(
            ["alpha".to_owned(), "beta".to_owned(), "gamma".to_owned()],
            [
                edge("gamma", "beta", DependencyKind::Normal),
                edge("gamma", "alpha", DependencyKind::Build),
            ],
        );
        assert_eq!(
            graph.dependencies("gamma").collect::<Vec<_>>(),
            ["alpha", "beta"]
        );
        assert_eq!(graph.dependents("alpha").collect::<Vec<_>>(), ["gamma"]);
        assert_eq!(graph.dependencies("alpha").count(), 0);
        assert_eq!(graph.dependents("gamma").count(), 0);
        assert_eq!(graph.dependencies("missing").count(), 0);
        assert_eq!(graph.dependents("missing").count(), 0);
        assert!(!graph.contains("missing"));
    }

    #[test]
    fn new_adds_edge_endpoints_to_the_package_set() {
        let packages: Vec<String> = Vec::new();
        let graph = Graph::new(packages, [edge("beta", "alpha", DependencyKind::Normal)]);
        assert_eq!(graph.package_count(), 2);
        assert_eq!(graph.packages().collect::<Vec<_>>(), ["alpha", "beta"]);
    }

    #[test]
    fn filtering_keeps_packages_and_intersects_kinds() {
        let graph = Graph::new(
            ["alpha".to_owned(), "beta".to_owned(), "gamma".to_owned()],
            [
                edge("gamma", "alpha", DependencyKind::Normal),
                edge("gamma", "alpha", DependencyKind::Dev),
                edge("gamma", "beta", DependencyKind::Dev),
            ],
        );
        let normal = graph.filtered(&BTreeSet::from([DependencyKind::Normal]));
        assert_eq!(normal.package_count(), 3);
        assert_eq!(normal.edge_count(), 1);
        assert_eq!(normal.dependencies("gamma").collect::<Vec<_>>(), ["alpha"]);
        assert_eq!(
            normal.edge_kinds("gamma", "alpha"),
            Some(&BTreeSet::from([DependencyKind::Normal]))
        );

        let dev = graph.filtered(&BTreeSet::from([DependencyKind::Dev]));
        assert_eq!(dev.package_count(), 3);
        assert_eq!(dev.edge_count(), 2);
        assert_eq!(
            dev.edge_kinds("gamma", "alpha"),
            Some(&BTreeSet::from([DependencyKind::Dev]))
        );

        let build = graph.filtered(&BTreeSet::from([DependencyKind::Build]));
        assert_eq!(build.package_count(), 3);
        assert_eq!(build.edge_count(), 0);
        assert_eq!(build.dependencies("gamma").count(), 0);
        assert_eq!(build.dependents("alpha").count(), 0);
        assert!(build.edge_kinds("gamma", "alpha").is_none());
    }
}
