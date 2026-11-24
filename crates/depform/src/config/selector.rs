//! Package selectors.
//!
//! A [`Selector`] resolves to a set of package names, serving rule selections
//! and the set checks. Parsing, rendering and empty-selection validation live
//! here. Matching is purely textual: `*` is the only wildcard and regular
//! expressions are never used.

use std::{
    collections::BTreeSet,
    fmt::{Display, Formatter, Result as FmtResult},
};

use toml::Value;

use crate::{DepformError, Graph};

/// A set of packages selected by a config rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Selector {
    /// A package-name pattern; `*` matches any run of characters.
    Name(String),
}

/// A rule's `select`: the packages a node rule checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Select {
    /// A selection of packages, used by node checks.
    Node(Selector),
}

impl Selector {
    /// Parse one selector value.
    pub(crate) fn parse(value: &Value) -> Result<Self, String> {
        match value {
            Value::String(text) => Self::parse_text(text),
            other => Err(format!("expected a selector, found {other}")),
        }
    }

    /// Parse a string selector: a name pattern.
    fn parse_text(text: &str) -> Result<Self, String> {
        validate_pattern(text)?;
        Ok(Self::Name(text.to_owned()))
    }

    /// The selected packages, sorted.
    pub(crate) fn resolve(&self, graph: &Graph) -> BTreeSet<String> {
        match self {
            Self::Name(pattern) => selected(graph, |package| matches_pattern(pattern, package)),
        }
    }

    /// Reject a selector whose explicit name leaves match nothing.
    ///
    /// A pattern that selects no package is almost certainly a typo, so it is
    /// a config error rather than a rule that silently holds.
    pub(crate) fn validate_explicit(&self, graph: &Graph, rule: &str) -> Result<(), DepformError> {
        match self {
            Self::Name(_) => {
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
    /// Parse a rule's `select`: a package selector.
    pub(crate) fn parse(value: &Value) -> Result<Self, String> {
        Ok(Self::Node(Selector::parse(value)?))
    }
}

impl Display for Selector {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        match self {
            Self::Name(pattern) => write!(f, "{pattern:?}"),
        }
    }
}

impl Display for Select {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        match self {
            Self::Node(selector) => selector.fmt(f),
        }
    }
}

/// The packages of `graph` that `matches` accepts, sorted.
fn selected(graph: &Graph, matches: impl Fn(&str) -> bool) -> BTreeSet<String> {
    graph
        .packages()
        .filter(|package| matches(package))
        .map(str::to_owned)
        .collect()
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
    use toml::Table;

    use super::*;

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
    fn name_selectors_parse_strings() {
        let value = Value::String("core*".to_owned());
        assert!(Selector::parse(&value).is_ok());
        assert!(Selector::parse(&Value::Integer(1)).is_err());
        assert!(Selector::parse(&Value::Array(Vec::new())).is_err());
        assert!(Selector::parse(&Value::Table(Table::new())).is_err());
    }
}
