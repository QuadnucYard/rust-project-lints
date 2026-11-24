//! Rule evaluation against a filtered graph.
//!
//! [`check_graph`] first validates every explicit selector of every rule, then
//! evaluates the rules in name order. Validation happens up front so a config
//! error never hides behind a violation, and evaluation happens in contract
//! order — rule, then package, then check — so the diagnostics are stable.
//!
//! The module performs no I/O: the caller loads the workspace and its config.

#[cfg(test)]
mod tests;

use std::collections::BTreeSet;

use crate::{
    Config, DepformError, Graph,
    config::{NodeRule, PairRule, Range, RuleConfig, RuleKind, Selector, SetCheck, resolve_pairs},
    graph::RootSearch,
    report::{Violation, render_kinds},
};

/// Check an already loaded graph against a config.
///
/// The checked view is the graph filtered to the config's dependency kinds;
/// `graph` itself is left untouched.
pub fn check_graph(graph: &Graph, config: &Config) -> Result<Vec<Violation>, DepformError> {
    let view = graph.filtered(config.kinds());
    for rule in config.rules() {
        rule.validate(&view)?;
    }
    let needs_roots = config.rules().iter().any(RuleConfig::needs_roots);
    let roots = resolve_roots(&view, config, needs_roots)?;
    let depth_view = roots.map(|roots| DepthView {
        search: view.root_search(&roots),
    });
    let mut violations = Vec::new();
    for rule in config.rules() {
        match &rule.kind {
            RuleKind::Node(node) => {
                check_node(
                    &view,
                    depth_view.as_ref(),
                    &rule.name,
                    node,
                    &mut violations,
                )?;
            }
            RuleKind::Pair(pair) => check_pair(&view, &rule.name, pair, &mut violations),
        }
    }
    Ok(violations)
}

/// Evaluate every node check of one rule against its selected packages.
fn check_node(
    view: &Graph,
    depth_view: Option<&DepthView>,
    rule: &str,
    node: &NodeRule,
    violations: &mut Vec<Violation>,
) -> Result<(), DepformError> {
    let selection = node.select.to_string();
    let dependencies = node
        .dependencies
        .as_ref()
        .map(|check| ResolvedSet::new(check, view));
    let reachable = node
        .reachable
        .as_ref()
        .map(|check| ResolvedSet::new(check, view));
    for package in node.select.resolve(view) {
        let report = NodeReport {
            rule,
            selection: &selection,
            package: &package,
        };
        if let Some(sets) = &dependencies {
            check_sets(view, SetKind::Dependencies, sets, &report, violations);
        }
        if let Some(sets) = &reachable {
            check_sets(view, SetKind::Reachable, sets, &report, violations);
        }
        if let (Some(range), Some(depth_view)) = (&node.depth, depth_view) {
            check_depth(depth_view, range, &report, violations);
        }
        if let Some(range) = &node.height {
            check_height(view, range, &report, violations)?;
        }
        if let Some(range) = &node.dependency_count {
            check_count(view, CountKind::Dependencies, range, &report, violations);
        }
        if let Some(range) = &node.dependent_count {
            check_count(view, CountKind::Dependents, range, &report, violations);
        }
    }
    Ok(())
}

/// Evaluate every pair check of one rule against its selected pairs.
fn check_pair(view: &Graph, rule: &str, pair: &PairRule, violations: &mut Vec<Violation>) {
    let selection = format!("{{ from = {}, to = {} }}", pair.from, pair.to);
    for (from, to) in resolve_pairs(&pair.from, &pair.to, view) {
        let report = PairReport {
            rule,
            selection: &selection,
            from: &from,
            to: &to,
        };
        if let Some(range) = &pair.distance {
            check_distance(view, range, &report, violations);
        }
    }
}

/// The roots depth is measured from: the declared ones, or inferred.
///
/// Declared roots are validated against the view whether or not a rule needs
/// them. Inference only runs when a rule checks `depth`; an inferred set that
/// is empty means every package has a dependent, which no depth can describe.
fn resolve_roots(
    view: &Graph,
    config: &Config,
    needed: bool,
) -> Result<Option<Vec<String>>, DepformError> {
    if let Some(roots) = config.roots() {
        for root in roots {
            if !view.contains(root) {
                return Err(DepformError::Config(format!(
                    "[graph] roots names `{root}`, which is not a package in the graph"
                )));
            }
        }
        return Ok(Some(roots.to_vec()));
    }
    if !needed {
        return Ok(None);
    }
    let inferred: Vec<String> = view
        .packages()
        .filter(|package| view.dependents(package).next().is_none())
        .map(str::to_owned)
        .collect();
    if inferred.is_empty() {
        return Err(DepformError::Graph(
            "no package is free of dependents, so no root can be inferred; \
             declare `[graph] roots`"
                .to_owned(),
        ));
    }
    Ok(Some(inferred))
}

