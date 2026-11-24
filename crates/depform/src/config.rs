//! The `depform.toml` schema and its validation.
//!
//! The serde skeleton only fixes the table names and rejects unknown
//! top-level keys. `select` and `check` are hand-parsed because they are small
//! expression languages whose errors must name the offending rule, and because
//! every unknown key below the skeleton is rejected here as a hard error.
//!
//! The sub-languages live in [`selector`] and [`range`]; this module owns the
//! document, rule structure and file loading.

mod range;
mod selector;

#[cfg(test)]
mod tests;

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::ErrorKind,
    path::Path,
};

use serde::Deserialize;
use toml::{Table, Value};

use self::selector::{Groups, Select};
pub(crate) use self::{
    range::Range,
    selector::{Selector, resolve_pairs},
};
use crate::{DependencyKind, DepformError, Graph};

/// A parsed and validated `depform.toml`.
#[derive(Debug)]
pub struct Config {
    graph: GraphSettings,
    rules: Vec<RuleConfig>,
}

impl Config {
    /// Parse and validate a config document.
    pub fn from_toml(text: &str) -> Result<Self, DepformError> {
        let file: ConfigFile =
            toml::from_str(text).map_err(|error| DepformError::Config(error.to_string()))?;
        let graph = GraphSettings::parse(&file.graph)?;
        let groups = Groups::new(file.groups)?;
        let mut rules = Vec::with_capacity(file.rules.len());
        for (name, rule) in file.rules {
            rules.push(RuleConfig::parse(&name, rule, &groups)?);
        }
        Ok(Self { graph, rules })
    }

    /// Load a workspace's config from `config_path`, or from
    /// `<root>/depform.toml` when no explicit path is given.
    ///
    /// A missing default config is [`DepformError::MissingConfig`]; a missing
    /// or unreadable explicit path is an I/O error.
    pub(crate) fn load(root: &Path, config_path: Option<&Path>) -> Result<Self, DepformError> {
        let path = match config_path {
            Some(path) => path.to_path_buf(),
            None => root.join("depform.toml"),
        };
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(source) if config_path.is_none() && source.kind() == ErrorKind::NotFound => {
                return Err(DepformError::MissingConfig { path });
            }
            Err(source) => return Err(DepformError::ReadConfig { path, source }),
        };
        Self::from_toml(&text)
    }

    /// The dependency kinds that participate in the checked view.
    pub(crate) fn kinds(&self) -> &BTreeSet<DependencyKind> {
        &self.graph.kinds
    }

    /// The rules, ordered by name.
    pub(crate) fn rules(&self) -> &[RuleConfig] {
        &self.rules
    }

    /// The declared roots, or `None` when they are inferred from the view.
    pub(crate) fn roots(&self) -> Option<&[String]> {
        self.graph.roots.as_deref()
    }
}

#[cfg(test)]
impl Config {
    /// Assemble a config from already-built rules.
    ///
    /// Tests of rule evaluation use this instead of a document, so a parse
    /// failure cannot masquerade as an evaluation failure. The document
    /// schema has its own tests in this module.
    pub(crate) fn from_rules(rules: Vec<RuleConfig>) -> Self {
        Self {
            graph: GraphSettings {
                kinds: BTreeSet::from([DependencyKind::Normal]),
                roots: None,
            },
            rules,
        }
    }

    /// Select the declaration kinds of the checked view.
    pub(crate) fn with_kinds(mut self, kinds: impl IntoIterator<Item = DependencyKind>) -> Self {
        self.graph.kinds = kinds.into_iter().collect();
        self
    }

    /// Declare the roots depth is measured from.
    pub(crate) fn with_roots(mut self, roots: impl IntoIterator<Item = &'static str>) -> Self {
        self.graph.roots = Some(roots.into_iter().map(str::to_owned).collect());
        self
    }
}

/// The `[graph]` table: which declaration kinds form the checked view.
#[derive(Debug)]
struct GraphSettings {
    kinds: BTreeSet<DependencyKind>,
    roots: Option<Vec<String>>,
}

