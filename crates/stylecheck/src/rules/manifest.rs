//! Cargo manifest layout rules.
//!
//! TOML file-format rules live in [`super::toml`] and the workspace facts the
//! dependency-group rules need live in [`super::workspace`].

use toml_edit::{Document, Table};

use super::{
    toml::{check_toml_file, line_of, span_line},
    workspace::{WorkspaceMembers, collect_explicit_tables},
};
use crate::{Rule, Violation};

/// Check a manifest's line endings, sections and dependency groups using the
/// declaration order gathered from the root.
pub fn check_manifest_with_workspace_members(
    path: &str,
    text: &str,
    is_root: bool,
    workspace_members: &WorkspaceMembers,
) -> Vec<Violation> {
    let mut found = check_toml_file(path, text);
    let doc = match Document::parse(text) {
        Ok(doc) => doc,
        Err(error) => {
            let line = error.span().map_or(1, |span| line_of(text, span.start));
            found.push(Violation::with_detail(
                path,
                line,
                Rule::ManifestParse,
                text,
                error.to_string(),
            ));
            return found;
        }
    };
    let mut tables = Vec::new();
    collect_explicit_tables(doc.as_table(), "", text, &mut tables);

    /// Whether the manifest declares a direct table with this path.
    fn has_table(tables: &[(String, usize, &Table)], table_path: &str) -> bool {
        tables.iter().any(|(path, _, _)| path == table_path)
    }

    // A scanned root manifest is a workspace root only when it declares
    // `[workspace]`; a single-crate root is checked like any crate manifest
    // instead of silently skipping the package rules.
    let workspace_root = is_root && has_table(&tables, "workspace");
    if workspace_root {
        tables
            .iter()
            .filter(|(table_path, _, _)| table_path == "workspace.dependencies")
            .flat_map(|(_, _, table)| {
                check_root_dependency_groups(path, table, text, workspace_members)
            })
            .for_each(|violation| found.push(violation));
    }
    // A workspace root that is also a package carries both kinds of content;
    // every other manifest carries the package sections a crate manifest has.
    if !workspace_root || has_table(&tables, "package") {
        found.extend(check_section_order(path, &tables, text));
        found.extend(check_section_spacing(path, &tables, text));
        tables
            .iter()
            .filter(|(table_path, _, _)| {
                matches!(
                    table_path.as_str(),
                    "dependencies" | "dev-dependencies" | "build-dependencies"
                )
            })
            .flat_map(|(_, _, table)| {
                check_crate_dependency_groups(path, table, text, workspace_members)
            })
            .for_each(|violation| found.push(violation));
    }
    found
}

/// The only dependency aspects permitted in a crate manifest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum CrateDependencyGroup {
    WorkspaceMember,
    ThirdParty,
}

impl CrateDependencyGroup {
    const fn name(self) -> &'static str {
        match self {
            Self::WorkspaceMember => "workspace-member",
            Self::ThirdParty => "third-party",
        }
    }
}

/// Collect dependency entries in the source order preserved by `toml_edit`.
fn dependency_entries(table: &Table, text: &str) -> Vec<(String, usize)> {
    let mut entries = Vec::new();
    for (key, item) in table.iter() {
        if item.as_table().is_some_and(|sub| !sub.is_dotted()) {
            continue;
        }
        entries.push((key.to_owned(), span_line(item, text)));
    }
    entries
}

/// Split a table into the blank-line-delimited groups already present in the
/// source. The checker validates those groups; it never rewrites their shape.
fn dependency_groups<'a>(entries: &'a [(String, usize)], text: &str) -> Vec<&'a [(String, usize)]> {
    let mut groups = Vec::new();
    let mut start = 0;
    for index in 1..entries.len() {
        if blank_lines_between(text, entries[index - 1].1, entries[index].1) > 0 {
            groups.push(&entries[start..index]);
            start = index;
        }
    }
    if !entries.is_empty() {
        groups.push(&entries[start..]);
    }
    groups
}

