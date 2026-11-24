//! Contract text rendering.
//!
//! The graph listing owns one stable, greppable format, so a caller compares
//! two listings line by line.

use std::collections::BTreeSet;

use crate::{DependencyKind, Graph};

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
