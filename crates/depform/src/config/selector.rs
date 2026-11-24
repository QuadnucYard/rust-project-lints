//! Package selectors.
//!
//! A [`Selector`] resolves to a set of package names, serving rule selections
//! and the set checks. [`Select`] adds the pair form that only pair rules use,
//! so pair selectors cannot reach a set check. Parsing, rendering and
//! empty-selection validation live here. Matching is purely textual: `*` is
//! the only wildcard and regular expressions are never used.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::{Display, Formatter, Result as FmtResult},
};

use toml::{Table, Value};

use super::is_identifier;
use crate::{DepformError, Graph};

/// A set of packages selected by a config rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Selector {
    /// A package-name pattern; `*` matches any run of characters.
    Name(String),
    /// A group reference, carrying the patterns the group declares.
    Group {
        /// The group name as written after `@`.
        name: String,
        /// The patterns the group declares; they match as a union.
        patterns: Vec<String>,
    },
    /// The union of the listed selectors.
    Union(Vec<Selector>),
    /// The direct dependents of the inner selection.
    DependentsOf(Box<Selector>),
    /// The direct dependencies of the inner selection.
    DependenciesOf(Box<Selector>),
    /// The transitive dependencies of the inner selection.
    ReachableFrom(Box<Selector>),
}

/// A rule's `select`: either packages, or ordered pairs of packages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Select {
    /// A selection of packages, used by node checks.
    Node(Selector),
    /// Ordered pairs, used by pair checks.
    Pair {
        /// The pairs' first components.
        from: Selector,
        /// The pairs' second components.
        to: Selector,
    },
}

impl Selector {
    /// Parse one selector value.
    pub(crate) fn parse(value: &Value, groups: &Groups) -> Result<Self, String> {
        match value {
            Value::String(text) => Self::parse_text(text, groups),
            Value::Array(items) => Self::parse_union(items, groups),
            Value::Table(table) => Self::parse_table(table, groups),
            other => Err(format!("expected a selector, found {other}")),
        }
    }

    /// Parse a name selector: a string or an array of strings, never a table.
    ///
    /// Structural arguments must be name selectors because structural
    /// selectors do not compose.
    fn parse_name(value: &Value, groups: &Groups) -> Result<Self, String> {
        match value {
            Value::String(_) | Value::Array(_) => Self::parse(value, groups),
            other => Err(format!("expected a name selector, found {other}")),
        }
    }

    /// Parse a graph-position table with exactly one key.
    fn parse_table(table: &Table, groups: &Groups) -> Result<Self, String> {
        let mut entries = table.iter();
        let Some((key, argument)) = entries.next() else {
            return Err(
                "a selector table must name exactly one of `dependents-of`, \
                 `dependencies-of` and `reachable-from`"
                    .to_owned(),
            );
        };
        let inner = match key.as_str() {
            "dependents-of" => Self::DependentsOf,
            "dependencies-of" => Self::DependenciesOf,
            "reachable-from" => Self::ReachableFrom,
            "from" | "to" => {
                return Err("a pair selector is not allowed here; use `dependents-of`, \
                     `dependencies-of` or `reachable-from`"
                    .to_owned());
            }
            other => {
                return Err(format!(
                    "unknown selector `{other}`; use `dependents-of`, `dependencies-of` \
                     or `reachable-from`"
                ));
            }
        };
        if entries.next().is_some() {
            return Err("a selector table must name exactly one key".to_owned());
        }
        Ok(inner(Box::new(Self::parse_name(argument, groups)?)))
    }

    /// Parse an array selector, whose entries must be strings.
    fn parse_union(items: &[Value], groups: &Groups) -> Result<Self, String> {
        if items.is_empty() {
            return Err("a selector array must list at least one selector".to_owned());
        }
        let mut selectors = Vec::with_capacity(items.len());
        for item in items {
            let Value::String(text) = item else {
                return Err(format!("a selector array holds strings, found {item}"));
            };
            selectors.push(Self::parse_text(text, groups)?);
        }
        Ok(Self::Union(selectors))
    }