/// Root groups follow the configured aspect order, then third-party
/// dependencies. Several adjacent groups may share an aspect because a
/// workspace may separate related packages for navigation.
fn check_root_dependency_groups(
    path: &str,
    table: &Table,
    text: &str,
    workspace_members: &WorkspaceMembers,
) -> Vec<Violation> {
    let entries = dependency_entries(table, text);
    let mut found = Vec::new();
    let mut previous: Option<(&str, usize)> = None;
    for entries in dependency_groups(&entries, text) {
        let (group, rank) = workspace_members.root_group(&entries[0].0);
        for (key, line) in entries.iter().skip(1) {
            let (observed, observed_rank) = workspace_members.root_group(key);
            if observed_rank != rank {
                found.push(Violation::with_detail(
                    path,
                    *line,
                    Rule::DependencyGroupOrder,
                    text,
                    format!(
                        "root group mixes {observed} dependency `{key}` with {group} dependencies"
                    ),
                ));
            }
        }
        if let Some((previous_group, previous_rank)) = previous
            && rank < previous_rank
        {
            found.push(Violation::with_detail(
                path,
                entries[0].1,
                Rule::DependencyGroupOrder,
                text,
                format!("{group} group appears after {previous_group} group"),
            ));
        }
        found.extend(check_group_sorting(path, entries, text, group));
        previous = Some((group, rank));
    }
    found
}

/// Crate manifests only distinguish workspace members from third-party
/// dependencies. The blank-line groups already present stay intact; each one
/// must contain a single aspect, appear after any earlier aspect's groups and
/// be internally sorted, mirroring the root-manifest semantics.
fn check_crate_dependency_groups(
    path: &str,
    table: &Table,
    text: &str,
    workspace_members: &WorkspaceMembers,
) -> Vec<Violation> {
    let entries = dependency_entries(table, text);
    let mut found = Vec::new();
    let mut previous = None;
    for entries in dependency_groups(&entries, text) {
        let group = crate_dependency_group(&entries[0].0, workspace_members);
        for (key, line) in entries.iter().skip(1) {
            let observed = crate_dependency_group(key, workspace_members);
            if observed != group {
                found.push(Violation::with_detail(
                    path,
                    *line,
                    Rule::DependencyGroupOrder,
                    text,
                    format!(
                        "crate group mixes {} dependency `{key}` with {} dependencies",
                        observed.name(),
                        group.name()
                    ),
                ));
            }
        }
        if let Some(previous) = previous
            && group < previous
        {
            found.push(Violation::with_detail(
                path,
                entries[0].1,
                Rule::DependencyGroupOrder,
                text,
                format!(
                    "{} group appears after {} group",
                    group.name(),
                    previous.name()
                ),
            ));
        }
        found.extend(check_group_sorting(path, entries, text, group.name()));
        previous = Some(group);
    }
    found
}

/// Report each local inversion with the two names that establish the order.
fn check_group_sorting(
    path: &str,
    entries: &[(String, usize)],
    text: &str,
    group_name: &str,
) -> Vec<Violation> {
    entries
        .windows(2)
        .filter_map(|pair| {
            let (previous_key, _) = &pair[0];
            let (key, line) = &pair[1];
            (key < previous_key).then(|| {
                Violation::with_detail(
                    path,
                    *line,
                    Rule::DependencySorted,
                    text,
                    format!("`{key}` must sort before `{previous_key}` in the {group_name} group"),
                )
            })
        })
        .collect()
}

fn crate_dependency_group(key: &str, workspace_members: &WorkspaceMembers) -> CrateDependencyGroup {
    if workspace_members.is_member(key) {
        CrateDependencyGroup::WorkspaceMember
    } else {
        CrateDependencyGroup::ThirdParty
    }
}

/// Rule `section-order` over direct Cargo sections.
fn check_section_order(
    path: &str,
    tables: &[(String, usize, &Table)],
    text: &str,
) -> Vec<Violation> {
    let mut found = Vec::new();
    let mut previous_rank: Option<u8> = None;
    for (table_path, line, _) in tables.iter().filter(|(path, _, _)| is_section(path)) {
        if let Some(rank) = section_rank(table_path)
            && let Some(previous) = previous_rank
            && rank < previous
        {
            found.push(Violation::with_detail(
                path,
                *line,
                Rule::ManifestSectionOrder,
                text,
                format!("section `{table_path}` must follow the canonical section order"),
            ));
        }
        previous_rank = section_rank(table_path);
    }
    found
}

