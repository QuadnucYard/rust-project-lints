//! The workspace dependency graph.
//!
//! Nodes are the package names of workspace members, and edges are their
//! workspace-internal dependency declarations. The module stays free of
//! metadata, configuration and reporting types so the algorithms are reusable
//! and testable on hand-built graphs. Every traversal walks adjacency in
//! sorted order, which keeps witness paths and diagnostics deterministic.

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
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

    /// The packages reachable from `package` by one or more edges, sorted.
    ///
    /// The start is included only when a cycle leads back to it.
    pub fn reachable(&self, package: &str) -> BTreeSet<String> {
        let mut seen = BTreeSet::new();
        let mut queue: Vec<String> = self.dependencies(package).map(str::to_owned).collect();
        while let Some(node) = queue.pop() {
            if !seen.insert(node.clone()) {
                continue;
            }
            queue.extend(self.dependencies(&node).map(str::to_owned));
        }
        seen
    }

    /// A shortest path from `from` to `to`, endpoints included.
    ///
    /// A package in the graph has a one-element path to itself, while a
    /// package outside it has none. Neighbors are visited in sorted order, so
    /// the returned path is deterministic.
    pub fn path(&self, from: &str, to: &str) -> Option<Vec<String>> {
        if from == to {
            return self.contains(from).then(|| vec![from.to_owned()]);
        }
        self.search(from, to, None)
    }

    /// A shortest cycle through `package`, with the package repeated at both
    /// ends.
    ///
    /// `None` means no path leads from the package back to itself.
    pub fn cycle_path(&self, package: &str) -> Option<Vec<String>> {
        self.search(package, package, None)
    }

    /// The shortest directed path length from `from` to `to`.
    ///
    /// The distance from a package to itself is zero; an unreachable target —
    /// including a package outside the graph — has no distance.
    pub fn distance(&self, from: &str, to: &str) -> Option<usize> {
        self.path(from, to).map(|path| path.len() - 1)
    }

    /// The shortest distance from any root to every package, roots included.
    ///
    /// A package no root reaches has no depth. Roots and neighbors are visited
    /// in sorted order, so the first parent the search records — and with it
    /// every witness path — is deterministic.
    pub fn depths(&self, roots: &[String]) -> BTreeMap<String, Option<usize>> {
        self.root_search(roots).depths
    }

    /// The witness path from a closest root to `package`, root first.
    pub fn depth_path(&self, roots: &[String], package: &str) -> Option<Vec<String>> {
        self.root_search(roots).path(package)
    }

    /// Multi-source breadth-first search from `roots`.
    pub(crate) fn root_search(&self, roots: &[String]) -> RootSearch {
        let mut depths: BTreeMap<String, Option<usize>> = self
            .packages
            .iter()
            .map(|package| (package.clone(), None))
            .collect();
        let mut parents = BTreeMap::new();
        let mut queue = VecDeque::new();
        let mut sorted: Vec<&String> = roots.iter().collect();
        sorted.sort();
        for root in sorted {
            if let Some(slot) = depths.get_mut(root)
                && slot.is_none()
            {
                *slot = Some(0);
                parents.insert(root.clone(), None);
                queue.push_back(root.clone());
            }
        }
        while let Some(node) = queue.pop_front() {
            let Some(depth) = depths.get(&node).copied().flatten() else {
                continue;
            };
            for package in self.dependencies(&node) {
                if let Some(slot) = depths.get_mut(package)
                    && slot.is_none()
                {
                    *slot = Some(depth + 1);
                    parents.insert(package.to_owned(), Some(node.clone()));
                    queue.push_back(package.to_owned());
                }
            }
        }
        RootSearch { depths, parents }
    }

    /// The longest downstream path length from `package`, with a witness path.
    ///
    /// A package with no dependencies has height 0 and a one-element path.
    /// Ties keep the first longest branch in sorted neighbor order. A cycle in
    /// the downstream cone leaves the height undefined, reported as an error
    /// that spells the cycle.
    pub fn height(&self, package: &str) -> Result<(usize, Vec<String>), String> {
        if !self.contains(package) {
            return Err(format!("package `{package}` is not in the graph"));
        }
        let mut memo = BTreeMap::new();
        let mut stack = Vec::new();
        self.height_with(package, &mut memo, &mut stack)
    }

    /// Memoized depth-first height search along one DFS stack.
    fn height_with(
        &self,
        package: &str,
        memo: &mut BTreeMap<String, (usize, Vec<String>)>,
        stack: &mut Vec<String>,
    ) -> Result<(usize, Vec<String>), String> {
        if let Some(known) = memo.get(package) {
            return Ok(known.clone());
        }
        stack.push(package.to_owned());
        let mut best: Option<(usize, Vec<String>)> = None;
        for next in self.dependencies(package) {
            if let Some(position) = stack.iter().position(|entry| entry == next) {
                let mut cycle = stack[position..].to_vec();
                cycle.push(next.to_owned());
                return Err(format!("dependency cycle {}", cycle.join(" -> ")));
            }
            let (height, path) = self.height_with(next, memo, stack)?;
            let height = height + 1;
            if best.as_ref().is_none_or(|(longest, _)| height > *longest) {
                let mut witness = Vec::with_capacity(path.len() + 1);
                witness.push(package.to_owned());
                witness.extend(path);
                best = Some((height, witness));
            }
        }
        stack.pop();
        let result = best.unwrap_or_else(|| (0, vec![package.to_owned()]));
        memo.insert(package.to_owned(), result.clone());
        Ok(result)
    }

    /// A shortest path from `from` to `to` that avoids every gateway package.
    ///
    /// Gateways are skipped as intermediates, so a path exists here only when
    /// it reaches `to` without passing through one. The endpoints are never
    /// intermediates: a gateway equal to `from` or `to` does not block. `None`
    /// means every path crosses a gateway, or `to` is unreachable.
    pub fn bypass_path(
        &self,
        from: &str,
        to: &str,
        gateways: &BTreeSet<String>,
    ) -> Option<Vec<String>> {
        if from == to {
            return None;
        }
        self.search(from, to, Some(gateways))
    }

    /// Breadth-first search for `to`, skipping gateways as intermediates.
    ///
    /// The start is seeded as visited, so a search towards the start itself
    /// only succeeds by leaving it and returning, which is exactly a cycle.
    fn search(
        &self,
        from: &str,
        to: &str,
        gateways: Option<&BTreeSet<String>>,
    ) -> Option<Vec<String>> {
        let mut parents: BTreeMap<String, Option<String>> =
            BTreeMap::from([(from.to_owned(), None)]);
        let mut queue = VecDeque::from([from.to_owned()]);
        while let Some(node) = queue.pop_front() {
            for next in self.dependencies(&node) {
                if next != to && gateways.is_some_and(|gateways| gateways.contains(next)) {
                    continue;
                }
                if next == to {
                    let mut path = trail(&parents, &node);
                    path.push(to.to_owned());
                    return Some(path);
                }
                if parents.contains_key(next) {
                    continue;
                }
                parents.insert(next.to_owned(), Some(node.clone()));
                queue.push_back(next.to_owned());
            }
        }
        None
    }
}