/// One `[rules.NAME]` table.
#[derive(Debug)]
pub(crate) struct RuleConfig {
    pub(crate) name: String,
    pub(crate) kind: RuleKind,
}

/// What a rule asserts, which fixes the shape of its selection.
#[derive(Debug)]
pub(crate) enum RuleKind {
    /// Checks that apply to one package at a time.
    Node(Box<NodeRule>),
    /// Checks that apply to ordered pairs of packages.
    Pair(PairRule),
}

/// The selection and checks of a node rule.
#[derive(Debug)]
pub(crate) struct NodeRule {
    pub(crate) select: Selector,
    pub(crate) dependencies: Option<SetCheck>,
    pub(crate) reachable: Option<SetCheck>,
    pub(crate) depth: Option<Range>,
    pub(crate) height: Option<Range>,
    pub(crate) dependency_count: Option<Range>,
    pub(crate) dependent_count: Option<Range>,
}

/// The endpoints and checks of a pair rule.
#[derive(Debug)]
pub(crate) struct PairRule {
    pub(crate) from: Selector,
    pub(crate) to: Selector,
    pub(crate) distance: Option<Range>,
}

impl RuleConfig {
    /// Whether the rule needs the depth search rooted at the graph roots.
    pub(crate) fn needs_roots(&self) -> bool {
        matches!(&self.kind, RuleKind::Node(rule) if rule.depth.is_some())
    }

    /// Reject every explicit selector of the rule that matches no package.
    pub(crate) fn validate(&self, graph: &Graph) -> Result<(), DepformError> {
        match &self.kind {
            RuleKind::Node(rule) => {
                rule.select.validate_explicit(graph, &self.name)?;
                for check in [&rule.dependencies, &rule.reachable].into_iter().flatten() {
                    for selector in check.selectors() {
                        selector.validate_explicit(graph, &self.name)?;
                    }
                }
                Ok(())
            }
            RuleKind::Pair(rule) => {
                rule.from.validate_explicit(graph, &self.name)?;
                rule.to.validate_explicit(graph, &self.name)?;
                Ok(())
            }
        }
    }
}

#[cfg(test)]
impl RuleConfig {
    /// A node rule with no checks.
    pub(crate) fn node(name: &str, select: Selector) -> Self {
        Self::node_with(name, select, |_| {})
    }

    /// A node rule, edited before it is returned.
    pub(crate) fn node_with(
        name: &str,
        select: Selector,
        edit: impl FnOnce(&mut NodeRule),
    ) -> Self {
        let mut rule = NodeRule {
            select,
            dependencies: None,
            reachable: None,
            depth: None,
            height: None,
            dependency_count: None,
            dependent_count: None,
        };
        edit(&mut rule);
        Self {
            name: name.to_owned(),
            kind: RuleKind::Node(Box::new(rule)),
        }
    }

    /// A pair rule, edited before it is returned.
    pub(crate) fn pair_with(
        name: &str,
        from: Selector,
        to: Selector,
        edit: impl FnOnce(&mut PairRule),
    ) -> Self {
        let mut rule = PairRule {
            from,
            to,
            distance: None,
        };
        edit(&mut rule);
        Self {
            name: name.to_owned(),
            kind: RuleKind::Pair(rule),
        }
    }

    /// The node half of a node rule.
    fn node_rule(&self) -> &NodeRule {
        match &self.kind {
            RuleKind::Node(rule) => rule,
            other => panic!("expected a node rule, found {other:?}"),
        }
    }

    /// The pair half of a pair rule.
    fn pair_rule(&self) -> &PairRule {
        match &self.kind {
            RuleKind::Pair(rule) => rule,
            other => panic!("expected a pair rule, found {other:?}"),
        }
    }
}

/// One set check: `allow`, `deny` and `require` selectors, at least one.
#[derive(Debug)]
pub(crate) struct SetCheck {
    pub(crate) allow: Option<Selector>,
    pub(crate) deny: Option<Selector>,
    pub(crate) require: Option<Selector>,
}

impl SetCheck {
    /// Every selector the check mentions.
    pub(crate) fn selectors(&self) -> impl Iterator<Item = &Selector> {
        [
            self.allow.as_ref(),
            self.deny.as_ref(),
            self.require.as_ref(),
        ]
        .into_iter()
        .flatten()
    }