/// Rule `section-spacing` over direct Cargo sections.
fn check_section_spacing(
    path: &str,
    tables: &[(String, usize, &Table)],
    text: &str,
) -> Vec<Violation> {
    let sections = tables
        .iter()
        .filter(|(table_path, _, _)| is_section(table_path))
        .collect::<Vec<_>>();
    sections
        .windows(2)
        .filter_map(|pair| {
            let (_, line_b, _) = pair[1];
            (blanks_after_last_content(text, pair[0].1, *line_b) != 1).then(|| {
                Violation::with_detail(
                    path,
                    *line_b,
                    Rule::ManifestSectionSpacing,
                    text,
                    "sections require exactly one blank line between their final content and the next header",
                )
            })
        })
        .collect()
}

/// Direct sections are ranked; nested metadata tables are not section headers.
fn is_section(table_path: &str) -> bool {
    matches!(
        table_path,
        "package"
            | "lib"
            | "features"
            | "dependencies"
            | "build-dependencies"
            | "dev-dependencies"
            | "lints"
    )
}

fn section_rank(table_path: &str) -> Option<u8> {
    Some(match table_path {
        "package" => 0,
        "lib" => 1,
        "features" => 2,
        "dependencies" => 3,
        "build-dependencies" => 4,
        "dev-dependencies" => 5,
        "lints" => 6,
        _ => return None,
    })
}

fn blanks_after_last_content(text: &str, header_a: usize, header_b: usize) -> usize {
    let mut count = 0;
    // An empty group's last content line is its own header.
    let mut saw_content = true;
    for (index, line) in text.lines().enumerate() {
        let line_no = index + 1;
        if line_no <= header_a || line_no >= header_b {
            continue;
        }
        if line.trim().is_empty() {
            if saw_content {
                count += 1;
            }
        } else {
            saw_content = true;
            count = 0;
        }
    }
    count
}

