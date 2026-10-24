//! Cargo manifest and TOML data-file layout rules.

use std::collections::{BTreeMap, BTreeSet};

use toml_edit::{Document, Item, Table};

use crate::{Rule, StyleConfig, Violation};

/// The aspect name every dependency outside the configured root aspects gets.
const THIRD_PARTY: &str = "third-party";

/// Workspace-member declaration order and root aspect, derived from the root
/// manifest and the dependency aspects declared in the style config rather
/// than from package-name or directory conventions.
#[derive(Debug, Clone)]
pub struct WorkspaceMembers {
    members: BTreeMap<String, WorkspaceMember>,
    member_names: BTreeSet<String>,
    /// Rank assigned to dependencies outside every configured aspect; one past
    /// the last declared aspect.
    third_party_rank: usize,
}

#[derive(Debug, Clone)]
struct WorkspaceMember {
    /// Position of the member's aspect in the declared aspect order.
    aspect: usize,
    /// The aspect name used in reports.
    name: String,
}

impl WorkspaceMembers {
    /// Derive workspace-member aspects from the root manifest and the aspect
    /// declarations of `config`: a `[workspace.dependencies]` entry carrying a
    /// `path` is a workspace member, and its root aspect is the first declared
    /// aspect whose prefix matches that path. A member outside every aspect
    /// still belongs to the workspace, but ranks with third-party
    /// dependencies. An invalid root yields an empty catalog; callers report
    /// the root parse diagnostic through
    /// [`check_manifest_with_workspace_members`].
    pub fn from_root_manifest(text: &str, config: &StyleConfig) -> Self {
        let mut members = Self {
            members: BTreeMap::new(),
            member_names: BTreeSet::new(),
            third_party_rank: config.dependency_aspects().len(),
        };
        let Ok(doc) = Document::parse(text) else {
            return members;
        };
        let mut tables = Vec::new();
        collect_explicit_tables(doc.as_table(), "", text, &mut tables);
        let Some((_, _, dependencies)) = tables
            .into_iter()
            .find(|(table_path, _, _)| table_path == "workspace.dependencies")
        else {
            return members;
        };
        members.members = dependencies
            .iter()
            .filter_map(|(key, item)| {
                let path = item.as_table_like()?.get("path")?.as_str()?;
                Some((key.to_owned(), member_from_path(path, config)))
            })
            .collect();
        members
    }

    /// Add the package names of the workspace member manifests the caller
    /// selected through the member globs. A member that no crate depends on
    /// still belongs to this set, so the dependency table alone cannot provide
    /// it.
    pub fn with_member_manifests(mut self, manifests: &[(&str, &str)]) -> Self {
        for (_, text) in manifests {
            let Ok(document) = Document::parse(text) else {
                continue;
            };
            let Some(name) = document
                .as_table()
                .get("package")
                .and_then(Item::as_table)
                .and_then(|package| package.get("name"))
                .and_then(Item::as_str)
            else {
                continue;
            };
            self.member_names.insert(name.replace('-', "_"));
        }
        self
    }

    /// The root aspect of a dependency key: a workspace member reports its
    /// configured aspect and declared rank; anything else ranks as
    /// third-party.
    fn root_group(&self, key: &str) -> (&str, usize) {
        self.members
            .get(key)
            .map_or((THIRD_PARTY, self.third_party_rank), |member| {
                (member.name.as_str(), member.aspect)
            })
    }

    /// Whether a dependency key names a workspace member.
    fn is_member(&self, key: &str) -> bool {
        self.members.contains_key(key)
    }

    /// The crate name of every known workspace member, normalized for source paths.
    pub fn member_crate_names(&self) -> BTreeSet<String> {
        self.members
            .keys()
            .map(|key| key.replace('-', "_"))
            .chain(self.member_names.iter().cloned())
            .collect()
    }
}

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

/// Rules `line-endings`, `trailing-blank-line` and `missing-trailing-newline`
/// over one TOML file: LF-only line endings and exactly one trailing newline.
/// Both apply to every TOML file the tree contains — the Cargo manifests and
/// the authored data files alike — so a data file cannot drift from the
/// manifest convention.
pub fn check_toml_file(path: &str, text: &str) -> Vec<Violation> {
    let mut found = check_line_endings(path, text);
    found.extend(check_trailing_newline(path, text));
    found
}

/// Rule `line-endings`: a TOML file must end its lines with LF only. The whole
/// file yields one report anchored at its first carriage return, so a CRLF file
/// does not flood the report with one violation per line; a file that also
/// fails to parse still reports its line endings, because the parse error alone
/// does not disclose them.
fn check_line_endings(path: &str, text: &str) -> Vec<Violation> {
    let Some(offset) = text.find('\r') else {
        return Vec::new();
    };
    let crlf = text.matches("\r\n").count();
    let bare = text.matches('\r').count() - crlf;
    let detail = if bare == 0 {
        format!("{crlf} CRLF line endings")
    } else {
        format!("{crlf} CRLF line endings and {bare} bare carriage returns")
    };
    vec![Violation::with_detail(
        path,
        line_of(text, offset),
        Rule::TomlLineEnding,
        text,
        detail,
    )]
}