/// The depths and parent links of one multi-source breadth-first search.
pub(crate) struct RootSearch {
    pub(crate) depths: BTreeMap<String, Option<usize>>,
    parents: BTreeMap<String, Option<String>>,
}

impl RootSearch {
    /// The depth of `package`, if a root reaches it.
    pub(crate) fn depth(&self, package: &str) -> Option<usize> {
        self.depths.get(package).copied().flatten()
    }

    /// The witness path from the closest root to `package`, root first.
    pub(crate) fn path(&self, package: &str) -> Option<Vec<String>> {
        self.depth(package)?;
        Some(trail(&self.parents, package))
    }
}

/// Walk parent links from `to` back to the start and return the path.
fn trail(parents: &BTreeMap<String, Option<String>>, to: &str) -> Vec<String> {
    let mut path = Vec::new();
    let mut current = Some(to.to_owned());
    while let Some(node) = current {
        current = parents.get(&node).cloned().flatten();
        path.push(node);
    }
    path.reverse();
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edge(from: &str, to: &str, kind: DependencyKind) -> (String, String, DependencyKind) {
        (from.to_owned(), to.to_owned(), kind)
    }

    fn names(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
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

    /// `a -> b -> c` plus `a -> c` and `d` with no edges.
    fn chain() -> Graph {
        Graph::new(
            [
                "a".to_owned(),
                "b".to_owned(),
                "c".to_owned(),
                "d".to_owned(),
            ],
            [
                edge("a", "b", DependencyKind::Normal),
                edge("b", "c", DependencyKind::Normal),
                edge("a", "c", DependencyKind::Normal),
            ],
        )
    }

    #[test]
    fn reachable_follows_one_or_more_edges() {
        let graph = chain();
        assert_eq!(
            graph.reachable("a"),
            BTreeSet::from(["b".to_owned(), "c".to_owned()])
        );
        assert_eq!(graph.reachable("c"), BTreeSet::new());
        assert_eq!(graph.reachable("d"), BTreeSet::new());
        assert_eq!(graph.reachable("missing"), BTreeSet::new());
    }

    #[test]
    fn a_cycle_makes_the_start_reachable_from_itself() {
        let graph = Graph::new(
            ["a".to_owned(), "b".to_owned()],
            [
                edge("a", "b", DependencyKind::Normal),
                edge("b", "a", DependencyKind::Normal),
            ],
        );
        assert_eq!(
            graph.reachable("a"),
            BTreeSet::from(["a".to_owned(), "b".to_owned()])
        );
        let self_loop = Graph::new(["a".to_owned()], [edge("a", "a", DependencyKind::Normal)]);
        assert_eq!(self_loop.reachable("a"), BTreeSet::from(["a".to_owned()]));
    }

    #[test]
    fn paths_are_the_shortest_and_include_their_endpoints() {
        let graph = chain();
        assert_eq!(
            graph.path("a", "c"),
            Some(vec!["a".to_owned(), "c".to_owned()])
        );
        assert_eq!(graph.distance("a", "c"), Some(1));
        let deep = Graph::new(
            ["a".to_owned(), "b".to_owned(), "c".to_owned()],
            [
                edge("a", "b", DependencyKind::Normal),
                edge("b", "c", DependencyKind::Normal),
            ],
        );
        assert_eq!(
            deep.path("a", "c"),
            Some(vec!["a".to_owned(), "b".to_owned(), "c".to_owned()])
        );
        assert_eq!(deep.distance("a", "c"), Some(2));
        assert_eq!(deep.path("c", "a"), None);
        assert_eq!(deep.path("a", "missing"), None);
        assert_eq!(deep.path("a", "a"), Some(vec!["a".to_owned()]));
        assert_eq!(deep.distance("a", "a"), Some(0));
        assert_eq!(deep.path("missing", "missing"), None);
        assert_eq!(deep.distance("missing", "missing"), None);
    }

    #[test]
    fn cycles_have_a_closed_witness_path() {
        let graph = Graph::new(
            ["a".to_owned(), "b".to_owned(), "c".to_owned()],
            [
                edge("a", "b", DependencyKind::Normal),
                edge("b", "a", DependencyKind::Normal),
                edge("b", "c", DependencyKind::Normal),
                edge("c", "b", DependencyKind::Normal),
            ],
        );
        assert_eq!(graph.cycle_path("a"), Some(names(&["a", "b", "a"])));
        assert_eq!(graph.cycle_path("b"), Some(names(&["b", "a", "b"])));
        assert_eq!(graph.cycle_path("missing"), None);

        let acyclic = layered();
        assert_eq!(acyclic.cycle_path("core"), None);
    }

    #[test]
    fn sorted_neighbors_choose_the_witness_path() {
        let graph = Graph::new(
            [
                "a".to_owned(),
                "b".to_owned(),
                "c".to_owned(),
                "d".to_owned(),
            ],
            [
                edge("a", "d", DependencyKind::Normal),
                edge("a", "b", DependencyKind::Normal),
                edge("b", "c", DependencyKind::Normal),
                edge("d", "c", DependencyKind::Normal),
            ],
        );
        assert_eq!(
            graph.path("a", "c"),
            Some(vec!["a".to_owned(), "b".to_owned(), "c".to_owned()])
        );
    }

    /// `app -> facade -> core` plus an unconnected `tool`.
    fn layered() -> Graph {
        Graph::new(
            [
                "app".to_owned(),
                "facade".to_owned(),
                "core".to_owned(),
                "tool".to_owned(),
            ],
            [
                edge("app", "facade", DependencyKind::Normal),
                edge("facade", "core", DependencyKind::Normal),
            ],
        )
    }

    #[test]
    fn depths_measure_the_shortest_distance_from_any_root() {
        let graph = layered();
        let roots = ["app".to_owned()];
        let depths = graph.depths(&roots);
        assert_eq!(depths.len(), 4);
        assert_eq!(depths["app"], Some(0));
        assert_eq!(depths["facade"], Some(1));
        assert_eq!(depths["core"], Some(2));
        assert_eq!(depths["tool"], None);
        assert_eq!(
            graph.depth_path(&roots, "core"),
            Some(vec![
                "app".to_owned(),
                "facade".to_owned(),
                "core".to_owned(),
            ])
        );
        assert_eq!(
            graph.depth_path(&roots, "app"),
            Some(vec!["app".to_owned()])
        );
        assert_eq!(graph.depth_path(&roots, "tool"), None);
        assert_eq!(graph.depth_path(&roots, "missing"), None);
    }

    #[test]
    fn several_roots_share_one_search() {
        let graph = Graph::new(
            ["a".to_owned(), "b".to_owned(), "c".to_owned()],
            [
                edge("a", "c", DependencyKind::Normal),
                edge("b", "c", DependencyKind::Normal),
            ],
        );
        let depths = graph.depths(&["b".to_owned(), "a".to_owned()]);
        assert_eq!(depths["a"], Some(0));
        assert_eq!(depths["b"], Some(0));
        assert_eq!(depths["c"], Some(1));
        assert_eq!(
            graph.depth_path(&["b".to_owned(), "a".to_owned()], "c"),
            Some(vec!["a".to_owned(), "c".to_owned()])
        );
    }

    #[test]
    fn roots_outside_the_graph_contribute_nothing() {
        let graph = layered();
        let depths = graph.depths(&["missing".to_owned()]);
        assert!(depths.values().all(Option::is_none));
    }

    #[test]
    fn the_first_sorted_parent_wins_a_tie() {
        let graph = Graph::new(
            [
                "a".to_owned(),
                "b".to_owned(),
                "c".to_owned(),
                "d".to_owned(),
            ],
            [
                edge("a", "c", DependencyKind::Normal),
                edge("a", "b", DependencyKind::Normal),
                edge("b", "d", DependencyKind::Normal),
                edge("c", "d", DependencyKind::Normal),
            ],
        );
        let roots = ["a".to_owned()];
        assert_eq!(graph.depths(&roots)["d"], Some(2));
        assert_eq!(
            graph.depth_path(&roots, "d"),
            Some(vec!["a".to_owned(), "b".to_owned(), "d".to_owned()])
        );
    }

    #[test]
    fn height_is_the_longest_downstream_path() {
        let graph = layered();
        assert_eq!(graph.height("facade"), Ok((1, names(&["facade", "core"]))));
        assert_eq!(
            graph.height("app"),
            Ok((2, names(&["app", "facade", "core"])))
        );
        assert_eq!(graph.height("core"), Ok((0, names(&["core"]))));
    }

    #[test]
    fn the_longest_branch_wins_and_ties_keep_the_sorted_one() {
        let graph = Graph::new(
            [
                "app".to_owned(),
                "short".to_owned(),
                "deep".to_owned(),
                "leaf".to_owned(),
            ],
            [
                edge("app", "short", DependencyKind::Normal),
                edge("app", "deep", DependencyKind::Normal),
                edge("deep", "leaf", DependencyKind::Normal),
            ],
        );
        assert_eq!(
            graph.height("app"),
            Ok((2, names(&["app", "deep", "leaf"])))
        );

        let tied = Graph::new(
            ["app".to_owned(), "beta".to_owned(), "alpha".to_owned()],
            [
                edge("app", "beta", DependencyKind::Normal),
                edge("app", "alpha", DependencyKind::Normal),
            ],
        );
        assert_eq!(tied.height("app"), Ok((1, names(&["app", "alpha"]))));
    }

    #[test]
    fn a_cycle_in_the_cone_makes_height_undefined() {
        let graph = Graph::new(
            ["a".to_owned(), "b".to_owned(), "c".to_owned()],
            [
                edge("a", "b", DependencyKind::Normal),
                edge("b", "a", DependencyKind::Normal),
            ],
        );
        let error = graph.height("a").expect_err("a cycle");
        assert_eq!(error, "dependency cycle a -> b -> a");

        let self_loop = Graph::new(["a".to_owned()], [edge("a", "a", DependencyKind::Normal)]);
        assert_eq!(
            self_loop.height("a").expect_err("a cycle"),
            "dependency cycle a -> a"
        );
    }

    #[test]
    fn a_cycle_outside_the_cone_is_irrelevant() {
        let graph = Graph::new(
            [
                "a".to_owned(),
                "b".to_owned(),
                "c".to_owned(),
                "d".to_owned(),
            ],
            [
                edge("a", "b", DependencyKind::Normal),
                edge("c", "d", DependencyKind::Normal),
                edge("d", "c", DependencyKind::Normal),
            ],
        );
        assert_eq!(graph.height("a"), Ok((1, names(&["a", "b"]))));
        assert!(graph.height("c").is_err());
    }

    #[test]
    fn height_rejects_unknown_packages() {
        let graph = layered();
        assert_eq!(
            graph.height("missing").expect_err("an unknown package"),
            "package `missing` is not in the graph"
        );
    }

    #[test]
    fn bypass_paths_avoid_every_gateway() {
        // `app -> facade -> core` and a direct `app -> core`.
        let graph = Graph::new(
            ["app".to_owned(), "facade".to_owned(), "core".to_owned()],
            [
                edge("app", "facade", DependencyKind::Normal),
                edge("app", "core", DependencyKind::Normal),
                edge("facade", "core", DependencyKind::Normal),
            ],
        );
        let gateways = BTreeSet::from(["facade".to_owned()]);
        assert_eq!(
            graph.bypass_path("app", "core", &gateways),
            Some(names(&["app", "core"]))
        );

        let gated = Graph::new(
            ["app".to_owned(), "facade".to_owned(), "core".to_owned()],
            [
                edge("app", "facade", DependencyKind::Normal),
                edge("facade", "core", DependencyKind::Normal),
            ],
        );
        assert_eq!(gated.bypass_path("app", "core", &gateways), None);
        assert_eq!(gated.bypass_path("core", "app", &gateways), None);
        assert_eq!(gated.bypass_path("app", "app", &gateways), None);
    }

    #[test]
    fn a_gateway_at_an_endpoint_does_not_block() {
        let graph = Graph::new(
            ["app".to_owned(), "core".to_owned()],
            [edge("app", "core", DependencyKind::Normal)],
        );
        let source = BTreeSet::from(["app".to_owned()]);
        assert_eq!(
            graph.bypass_path("app", "core", &source),
            Some(names(&["app", "core"]))
        );
        let target = BTreeSet::from(["core".to_owned()]);
        assert_eq!(
            graph.bypass_path("app", "core", &target),
            Some(names(&["app", "core"]))
        );
    }

    #[test]
    fn bypass_search_terminates_on_cycles() {
        let graph = Graph::new(
            ["a".to_owned(), "b".to_owned(), "c".to_owned()],
            [
                edge("a", "b", DependencyKind::Normal),
                edge("b", "a", DependencyKind::Normal),
                edge("b", "c", DependencyKind::Normal),
            ],
        );
        let gateways = BTreeSet::new();
        assert_eq!(
            graph.bypass_path("a", "c", &gateways),
            Some(names(&["a", "b", "c"]))
        );
    }
}