fn blank_lines_between(text: &str, line_a: usize, line_b: usize) -> usize {
    if line_b <= line_a + 1 {
        return 0;
    }
    text.lines()
        .enumerate()
        .filter(|(index, _)| *index + 1 > line_a && *index + 1 < line_b)
        .filter(|(_, line)| line.trim().is_empty())
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::workspace::fixtures::{ROOT_MANIFEST, aspects};

    fn crate_manifest(text: &str) -> Vec<Violation> {
        let members = WorkspaceMembers::from_root_manifest(ROOT_MANIFEST, &aspects());
        check_manifest_with_workspace_members("Cargo.toml", text, false, &members)
    }

    fn root_manifest(text: &str) -> Vec<Violation> {
        let members = WorkspaceMembers::from_root_manifest(text, &aspects());
        check_manifest_with_workspace_members("Cargo.toml", text, true, &members)
    }

    #[test]
    fn clean_crate_manifest_is_clean() {
        let text = r#"[package]
name = "probe"
version.workspace = true
edition.workspace = true
rust-version.workspace = true

[lib]
proc-macro = true

[dependencies]
engine-alpha.workspace = true
foundation-alpha.workspace = true
game-zulu.workspace = true

helper.workspace = true
proc-macro2.workspace = true
quote.workspace = true
serde.workspace = true
syn.workspace = true

[lints]
workspace = true
"#;
        let found = crate_manifest(text);
        assert!(found.is_empty(), "{found:?}");
    }

    #[test]
    fn root_group_order_is_reported() {
        let out_of_order = r#"[workspace]
resolver = "3"

[workspace.dependencies]
engine-alpha.path = "crates/engine/core/alpha"

foundation-alpha.path = "crates/foundation/alpha"
"#;
        let mixed = r#"[workspace]
resolver = "3"

[workspace.dependencies]
foundation-alpha.path = "crates/foundation/alpha"
game-alpha.path = "crates/game/core/alpha"
"#;
        let found = root_manifest(out_of_order);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].rule, Rule::DependencyGroupOrder);
        let found = root_manifest(mixed);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].rule, Rule::DependencyGroupOrder);
        assert!(
            found[0]
                .detail
                .as_deref()
                .is_some_and(|detail| detail.contains("mixes")),
            "{found:?}"
        );
    }

    #[test]
    fn crate_group_order_is_reported() {
        let text = r#"[package]
name = "probe"

[dependencies]
serde.workspace = true

engine-alpha.workspace = true
"#;
        let found = crate_manifest(text);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].rule, Rule::DependencyGroupOrder);
    }

    #[test]
    fn clean_root_manifest_is_clean() {
        let text = r#"[workspace]
resolver = "3"

[workspace.dependencies]
foundation-alpha.path = "crates/foundation/alpha"
foundation-beta.path = "crates/foundation/beta"

engine-alpha.path = "crates/engine/core/alpha"

game-alpha.path = "crates/game/core/alpha"

game-zulu.path = "crates/game/core/zulu"

codec = "1.8"

proc-macro2 = "1"
quote = "1"
syn = { version = "3", features = ["derive"] }
"#;
        assert!(root_manifest(text).is_empty());
    }

    #[test]
    fn root_manifest_without_workspace_is_checked_as_a_crate() {
        let text = r#"[package]
name = "probe"

[dependencies]
serde.workspace = true
arc-swap.workspace = true
"#;
        let found = root_manifest(text);
        assert!(
            found
                .iter()
                .any(|violation| violation.rule == Rule::DependencySorted),
            "{found:?}"
        );
    }

    #[test]
    fn sections_without_blank_line_are_reported() {
        let text = r#"[package]
name = "probe"
version.workspace = true
[dependencies]
foundation-alpha.workspace = true
"#;
        assert!(
            crate_manifest(text)
                .iter()
                .any(|violation| violation.rule == Rule::ManifestSectionSpacing)
        );
    }

    #[test]
    fn empty_section_spacing_counts_from_its_header() {
        let clean = r#"[package]
name = "probe"

[lib]

[dependencies]
serde.workspace = true
"#;
        assert!(crate_manifest(clean).is_empty());
        for text in [
            r#"[package]
name = "probe"

[lib]
[dependencies]
serde.workspace = true
"#,
            r#"[package]
name = "probe"

[lib]


[dependencies]
serde.workspace = true
"#,
        ] {
            let found = crate_manifest(text);
            assert_eq!(found.len(), 1, "{found:?}");
            assert_eq!(found[0].rule, Rule::ManifestSectionSpacing);
        }
    }

    #[test]
    fn dependency_sorting_is_reported() {
        // A root-table group, a crate workspace-member group and a crate
        // third-party group are each checked for internal order.
        let unsorted = [
            root_manifest(
                "[workspace]\nresolver = \"3\"\n\n[workspace.dependencies]\nfoundation-beta.path = \"crates/foundation/beta\"\nfoundation-alpha.path = \"crates/foundation/alpha\"\n",
            ),
            crate_manifest(
                "[package]\nname = \"probe\"\n\n[dependencies]\nengine-beta.workspace = true\nengine-alpha.workspace = true\n",
            ),
            crate_manifest(
                "[package]\nname = \"probe\"\n\n[dependencies]\nserde.workspace = true\nhelper.workspace = true\n",
            ),
        ];
        for found in unsorted {
            assert_eq!(found.len(), 1, "{found:?}");
            assert_eq!(found[0].rule, Rule::DependencySorted);
        }
    }

    #[test]
    fn line_endings_are_reported_for_a_manifest_that_also_fails_to_parse() {
        let text = "[package]\rname = \"probe\"\n";
        let found = crate_manifest(text);
        assert!(
            found
                .iter()
                .any(|violation| violation.rule == Rule::TomlLineEnding),
            "{found:?}"
        );
        assert!(
            found
                .iter()
                .any(|violation| violation.rule == Rule::ManifestParse),
            "{found:?}"
        );
    }

    #[test]
    fn invalid_toml_is_reported_with_context() {
        let found = crate_manifest("[package\nname = \"x\"\n");
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].rule, Rule::ManifestParse);
        assert!(found[0].detail.is_some());
        assert!(found[0].context.lines.iter().any(|line| line.highlighted));
    }

    #[test]
    fn nested_tables_are_not_sections() {
        let text = r#"[package]
name = "probe"

[package.metadata]
owner = "probe"

[dependencies]
foundation-alpha.workspace = true
"#;
        assert!(crate_manifest(text).is_empty());
    }
}
