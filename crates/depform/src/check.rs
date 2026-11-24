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
    config::{NodeRule, RuleKind, Selector, SetCheck},
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
    let mut violations = Vec::new();
    for rule in config.rules() {
        match &rule.kind {
            RuleKind::Node(node) => check_node(&view, &rule.name, node, &mut violations),
        }
    }
    Ok(violations)
}

/// Evaluate every node check of one rule against its selected packages.
fn check_node(view: &Graph, rule: &str, node: &NodeRule, violations: &mut Vec<Violation>) {
    let selection = node.select.to_string();
    let dependencies = node
        .dependencies
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
    }
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
}

impl SetKind {
    /// The packages `package` is compared against.
    fn neighbors(self, view: &Graph, package: &str) -> BTreeSet<String> {
        match self {
            Self::Dependencies => view.dependencies(package).map(str::to_owned).collect(),
        }
    }

    /// The observed detail for one member of the relation.
    fn observation(self, view: &Graph, package: &str, target: &str) -> String {
        match self {
            Self::Dependencies => edge_observation(view, package, target),
        }
    }

    /// The message for a member outside the allowed set.
    fn allow_message(self, package: &str, target: &str) -> String {
        match self {
            Self::Dependencies => {
                format!("package `{package}` has a dependency outside the allowed set: `{target}`")
            }
        }
    }

    /// The message for a member inside the denied set.
    fn deny_message(self, package: &str, target: &str) -> String {
        match self {
            Self::Dependencies => {
                format!("package `{package}` has a denied dependency `{target}`")
            }
        }
    }

    /// The message for a required member that is absent.
    fn require_message(self, package: &str, target: &str) -> String {
        match self {
            Self::Dependencies => {
                format!("package `{package}` is missing required dependency `{target}`")
            }
        }
    }

    /// The observed detail for a required member that is absent.
    fn missing_observation(self, target: &str) -> String {
        match self {
            Self::Dependencies => format!("missing dependency {target}"),
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