    /// Parse a string selector: a group reference or a name pattern.
    fn parse_text(text: &str, groups: &Groups) -> Result<Self, String> {
        if let Some(name) = text.strip_prefix('@') {
            let Some(patterns) = groups.get(name) else {
                return Err(format!("unknown group `@{name}`"));
            };
            return Ok(Self::Group {
                name: name.to_owned(),
                patterns: patterns.to_vec(),
            });
        }
        validate_pattern(text)?;
        Ok(Self::Name(text.to_owned()))
    }

    /// The selected packages, sorted.
    pub(crate) fn resolve(&self, graph: &Graph) -> BTreeSet<String> {
        match self {
            Self::Name(pattern) => selected(graph, |package| matches_pattern(pattern, package)),
            Self::Group { patterns, .. } => selected(graph, |package| {
                patterns
                    .iter()
                    .any(|pattern| matches_pattern(pattern, package))
            }),
            Self::Union(items) => items.iter().flat_map(|item| item.resolve(graph)).collect(),
            Self::DependentsOf(inner) => {
                union_neighbors(graph, inner, |graph, package, selected| {
                    selected.extend(graph.dependents(package).map(str::to_owned));
                })
            }
            Self::DependenciesOf(inner) => {
                union_neighbors(graph, inner, |graph, package, selected| {
                    selected.extend(graph.dependencies(package).map(str::to_owned));
                })
            }
            Self::ReachableFrom(inner) => {
                union_neighbors(graph, inner, |graph, package, selected| {
                    selected.extend(graph.reachable(package));
                })
            }
        }
    }

    /// Reject a selector whose explicit name leaves match nothing.
    ///
    /// A pattern that selects no package is almost certainly a typo, so it is
    /// a config error rather than a rule that silently holds. A group is
    /// validated as a whole, so one stale pattern inside an otherwise matching
    /// group is tolerated.
    pub(crate) fn validate_explicit(&self, graph: &Graph, rule: &str) -> Result<(), DepformError> {
        match self {
            Self::Union(items) => {
                for item in items {
                    item.validate_explicit(graph, rule)?;
                }
                Ok(())
            }
            Self::DependentsOf(inner)
            | Self::DependenciesOf(inner)
            | Self::ReachableFrom(inner) => inner.validate_explicit(graph, rule),
            Self::Name(_) | Self::Group { .. } => {
                if self.resolve(graph).is_empty() {
                    return Err(DepformError::Config(format!(
                        "rule `{rule}`: selector {self} matches no package in the graph"
                    )));
                }
                Ok(())
            }
        }
    }
}

impl Select {
    /// Parse a rule's `select`: a pair table when it names `from` or `to`,
    /// otherwise a package selector.
    pub(crate) fn parse(value: &Value, groups: &Groups) -> Result<Self, String> {
        if let Value::Table(table) = value
            && (table.contains_key("from") || table.contains_key("to"))
        {
            let (from, to) = parse_pair(table, groups)?;
            return Ok(Self::Pair { from, to });
        }
        Ok(Self::Node(Selector::parse(value, groups)?))
    }
}

impl Display for Selector {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        match self {
            Self::Name(pattern) => write!(f, "{pattern:?}"),
            Self::Group { name, .. } => write!(f, "@{name}"),
            Self::Union(items) => {
                let rendered: Vec<String> = items.iter().map(ToString::to_string).collect();
                write!(f, "[{}]", rendered.join(", "))
            }
            Self::DependentsOf(inner) => write!(f, "{{ dependents-of = {inner} }}"),
            Self::DependenciesOf(inner) => write!(f, "{{ dependencies-of = {inner} }}"),
            Self::ReachableFrom(inner) => write!(f, "{{ reachable-from = {inner} }}"),
        }
    }
}

impl Display for Select {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        match self {
            Self::Node(selector) => selector.fmt(f),
            Self::Pair { from, to } => write!(f, "{{ from = {from}, to = {to} }}"),
        }
    }
}