/// The root search the depth check reports from.
struct DepthView {
    search: RootSearch,
}

/// One evaluated package, with the rule that reports on it.
struct NodeReport<'a> {
    rule: &'a str,
    selection: &'a str,
    package: &'a str,
}

impl NodeReport<'_> {
    /// Push one violation whose details follow the node-check contract order.
    fn fail(
        &self,
        message: impl Into<String>,
        observed: impl Into<String>,
        expected: impl Into<String>,
        witness: Option<String>,
        violations: &mut Vec<Violation>,
    ) {
        let mut details = vec![
            detail("selection", self.selection),
            detail("package", self.package),
            detail("observed", observed),
            detail("expected", expected),
        ];
        if let Some(witness) = witness {
            details.push(detail("witness", witness));
        }
        violations.push(Violation::new(self.rule, message, details));
    }
}

/// One evaluated pair, with the rule that reports on it.
struct PairReport<'a> {
    rule: &'a str,
    selection: &'a str,
    from: &'a str,
    to: &'a str,
}

impl PairReport<'_> {
    /// Push one violation whose details follow the pair-check contract order.
    fn fail(
        &self,
        message: impl Into<String>,
        observed: impl Into<String>,
        expected: impl Into<String>,
        witness: Option<String>,
        violations: &mut Vec<Violation>,
    ) {
        let mut details = vec![
            detail("selection", self.selection),
            detail("from", self.from),
            detail("to", self.to),
            detail("observed", observed),
            detail("expected", expected),
        ];
        if let Some(witness) = witness {
            details.push(detail("witness", witness));
        }
        violations.push(Violation::new(self.rule, message, details));
    }
}

/// Evaluate the `depth` range for one package.
fn check_depth(
    depth_view: &DepthView,
    range: &Range,
    node: &NodeReport,
    violations: &mut Vec<Violation>,
) {
    let value = depth_view.search.depth(node.package);
    if range.check(value) {
        return;
    }
    let witness = depth_view.search.path(node.package).map(|path| {
        path.iter()
            .enumerate()
            .map(|(index, name)| format!("{name} ({index})"))
            .collect::<Vec<_>>()
            .join(" -> ")
    });
    node.fail(
        format!("depth of package `{}` is out of range", node.package),
        match value {
            Some(depth) => format!("depth {depth}"),
            None => "unreachable".to_owned(),
        },
        range.render(),
        witness,
        violations,
    );
}

/// Evaluate the `height` range for one package.
///
/// A cycle in the downstream cone leaves the height undefined, which is a
/// graph error rather than a violation: the config asked an unanswerable
/// question about the graph as it is.
fn check_height(
    view: &Graph,
    range: &Range,
    node: &NodeReport,
    violations: &mut Vec<Violation>,
) -> Result<(), DepformError> {
    let (height, path) = view.height(node.package).map_err(|detail| {
        DepformError::Graph(format!(
            "cannot compute height of `{}`: {detail}",
            node.package
        ))
    })?;
    if range.check(Some(height)) {
        return Ok(());
    }
    node.fail(
        format!("height of package `{}` is out of range", node.package),
        format!("height {height}"),
        range.render(),
        Some(path.join(" -> ")),
        violations,
    );
    Ok(())
}

/// Evaluate a `distance` range for one pair.
///
/// An unreachable target has no distance, so the range semantics are the same
/// as for depth: a lower bound passes, a cap or an exact value fails.
fn check_distance(view: &Graph, range: &Range, pair: &PairReport, violations: &mut Vec<Violation>) {
    let path = view.path(pair.from, pair.to);
    let value = path.as_ref().map(|path| path.len() - 1);
    if range.check(value) {
        return;
    }
    pair.fail(
        format!(
            "distance from `{}` to `{}` is out of range",
            pair.from, pair.to
        ),
        match value {
            Some(distance) => format!("distance {distance}"),
            None => "unreachable".to_owned(),
        },
        range.render(),
        path.map(|path| path.join(" -> ")),
        violations,
    );
}

/// Which adjacency a degree check counts.
#[derive(Debug, Clone, Copy)]
enum CountKind {
    /// Direct dependencies.
    Dependencies,
    /// Direct dependents.
    Dependents,
}

/// Evaluate a degree range for one package.
fn check_count(
    view: &Graph,
    kind: CountKind,
    range: &Range,
    node: &NodeReport,
    violations: &mut Vec<Violation>,
) {
    let (count, message) = match kind {
        CountKind::Dependencies => (view.dependencies(node.package).count(), "dependency count"),
        CountKind::Dependents => (view.dependents(node.package).count(), "dependent count"),
    };
    if range.check(Some(count)) {
        return;
    }
    node.fail(
        format!("{message} of package `{}` is out of range", node.package),
        format!("{message} {count}"),
        range.render(),
        None,
        violations,
    );
}