/// Rules `trailing-blank-line` and `missing-trailing-newline`: a TOML file ends
/// with exactly one trailing newline. A missing final newline leaves the last
/// line ambiguous in a diff and a trailing blank line adds noise after the last
/// entry; either is reported. A CRLF file is left to the line-ending rule so it
/// is not reported twice for the same trailing bytes.
fn check_trailing_newline(path: &str, text: &str) -> Vec<Violation> {
    if text.is_empty() || text.contains('\r') {
        return Vec::new();
    }
    let Some(last_content) = text.rfind(|ch: char| !ch.is_whitespace()) else {
        return Vec::new();
    };
    // `rfind` returns the start of a character, so advance by its full UTF-8
    // width: a multibyte final character must not be split at a byte boundary.
    let content_end = last_content
        + text[last_content..]
            .chars()
            .next()
            .map_or(0, char::len_utf8);
    let tail = &text[content_end..];
    // Exactly one newline closes the content line. Whitespace before that
    // newline belongs to the content line, not to a new blank line.
    if tail.ends_with('\n') && tail.matches('\n').count() == 1 {
        return Vec::new();
    }
    if !tail.contains('\n') {
        let line = text.matches('\n').count() + 1;
        return vec![Violation::new(
            path,
            line,
            Rule::TomlMissingTrailingNewline,
            text,
        )];
    }
    // A blank line follows the content line; report the first one. The stored
    // excerpt includes it, so the locus never points past the end of the file.
    let line = line_of(text, content_end) + 1;
    vec![Violation::new(
        path,
        line,
        Rule::TomlTrailingBlankLine,
        text,
    )]
}

/// Every third-party crate name that can appear in a source path: the standard
/// prelude crates plus every dependency key in the root workspace table and in
/// each member manifest, minus the member crate names themselves. Dependency
/// keys are the crate names used in source paths (renamed dependencies use the
/// key as written).
pub fn external_crate_names(
    root_text: &str,
    member_manifests: &[(&str, &str)],
    members: &WorkspaceMembers,
) -> BTreeSet<String> {
    let mut external = BTreeSet::from([
        "std".to_owned(),
        "core".to_owned(),
        "alloc".to_owned(),
        "proc_macro".to_owned(),
    ]);
    collect_dependency_keys(root_text, &mut external);
    for (_, text) in member_manifests {
        collect_dependency_keys(text, &mut external);
    }
    for member in members.member_crate_names() {
        external.remove(&member);
    }
    external
}