/// Parse a pair table: exactly `from` and `to`, both name selectors.
fn parse_pair(table: &Table, groups: &Groups) -> Result<(Selector, Selector), String> {
    let mut from = None;
    let mut to = None;
    for (key, value) in table {
        let slot = match key.as_str() {
            "from" => &mut from,
            "to" => &mut to,
            other => {
                return Err(format!(
                    "unknown pair selector key `{other}`; use `from` and `to`"
                ));
            }
        };
        *slot = Some(Selector::parse_name(value, groups)?);
    }
    let (Some(from), Some(to)) = (from, to) else {
        return Err("a pair selector needs both `from` and `to`".to_owned());
    };
    Ok((from, to))
}

/// The ordered pairs `from -> to`, sorted, without self-pairs.
pub(crate) fn resolve_pairs(
    from: &Selector,
    to: &Selector,
    graph: &Graph,
) -> Vec<(String, String)> {
    let from = from.resolve(graph);
    let to = to.resolve(graph);
    let mut pairs = Vec::with_capacity(from.len() * to.len());
    for source in &from {
        for target in &to {
            if source != target {
                pairs.push((source.clone(), target.clone()));
            }
        }
    }
    pairs
}

/// The packages of `graph` that `matches` accepts, sorted.
fn selected(graph: &Graph, matches: impl Fn(&str) -> bool) -> BTreeSet<String> {
    graph
        .packages()
        .filter(|package| matches(package))
        .map(str::to_owned)
        .collect()
}

/// Resolve `inner` and union the neighbor sets `extend` adds per package.
fn union_neighbors(
    graph: &Graph,
    inner: &Selector,
    mut extend: impl FnMut(&Graph, &str, &mut BTreeSet<String>),
) -> BTreeSet<String> {
    let mut selected = BTreeSet::new();
    for package in inner.resolve(graph) {
        extend(graph, &package, &mut selected);
    }
    selected
}

/// The `[groups]` table: named lists of package-name patterns.
#[derive(Debug, Default)]
pub(crate) struct Groups {
    groups: BTreeMap<String, Vec<String>>,
}

impl Groups {
    /// Validate the declared groups and keep them for selector resolution.
    pub(crate) fn new(groups: BTreeMap<String, Vec<String>>) -> Result<Groups, DepformError> {
        for (name, patterns) in &groups {
            if !is_identifier(name) {
                return Err(DepformError::Config(format!(
                    "invalid group name {name:?}: use ASCII alphanumerics, `_` and `-`"
                )));
            }
            if patterns.is_empty() {
                return Err(DepformError::Config(format!(
                    "group `{name}` must list at least one pattern"
                )));
            }
            for pattern in patterns {
                if pattern.starts_with('@') {
                    return Err(DepformError::Config(format!(
                        "group `{name}` entries are patterns, not group references: {pattern:?}"
                    )));
                }
                validate_pattern(pattern)
                    .map_err(|detail| DepformError::Config(format!("group `{name}`: {detail}")))?;
            }
        }
        Ok(Self { groups })
    }

    /// The patterns of `name`, or `None` when the group is not declared.
    fn get(&self, name: &str) -> Option<&[String]> {
        self.groups.get(name).map(Vec::as_slice)
    }
}

/// Reject patterns outside the supported character set.
fn validate_pattern(pattern: &str) -> Result<(), String> {
    if pattern.is_empty() {
        return Err("a package-name pattern must not be empty".to_owned());
    }
    for character in pattern.chars() {
        if !(character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '*')) {
            return Err(format!(
                "invalid package-name pattern {pattern:?}: {character:?} is not allowed; \
                 use ASCII alphanumerics, `-`, `_` and `*`"
            ));
        }
    }
    Ok(())
}