/// One set check whose selectors are resolved against the view.
struct ResolvedSet<'a> {
    allow: Option<(&'a Selector, BTreeSet<String>)>,
    deny: Option<(&'a Selector, BTreeSet<String>)>,
    require: Option<(&'a Selector, BTreeSet<String>)>,
}

impl<'a> ResolvedSet<'a> {
    fn new(check: &'a SetCheck, view: &Graph) -> Self {
        Self {
            allow: resolve(check.allow.as_ref(), view),
            deny: resolve(check.deny.as_ref(), view),
            require: resolve(check.require.as_ref(), view),
        }
    }
}

/// Pair a selector with its resolved packages.
fn resolve<'a>(
    selector: Option<&'a Selector>,
    view: &Graph,
) -> Option<(&'a Selector, BTreeSet<String>)> {
    selector.map(|selector| (selector, selector.resolve(view)))
}

/// Which relation a set check inspects.
#[derive(Debug, Clone, Copy)]
enum SetKind {
    /// Direct dependencies.
    Dependencies,
    /// Transitive reachability.
    Reachable,
}

impl SetKind {
    /// The packages `package` is compared against.
    fn neighbors(self, view: &Graph, package: &str) -> BTreeSet<String> {
        match self {
            Self::Dependencies => view.dependencies(package).map(str::to_owned).collect(),
            Self::Reachable => view.reachable(package),
        }
    }

    /// The observed detail for one member of the relation.
    fn observation(self, view: &Graph, package: &str, target: &str) -> String {
        match self {
            Self::Dependencies => edge_observation(view, package, target),
            Self::Reachable => path_observation(view, package, target),
        }
    }

    /// The message for a member outside the allowed set.
    fn allow_message(self, package: &str, target: &str) -> String {
        match self {
            Self::Dependencies => {
                format!("package `{package}` has a dependency outside the allowed set: `{target}`")
            }
            Self::Reachable => {
                format!("package `{package}` reaches `{target}`, which is outside the allowed set")
            }
        }
    }

    /// The message for a member inside the denied set.
    fn deny_message(self, package: &str, target: &str) -> String {
        match self {
            Self::Dependencies => {
                format!("package `{package}` has a denied dependency `{target}`")
            }
            Self::Reachable => {
                format!("package `{package}` reaches denied package `{target}`")
            }
        }
    }

    /// The message for a required member that is absent.
    fn require_message(self, package: &str, target: &str) -> String {
        match self {
            Self::Dependencies => {
                format!("package `{package}` is missing required dependency `{target}`")
            }
            Self::Reachable => {
                format!("package `{package}` cannot reach required package `{target}`")
            }
        }
    }

    /// The observed detail for a required member that is absent.
    fn missing_observation(self, target: &str) -> String {
        match self {
            Self::Dependencies => format!("missing dependency {target}"),
            Self::Reachable => format!("missing reachable {target}"),
        }
    }
}

/// Evaluate one set check for one package.
fn check_sets(
    view: &Graph,
    kind: SetKind,
    sets: &ResolvedSet,
    node: &NodeReport,
    violations: &mut Vec<Violation>,
) {
    let neighbors = kind.neighbors(view, node.package);
    if let Some((selector, allowed)) = &sets.allow {
        for target in &neighbors {
            if !allowed.contains(target) {
                node.fail(
                    kind.allow_message(node.package, target),
                    kind.observation(view, node.package, target),
                    format!("allow {selector}"),
                    None,
                    violations,
                );
            }
        }
    }
    if let Some((selector, denied)) = &sets.deny {
        for target in &neighbors {
            if denied.contains(target) {
                node.fail(
                    kind.deny_message(node.package, target),
                    kind.observation(view, node.package, target),
                    format!("deny {selector}"),
                    None,
                    violations,
                );
            }
        }
    }
    if let Some((selector, required)) = &sets.require {
        for missing in required {
            if !neighbors.contains(missing) {
                node.fail(
                    kind.require_message(node.package, missing),
                    kind.missing_observation(missing),
                    format!("require {selector}"),
                    None,
                    violations,
                );
            }
        }
    }
}

/// Build one ordered detail pair.
fn detail(key: &'static str, value: impl Into<String>) -> (&'static str, String) {
    (key, value.into())
}

/// Render an observed edge with the kinds it carries in the checked view.
fn edge_observation(view: &Graph, from: &str, to: &str) -> String {
    let kinds = view
        .edge_kinds(from, to)
        .expect("a direct dependency has an edge");
    format!("dependency {from} -> {to} ({})", render_kinds(kinds))
}

/// Render an observed path from the checked view.
///
/// The caller only asks for a path it already knows exists: the package is in
/// the source's reachable set. A package that reaches itself through a cycle
/// is reported with its shortest closed path.
fn path_observation(view: &Graph, from: &str, to: &str) -> String {
    let path = if from == to {
        view.cycle_path(from)
    } else {
        view.path(from, to)
    }
    .expect("a reachable package has a path");
    format!("path {}", path.join(" -> "))
}