/// Union the dependency keys of a manifest's dependency tables into `out`, with
/// `-` normalized to `_`. The root table is `[workspace.dependencies]`; crate
/// tables are `[dependencies]`/`[dev-dependencies]`/`[build-dependencies]`.
/// Both explicit headers and dotted `dependencies.foo = …` forms are handled.
fn collect_dependency_keys(text: &str, out: &mut BTreeSet<String>) {
    let Ok(document) = Document::parse(text) else {
        return;
    };
    let mut stack = vec![(String::new(), document.as_table())];
    while let Some((prefix, table)) = stack.pop() {
        for (key, item) in table.iter() {
            let table_path = if prefix.is_empty() {
                key.to_owned()
            } else {
                format!("{prefix}.{key}")
            };
            let Some(sub) = item.as_table() else {
                continue;
            };
            let relevant = table_path == "workspace.dependencies"
                || matches!(
                    table_path.as_str(),
                    "dependencies" | "dev-dependencies" | "build-dependencies"
                );
            if relevant {
                out.extend(sub.iter().map(|(dep, _)| dep.replace('-', "_")));
            }
            stack.push((table_path, sub));
        }
    }
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

/// Classify a declared workspace path through the configured aspects. A path
/// outside every aspect is still a workspace member, but ranks with
/// third-party dependencies.
fn member_from_path(path: &str, config: &StyleConfig) -> WorkspaceMember {
    match config.aspect_of_path(path) {
        Some(aspect) => WorkspaceMember {
            aspect,
            name: config.dependency_aspects()[aspect].name().to_owned(),
        },
        None => WorkspaceMember {
            aspect: config.dependency_aspects().len(),
            name: THIRD_PARTY.to_owned(),
        },
    }
}

fn crate_dependency_group(key: &str, workspace_members: &WorkspaceMembers) -> CrateDependencyGroup {
    if workspace_members.is_member(key) {
        CrateDependencyGroup::WorkspaceMember
    } else {
        CrateDependencyGroup::ThirdParty
    }
}

/// Collect every explicit table with its full dotted path and header line.
fn collect_explicit_tables<'a>(
    table: &'a Table,
    prefix: &str,
    text: &str,
    out: &mut Vec<(String, usize, &'a Table)>,
) {
    for (key, item) in table.iter() {
        let Some(sub) = item.as_table() else {
            continue;
        };
        let table_path = if prefix.is_empty() {
            key.to_owned()
        } else {
            format!("{prefix}.{key}")
        };
        if !sub.is_implicit() {
            out.push((table_path.clone(), span_line(item, text), sub));
        }
        collect_explicit_tables(sub, &table_path, text, out);
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

fn span_line(item: &Item, text: &str) -> usize {
    item.span().map_or(1, |span| line_of(text, span.start))
}

fn line_of(text: &str, offset: usize) -> usize {
    text[..offset.min(text.len())].matches('\n').count() + 1
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

    const ROOT_MANIFEST: &str = r#"[workspace]
resolver = "3"

[workspace.dependencies]
foundation-alpha.path = "crates/foundation/alpha"

engine-alpha.path = "crates/engine/core/alpha"
engine-beta.path = "crates/engine/core/beta"

game-alpha.path = "crates/game/core/alpha"
game-zulu.path = "crates/game/core/zulu"

helper = "2"
proc-macro2 = "1"
quote = "1"
serde = "1"
"#;

    /// The aspects the fixture root manifest declares its members under.
    fn aspects() -> StyleConfig {
        StyleConfig::from_toml(
            r#"[[dependency-group-order.aspects]]
name = "foundation"
paths = ["crates/foundation/"]

[[dependency-group-order.aspects]]
name = "engine"
paths = ["crates/engine/"]

[[dependency-group-order.aspects]]
name = "game"
paths = ["crates/game/"]
"#,
        )
        .expect("the fixture aspects parse")
    }

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
    fn crlf_manifest_is_reported_once_at_its_first_carriage_return() {
        let text = "[package]\r\nname = \"probe\"\r\n\r\n[dependencies]\r\nfoundation-alpha.workspace = true\r\n";
        let found = crate_manifest(text);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].rule, Rule::TomlLineEnding);
        assert_eq!(found[0].line, 1);
        assert!(
            found[0]
                .detail
                .as_deref()
                .is_some_and(|detail| detail.contains("CRLF"))
        );
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
    fn trailing_blank_line_is_reported() {
        for text in [
            "[package]\nname = \"probe\"\n\n",
            "[package]\nname = \"probe\"\n   \n",
        ] {
            let found = crate_manifest(text);
            assert_eq!(found.len(), 1, "{found:?}");
            assert_eq!(found[0].rule, Rule::TomlTrailingBlankLine);
        }
    }

    #[test]
    fn manifest_without_a_final_newline_is_reported() {
        let text = "[package]\nname = \"probe\"";
        let found = crate_manifest(text);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].rule, Rule::TomlMissingTrailingNewline);
        assert_eq!(found[0].line, 2);
    }

    #[test]
    fn multibyte_final_content_is_not_split_at_a_byte_boundary() {
        // The final non-whitespace character is `é` (two bytes), so the tail
        // slice must start after the whole character, not in the middle.
        for text in ["# café\n", "# café   \n"] {
            let found = check_toml_file("data.toml", text);
            assert!(found.is_empty(), "{text:?}: {found:?}");
        }

        let found = check_toml_file("data.toml", "# café");
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].rule, Rule::TomlMissingTrailingNewline);
        assert_eq!(found[0].line, 1);

        let found = check_toml_file("data.toml", "# café\n\n");
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].rule, Rule::TomlTrailingBlankLine);
        assert_eq!(found[0].line, 2);
    }

    #[test]
    fn trailing_spaces_do_not_make_a_blank_line() {
        // Spaces on the last content line are not a separate blank line.
        let found = check_toml_file("Cargo.toml", "[package]\nname = \"probe\"   \n");
        assert!(found.is_empty(), "{found:?}");

        // Without the closing newline the file is missing T3, not a blank line.
        let found = check_toml_file("Cargo.toml", "[package]\nname = \"probe\"   ");
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].rule, Rule::TomlMissingTrailingNewline);
        assert_eq!(found[0].line, 2);
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

    #[test]
    fn member_crate_names_normalize_dashes() {
        let members = WorkspaceMembers::from_root_manifest(ROOT_MANIFEST, &aspects());
        let names = members.member_crate_names();
        assert!(names.contains("foundation_alpha"));
        assert!(names.contains("engine_alpha"));
        assert!(names.contains("game_zulu"));
        assert!(!names.contains("helper"));
    }

    #[test]
    fn external_crate_names_collect_deps_and_exclude_members() {
        let members = WorkspaceMembers::from_root_manifest(ROOT_MANIFEST, &aspects());
        let crate_manifest = r#"[package]
name = "probe"

[dependencies]
foundation-alpha.workspace = true
codec.workspace = true
serde.workspace = true
"#;
        let external =
            external_crate_names(ROOT_MANIFEST, &[("Cargo.toml", crate_manifest)], &members);
        for seeded in ["std", "core", "alloc", "proc_macro"] {
            assert!(external.contains(seeded), "missing seeded {seeded}");
        }
        for third_party in ["serde", "proc_macro2", "quote", "codec"] {
            assert!(external.contains(third_party), "missing {third_party}");
        }
        for member in ["foundation_alpha", "engine_alpha", "game_zulu"] {
            assert!(!external.contains(member), "{member} must be subtracted");
        }
    }
}
