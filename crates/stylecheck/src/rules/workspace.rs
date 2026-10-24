//! Workspace facts the dependency-group rules need: member aspects and the
//! external crate names source paths may spell.

use std::collections::{BTreeMap, BTreeSet};

use toml_edit::{Document, Item, Table};

use super::toml::span_line;
use crate::StyleConfig;

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
    /// dependencies. An invalid root yields an empty catalog; the manifest
    /// rule reports the root parse diagnostic separately.
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
    pub(crate) fn root_group(&self, key: &str) -> (&str, usize) {
        self.members
            .get(key)
            .map_or((THIRD_PARTY, self.third_party_rank), |member| {
                (member.name.as_str(), member.aspect)
            })
    }

    /// Whether a dependency key names a workspace member.
    pub(crate) fn is_member(&self, key: &str) -> bool {
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

/// Collect every explicit table with its full dotted path and header line.
pub(crate) fn collect_explicit_tables<'a>(
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

#[cfg(test)]
pub(crate) mod fixtures {
    use crate::StyleConfig;

    pub(crate) const ROOT_MANIFEST: &str = r#"[workspace]
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
    pub(crate) fn aspects() -> StyleConfig {
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
}

#[cfg(test)]
mod tests {
    use super::{
        fixtures::{ROOT_MANIFEST, aspects},
        *,
    };

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