    /// Whether the check asserts nothing.
    fn is_empty(&self) -> bool {
        self.allow.is_none() && self.deny.is_none() && self.require.is_none()
    }
}

/// The top-level document: `[graph]`, `[groups]` and `[rules]` tables.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ConfigFile {
    graph: GraphFile,
    groups: BTreeMap<String, Vec<String>>,
    rules: BTreeMap<String, RuleFile>,
}

/// The `[graph]` table as written.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct GraphFile {
    #[serde(rename = "dependency-kinds")]
    dependency_kinds: Option<Vec<String>>,
    roots: Option<Vec<String>>,
}

/// One rule as written: the selector and the check table are still untyped.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RuleFile {
    select: Option<Value>,
    check: Value,
}

impl GraphSettings {
    fn parse(file: &GraphFile) -> Result<Self, DepformError> {
        let kinds = match &file.dependency_kinds {
            None => BTreeSet::from([DependencyKind::Normal]),
            Some(values) => {
                if values.is_empty() {
                    return Err(config_error(
                        "[graph] dependency-kinds must list at least one kind",
                    ));
                }
                let mut kinds = BTreeSet::new();
                for value in values {
                    let kind = match value.as_str() {
                        "normal" => DependencyKind::Normal,
                        "dev" => DependencyKind::Dev,
                        "build" => DependencyKind::Build,
                        other => {
                            return Err(config_error(format!(
                                "[graph] dependency-kinds names unknown kind `{other}`; \
                                 use `normal`, `dev` or `build`"
                            )));
                        }
                    };
                    if !kinds.insert(kind) {
                        return Err(config_error(format!(
                            "[graph] dependency-kinds repeats `{value}`"
                        )));
                    }
                }
                kinds
            }
        };
        let roots = match &file.roots {
            None => None,
            Some(roots) => {
                if roots.is_empty() {
                    return Err(config_error("[graph] roots must name at least one package"));
                }
                let mut seen = BTreeSet::new();
                for root in roots {
                    if root.is_empty() {
                        return Err(config_error("[graph] roots must not name an empty package"));
                    }
                    if !seen.insert(root) {
                        return Err(config_error(format!("[graph] roots repeats `{root}`")));
                    }
                }
                Some(roots.clone())
            }
        };
        Ok(Self { kinds, roots })
    }
}

impl RuleConfig {
    fn parse(name: &str, rule: RuleFile, groups: &Groups) -> Result<Self, DepformError> {
        if !is_identifier(name) {
            return Err(config_error(format!(
                "invalid rule name {name:?}: use ASCII alphanumerics, `_` and `-`"
            )));
        }
        let RuleFile { select, check } = rule;
        let check = check
            .as_table()
            .ok_or_else(|| rule_error(name, "`check` must be a table"))?;
        let checks = Checks::parse(name, check, groups)?;
        let select = match select {
            Some(value) => {
                Some(Select::parse(&value, groups).map_err(|detail| rule_error(name, detail))?)
            }
            None => None,
        };
        let kind = match select {
            None => return Err(rule_error(name, "the rule needs a `select` selector")),
            Some(Select::Node(select)) => {
                if checks.has_pair() {
                    return Err(rule_error(
                        name,
                        "a pair check needs a pair selector with `from` and `to`",
                    ));
                }
                if !checks.has_node() {
                    return Err(rule_error(name, "the rule needs at least one check"));
                }
                RuleKind::Node(Box::new(NodeRule {
                    select,
                    dependencies: checks.dependencies,
                    reachable: checks.reachable,
                    depth: checks.depth,
                    height: checks.height,
                    dependency_count: checks.dependency_count,
                    dependent_count: checks.dependent_count,
                }))
            }
            Some(Select::Pair { from, to }) => {
                if checks.has_node() && checks.has_pair() {
                    return Err(rule_error(
                        name,
                        "a rule cannot mix node checks and pair checks",
                    ));
                }
                if checks.has_node() {
                    return Err(rule_error(
                        name,
                        "a pair selector can only be used with pair checks",
                    ));
                }
                if !checks.has_pair() {
                    return Err(rule_error(name, "the rule needs at least one check"));
                }
                RuleKind::Pair(PairRule {
                    from,
                    to,
                    distance: checks.distance,
                })
            }
        };
        Ok(Self {
            name: name.to_owned(),
            kind,
        })
    }
}