/// Whether a `*`-pattern matches the whole package name.
///
/// The pattern is split at its wildcards: the first part must start the name,
/// the last part must end it, and the parts between must appear in order in
/// the remaining window. `*` matches any run of characters, including none.
pub(crate) fn matches_pattern(pattern: &str, name: &str) -> bool {
    let Some((first, tail)) = pattern.split_once('*') else {
        return pattern == name;
    };
    let (middle, last) = match tail.rsplit_once('*') {
        Some((middle, last)) => (middle, last),
        None => ("", tail),
    };
    if name.len() < first.len() + last.len() {
        return false;
    }
    if !name.starts_with(first) || !name.ends_with(last) {
        return false;
    }
    let window = &name[first.len()..name.len() - last.len()];
    let mut cursor = 0;
    for part in middle.split('*') {
        if part.is_empty() {
            continue;
        }
        match window[cursor..].find(part) {
            Some(index) => cursor += index + part.len(),
            None => return false,
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn groups(entries: &[(&str, &[&str])]) -> Groups {
        let table: BTreeMap<String, Vec<String>> = entries
            .iter()
            .map(|(name, patterns)| {
                (
                    (*name).to_owned(),
                    patterns
                        .iter()
                        .map(|pattern| (*pattern).to_owned())
                        .collect(),
                )
            })
            .collect();
        Groups::new(table).expect("valid groups")
    }

    fn graph(names: &[&str]) -> Graph {
        Graph::new(names.iter().map(|name| (*name).to_owned()), [])
    }

    fn selector(value: &str, groups: &Groups) -> Selector {
        Selector::parse(&Value::String(value.to_owned()), groups).expect("a valid selector")
    }

    #[test]
    fn an_exact_pattern_matches_only_itself() {
        assert!(matches_pattern("core", "core"));
        assert!(!matches_pattern("core", "core-utils"));
        assert!(!matches_pattern("core", "Core"));
    }

    #[test]
    fn a_lone_star_matches_anything() {
        assert!(matches_pattern("*", "core"));
        assert!(matches_pattern("*", "service-client-core"));
    }

    #[test]
    fn wildcards_match_prefixes_suffixes_and_middles() {
        assert!(matches_pattern("service-*", "service-api"));
        assert!(matches_pattern("service-*", "service-"));
        assert!(!matches_pattern("service-*", "other-service-api"));
        assert!(matches_pattern("*-core", "service-core"));
        assert!(!matches_pattern("*-core", "core"));
        assert!(matches_pattern("*core", "core"));
        assert!(!matches_pattern("*-core", "core-utils"));
        assert!(matches_pattern("a*b", "ab"));
        assert!(matches_pattern("a*b", "axxb"));
        assert!(!matches_pattern("a*b", "axx"));
        assert!(matches_pattern("*-client-*", "service-client-core"));
        assert!(!matches_pattern("*-client-*", "service-core-client"));
    }

    #[test]
    fn several_wildcards_keep_their_order() {
        assert!(matches_pattern("a*b*c", "abc"));
        assert!(matches_pattern("a*b*c", "a-x-b-y-c"));
        assert!(!matches_pattern("a*b*c", "a-c-b"));
        assert!(matches_pattern("**", "anything"));
        assert!(matches_pattern("a**b", "ab"));
        assert!(matches_pattern("a**b", "axb"));
    }

    #[test]
    fn overlapping_affixes_do_not_match() {
        assert!(!matches_pattern("ab*ab", "ab"));
        assert!(matches_pattern("ab*ab", "abab"));
        assert!(matches_pattern("ab*ab", "abxab"));
    }

    #[test]
    fn patterns_reject_characters_outside_the_alphabet() {
        assert!(validate_pattern("service-client").is_ok());
        assert!(validate_pattern("service_client*").is_ok());
        assert!(validate_pattern("two words").is_err());
        assert!(validate_pattern("a.b").is_err());
        assert!(validate_pattern("a+b").is_err());
        assert!(validate_pattern("").is_err());
    }

    #[test]
    fn name_selectors_parse_strings_and_string_arrays() {
        let groups = groups(&[]);
        let value = Value::String("core*".to_owned());
        assert!(Selector::parse(&value, &groups).is_ok());
        assert!(Selector::parse(&Value::Integer(1), &groups).is_err());
        assert!(Selector::parse(&Value::Array(Vec::new()), &groups).is_err());
        assert!(Selector::parse(&Value::Array(vec![Value::Integer(1)]), &groups).is_err());
        assert!(
            Selector::parse(&Value::Table(Table::new()), &groups)
                .expect_err("a table selector")
                .contains("exactly one of")
        );
    }

    #[test]
    fn a_group_resolves_its_patterns_as_a_union() {
        let groups = groups(&[("core", &["core", "shared-*"]), ("clients", &["client*"])]);
        let graph = graph(&["core", "shared-utils", "client-a", "app"]);
        assert_eq!(
            selector("@core", &groups).resolve(&graph),
            BTreeSet::from(["core".to_owned(), "shared-utils".to_owned()])
        );
        assert_eq!(
            selector("@clients", &groups).resolve(&graph),
            BTreeSet::from(["client-a".to_owned()])
        );
    }

    #[test]
    fn a_union_combines_names_groups_and_patterns() {
        let groups = groups(&[("core", &["core"])]);
        let graph = graph(&["core", "extra", "other"]);
        let union = Selector::parse(
            &Value::Array(vec![
                Value::String("@core".to_owned()),
                Value::String("extra".to_owned()),
            ]),
            &groups,
        )
        .expect("a union");
        assert_eq!(
            union.resolve(&graph),
            BTreeSet::from(["core".to_owned(), "extra".to_owned()])
        );
        assert_eq!(union.to_string(), "[@core, \"extra\"]");
    }

    #[test]
    fn unknown_groups_are_rejected() {
        let groups = groups(&[]);
        let error = Selector::parse(&Value::String("@nope".to_owned()), &groups)
            .expect_err("an unknown group");
        assert_eq!(error, "unknown group `@nope`");
    }

    #[test]
    fn group_tables_reject_references_and_bad_shapes() {
        let table: BTreeMap<String, Vec<String>> =
            BTreeMap::from([("a".to_owned(), vec!["@b".to_owned()])]);
        assert!(
            Groups::new(table)
                .expect_err("a group reference")
                .to_string()
                .contains("not group references")
        );
        let table: BTreeMap<String, Vec<String>> = BTreeMap::from([("a".to_owned(), Vec::new())]);
        assert!(
            Groups::new(table)
                .expect_err("an empty group")
                .to_string()
                .contains("at least one pattern")
        );
        let table: BTreeMap<String, Vec<String>> =
            BTreeMap::from([("a".to_owned(), vec![String::new()])]);
        assert!(
            Groups::new(table)
                .expect_err("an empty entry")
                .to_string()
                .contains("must not be empty")
        );
        let table: BTreeMap<String, Vec<String>> =
            BTreeMap::from([("a b".to_owned(), vec!["x".to_owned()])]);
        assert!(
            Groups::new(table)
                .expect_err("an invalid name")
                .to_string()
                .contains("invalid group name")
        );
    }

    /// Parse a selector written as a TOML document.
    fn document(source: &str, groups: &Groups) -> Result<Selector, String> {
        let value = toml::from_str::<Value>(source).expect("valid TOML");
        Selector::parse(&value, groups)
    }

    /// Parse a rule selection written as a TOML document.
    fn select(source: &str, groups: &Groups) -> Result<Select, String> {
        let value = toml::from_str::<Value>(source).expect("valid TOML");
        Select::parse(&value, groups)
    }

    /// `app -> facade -> core`, `other -> facade`, `core -> base`.
    fn layer_graph() -> Graph {
        Graph::new(
            ["app", "other", "facade", "core", "base"]
                .iter()
                .map(|name| (*name).to_owned()),
            [
                (
                    "app".to_owned(),
                    "facade".to_owned(),
                    crate::DependencyKind::Normal,
                ),
                (
                    "other".to_owned(),
                    "facade".to_owned(),
                    crate::DependencyKind::Normal,
                ),
                (
                    "facade".to_owned(),
                    "core".to_owned(),
                    crate::DependencyKind::Normal,
                ),
                (
                    "core".to_owned(),
                    "base".to_owned(),
                    crate::DependencyKind::Normal,
                ),
            ],
        )
    }

    fn resolved(source: &str, groups: &Groups, graph: &Graph) -> BTreeSet<String> {
        document(source, groups)
            .expect("a valid selector")
            .resolve(graph)
    }

    #[test]
    fn position_selectors_follow_one_or_more_edges() {
        let groups = groups(&[]);
        let graph = layer_graph();
        assert_eq!(
            resolved("dependents-of = \"facade\"", &groups, &graph),
            BTreeSet::from(["app".to_owned(), "other".to_owned()])
        );
        assert_eq!(
            resolved("dependencies-of = \"facade\"", &groups, &graph),
            BTreeSet::from(["core".to_owned()])
        );
        assert_eq!(
            resolved("reachable-from = \"app\"", &groups, &graph),
            BTreeSet::from(["base".to_owned(), "core".to_owned(), "facade".to_owned()])
        );
        assert_eq!(
            resolved("reachable-from = \"facade\"", &groups, &graph),
            BTreeSet::from(["base".to_owned(), "core".to_owned()])
        );
    }

    #[test]
    fn position_arguments_accept_groups_and_unions() {
        let groups = groups(&[("middle", &["facade"])]);
        let graph = layer_graph();
        assert_eq!(
            resolved("dependents-of = \"@middle\"", &groups, &graph),
            BTreeSet::from(["app".to_owned(), "other".to_owned()])
        );
        assert_eq!(
            resolved("dependencies-of = [\"app\", \"core\"]", &groups, &graph),
            BTreeSet::from(["base".to_owned(), "facade".to_owned()])
        );
    }

    #[test]
    fn position_selectors_render_and_reject_bad_shapes() {
        let groups = groups(&[("core", &["core"])]);
        assert_eq!(
            document("dependents-of = \"@core\"", &groups)
                .expect("a selector")
                .to_string(),
            "{ dependents-of = @core }"
        );
        assert_eq!(
            document("via = 1", &groups).expect_err("an unknown key"),
            "unknown selector `via`; use `dependents-of`, `dependencies-of` or \
             `reachable-from`"
        );
        assert!(
            document("from = \"a\"\nto = \"b\"", &groups)
                .expect_err("a pair selector in a name position")
                .contains("a pair selector is not allowed here")
        );
        assert!(
            document("dependents-of = { dependencies-of = \"a\" }", &groups)
                .expect_err("a nested table")
                .contains("expected a name selector")
        );
        assert!(
            document("dependents-of = \"a\"\ndependencies-of = \"b\"", &groups)
                .expect_err("two keys")
                .contains("exactly one key")
        );
        assert_eq!(
            Selector::parse(&Value::Table(Table::new()), &groups).expect_err("an empty table"),
            "a selector table must name exactly one of `dependents-of`, \
             `dependencies-of` and `reachable-from`"
        );
    }

    #[test]
    fn pair_selectors_yield_ordered_pairs_without_self_pairs() {
        let groups = groups(&[("layers", &["facade", "core"])]);
        let graph = layer_graph();
        let selection =
            select("from = \"app\"\nto = \"@layers\"", &groups).expect("a pair selector");
        let Select::Pair { from, to } = &selection else {
            panic!("a pair selection");
        };
        assert_eq!(selection.to_string(), "{ from = \"app\", to = @layers }");
        assert_eq!(
            resolve_pairs(from, to, &graph),
            [
                ("app".to_owned(), "core".to_owned()),
                ("app".to_owned(), "facade".to_owned()),
            ]
        );

        let self_pairs = select("from = \"app\"\nto = \"app\"", &groups).expect("a pair selector");
        let Select::Pair { from, to } = &self_pairs else {
            panic!("a pair selection");
        };
        assert!(resolve_pairs(from, to, &graph).is_empty());
    }

    #[test]
    fn pair_tables_need_both_ends() {
        let groups = groups(&[]);
        assert!(
            select("from = \"a\"", &groups)
                .expect_err("a missing end")
                .contains("needs both `from` and `to`")
        );
        assert!(
            select("to = \"a\"", &groups)
                .expect_err("a missing end")
                .contains("needs both `from` and `to`")
        );
        assert!(
            select("from = \"a\"\nto = \"b\"\nvia = \"c\"", &groups)
                .expect_err("an extra key")
                .contains("unknown pair selector key `via`")
        );
        assert!(
            select("from = { dependents-of = \"a\" }\nto = \"b\"", &groups)
                .expect_err("a nested table")
                .contains("expected a name selector")
        );
    }
}
