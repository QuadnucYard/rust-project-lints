//! Contract text rendering.
//!
//! A violation is one broken assertion. Its details are ordered by the
//! contract, so the rendered block reads the same way for every rule and a
//! caller can compare details positionally. The graph listing follows the same
//! rule: one stable, greppable format owned by this module.

use std::collections::BTreeSet;

use crate::{DependencyKind, Graph};

/// One rule violation with the details the diagnostic prints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    rule: String,
    message: String,
    details: Vec<(&'static str, String)>,
}

impl Violation {
    /// Build a violation; details keep the order given here.
    pub(crate) fn new(
        rule: impl Into<String>,
        message: impl Into<String>,
        details: Vec<(&'static str, String)>,
    ) -> Self {
        Self {
            rule: rule.into(),
            message: message.into(),
            details,
        }
    }

    /// The rule that reported the violation.
    pub fn rule(&self) -> &str {
        &self.rule
    }

    /// The one-line description of the broken assertion.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// The ordered `(key, value)` detail pairs.
    pub fn details(&self) -> &[(&'static str, String)] {
        &self.details
    }

    /// Render the violation as one diagnostic block, without a trailing
    /// newline.
    pub fn render(&self) -> String {
        let mut rendered = format!("error[{}]: {}", self.rule, self.message);
        for (key, value) in &self.details {
            rendered.push('\n');
            rendered.push_str("  ");
            rendered.push_str(key);
            rendered.push_str(": ");
            rendered.push_str(value);
        }
        rendered
    }
}

/// Render violations in their reported order, one blank line between blocks.
pub fn render_violations(violations: &[Violation]) -> String {
    let mut rendered = String::new();
    for violation in violations {
        rendered.push_str(&violation.render());
        rendered.push_str("\n\n");
    }
    rendered
}

/// Render every package and every declared workspace-internal edge, with no
/// blank lines so the listing stays greppable.
pub fn render_graph(graph: &Graph) -> String {
    let mut rendered = format!("packages: {}\n", graph.package_count());
    for package in graph.packages() {
        rendered.push_str("  ");
        rendered.push_str(package);
        rendered.push('\n');
    }
    rendered.push_str(&format!("edges: {}\n", graph.edge_count()));
    for (from, to, kinds) in graph.edges() {
        rendered.push_str("  ");
        rendered.push_str(from);
        rendered.push_str(" -> ");
        rendered.push_str(to);
        rendered.push_str(" (");
        rendered.push_str(&render_kinds(kinds));
        rendered.push_str(")\n");
    }
    rendered
}

/// Render a kind set in enum order, comma separated.
pub(crate) fn render_kinds(kinds: &BTreeSet<DependencyKind>) -> String {
    kinds
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn detail(key: &'static str, value: &str) -> (&'static str, String) {
        (key, value.to_owned())
    }

    #[test]
    fn a_block_lists_its_details_in_order() {
        let violation = Violation::new(
            "core-isolation",
            "package `client` has a denied dependency `core`",
            vec![
                detail("selection", "\"client\""),
                detail("observed", "dependency client -> core (normal)"),
                detail("expected", "deny \"core\""),
            ],
        );
        assert_eq!(
            violation.render(),
            "error[core-isolation]: package `client` has a denied dependency `core`\n  \
             selection: \"client\"\n  \
             observed: dependency client -> core (normal)\n  \
             expected: deny \"core\""
        );
        assert_eq!(violation.rule(), "core-isolation");
    }

    #[test]
    fn blocks_are_separated_by_one_blank_line() {
        let first = Violation::new("a", "first", Vec::new());
        let second = Violation::new("b", "second", vec![detail("k", "v")]);
        assert_eq!(
            render_violations(&[first, second]),
            "error[a]: first\n\nerror[b]: second\n  k: v\n\n"
        );
        assert_eq!(render_violations(&[]), "");
    }
}