/// The checks of one rule, before the selection fixes the rule's kind.
#[derive(Debug, Default)]
struct Checks {
    dependencies: Option<SetCheck>,
    reachable: Option<SetCheck>,
    depth: Option<Range>,
    height: Option<Range>,
    dependency_count: Option<Range>,
    dependent_count: Option<Range>,
    distance: Option<Range>,
}

impl Checks {
    fn parse(name: &str, table: &Table, groups: &Groups) -> Result<Self, DepformError> {
        let mut checks = Self::default();
        for (key, value) in table {
            match key.as_str() {
                "dependencies" => {
                    checks.dependencies = Some(SetCheck::parse(name, "dependency", value, groups)?);
                }
                "reachable" => {
                    checks.reachable = Some(SetCheck::parse(name, "reachable", value, groups)?);
                }
                "depth" => checks.depth = Some(parse_range(name, "depth", value)?),
                "height" => checks.height = Some(parse_range(name, "height", value)?),
                "dependency-count" => {
                    checks.dependency_count = Some(parse_range(name, "dependency-count", value)?);
                }
                "dependent-count" => {
                    checks.dependent_count = Some(parse_range(name, "dependent-count", value)?);
                }
                "distance" => checks.distance = Some(parse_range(name, "distance", value)?),
                other => return Err(rule_error(name, format!("unknown check `{other}`"))),
            }
        }
        Ok(checks)
    }

    /// Node checks apply to one package at a time.
    fn has_node(&self) -> bool {
        self.dependencies.is_some()
            || self.reachable.is_some()
            || self.depth.is_some()
            || self.height.is_some()
            || self.dependency_count.is_some()
            || self.dependent_count.is_some()
    }

    /// Pair checks apply to an ordered pair of packages.
    fn has_pair(&self) -> bool {
        self.distance.is_some()
    }
}

impl SetCheck {
    fn parse(
        rule: &str,
        label: &str,
        value: &Value,
        groups: &Groups,
    ) -> Result<Self, DepformError> {
        let table = value
            .as_table()
            .ok_or_else(|| rule_error(rule, format!("a {label} check must be a table")))?;
        let mut check = Self {
            allow: None,
            deny: None,
            require: None,
        };
        for (key, value) in table {
            let slot = match key.as_str() {
                "allow" => &mut check.allow,
                "deny" => &mut check.deny,
                "require" => &mut check.require,
                other => {
                    return Err(rule_error(
                        rule,
                        format!("unknown {label} check key `{other}`"),
                    ));
                }
            };
            *slot = Some(parse_selector(rule, value, groups)?);
        }
        if check.is_empty() {
            return Err(rule_error(
                rule,
                format!("a {label} check needs at least one of `allow`, `deny` and `require`"),
            ));
        }
        Ok(check)
    }
}

/// Parse a selector value in the context of one rule.
fn parse_selector(rule: &str, value: &Value, groups: &Groups) -> Result<Selector, DepformError> {
    Selector::parse(value, groups).map_err(|detail| rule_error(rule, detail))
}

/// Parse a range value in the context of one rule and check key.
fn parse_range(rule: &str, key: &str, value: &Value) -> Result<Range, DepformError> {
    Range::parse(value).map_err(|detail| rule_error(rule, format!("`{key}`: {detail}")))
}

/// Build a config error that names one rule.
fn rule_error(rule: &str, detail: impl Into<String>) -> DepformError {
    config_error(format!("rule `{rule}`: {}", detail.into()))
}

/// Build a config error without a rule context.
fn config_error(detail: impl Into<String>) -> DepformError {
    DepformError::Config(detail.into())
}

/// Whether `name` is a non-empty run of identifier characters.
///
/// Rule names and group names use the same alphabet, so their parsers share
/// the check.
fn is_identifier(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
}
