//! The `depform.toml` schema and its validation.
//!
//! The serde skeleton only fixes the table names and rejects unknown
//! top-level keys. `select` and `check` are hand-parsed because they are small
//! expression languages whose errors must name the offending rule, and because
//! every unknown key below the skeleton is rejected here as a hard error.
//!
//! The selector sub-language lives in [`selector`]; this module owns the
//! document, rule structure and file loading.

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

use self::selector::Select;
pub(crate) use self::selector::Selector;
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
        let mut rules = Vec::with_capacity(file.rules.len());
        for (name, rule) in file.rules {
            rules.push(RuleConfig::parse(&name, rule)?);
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
            },
            rules,
        }
    }

    /// Select the declaration kinds of the checked view.
    pub(crate) fn with_kinds(mut self, kinds: impl IntoIterator<Item = DependencyKind>) -> Self {
        self.graph.kinds = kinds.into_iter().collect();
        self
    }
}

/// The `[graph]` table: which declaration kinds form the checked view.
#[derive(Debug)]
struct GraphSettings {
    kinds: BTreeSet<DependencyKind>,
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
}

/// The selection and checks of a node rule.
#[derive(Debug)]
pub(crate) struct NodeRule {
    pub(crate) select: Selector,
    pub(crate) dependencies: Option<SetCheck>,
}

impl RuleConfig {
    /// Reject every explicit selector of the rule that matches no package.
    pub(crate) fn validate(&self, graph: &Graph) -> Result<(), DepformError> {
        match &self.kind {
            RuleKind::Node(rule) => {
                rule.select.validate_explicit(graph, &self.name)?;
                for check in [&rule.dependencies].into_iter().flatten() {
                    for selector in check.selectors() {
                        selector.validate_explicit(graph, &self.name)?;
                    }
                }
                Ok(())
            }
        }
    }
}

#[cfg(test)]
impl RuleConfig {
    /// A node rule, edited before it is returned.
    pub(crate) fn node_with(
        name: &str,
        select: Selector,
        edit: impl FnOnce(&mut NodeRule),
    ) -> Self {
        let mut rule = NodeRule {
            select,
            dependencies: None,
        };
        edit(&mut rule);
        Self {
            name: name.to_owned(),
            kind: RuleKind::Node(Box::new(rule)),
        }
    }

    /// The node half of a node rule.
    fn node_rule(&self) -> &NodeRule {
        match &self.kind {
            RuleKind::Node(rule) => rule,
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

/// The top-level document: `[graph]` and `[rules]` tables.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ConfigFile {
    graph: GraphFile,
    rules: BTreeMap<String, RuleFile>,
}

/// The `[graph]` table as written.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct GraphFile {
    #[serde(rename = "dependency-kinds")]
    dependency_kinds: Option<Vec<String>>,
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
        Ok(Self { kinds })
    }
}

impl RuleConfig {
    fn parse(name: &str, rule: RuleFile) -> Result<Self, DepformError> {
        if !is_identifier(name) {
            return Err(config_error(format!(
                "invalid rule name {name:?}: use ASCII alphanumerics, `_` and `-`"
            )));
        }
        let RuleFile { select, check } = rule;
        let check = check
            .as_table()
            .ok_or_else(|| rule_error(name, "`check` must be a table"))?;
        let checks = Checks::parse(name, check)?;
        let select = match select {
            Some(value) => Some(Select::parse(&value).map_err(|detail| rule_error(name, detail))?),
            None => None,
        };
        let kind = match select {
            None => return Err(rule_error(name, "the rule needs a `select` selector")),
            Some(Select::Node(select)) => {
                if !checks.has_node() {
                    return Err(rule_error(name, "the rule needs at least one check"));
                }
                RuleKind::Node(Box::new(NodeRule {
                    select,
                    dependencies: checks.dependencies,
                }))
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
}

impl Checks {
    fn parse(name: &str, table: &Table) -> Result<Self, DepformError> {
        let mut checks = Self::default();
        for (key, value) in table {
            match key.as_str() {
                "dependencies" => {
                    checks.dependencies = Some(SetCheck::parse(name, "dependency", value)?);
                }
                other => return Err(rule_error(name, format!("unknown check `{other}`"))),
            }
        }
        Ok(checks)
    }

    /// Node checks apply to one package at a time.
    fn has_node(&self) -> bool {
        self.dependencies.is_some()
    }
}

impl SetCheck {
    fn parse(rule: &str, label: &str, value: &Value) -> Result<Self, DepformError> {
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
            *slot = Some(parse_selector(rule, value)?);
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
fn parse_selector(rule: &str, value: &Value) -> Result<Selector, DepformError> {
    Selector::parse(value).map_err(|detail| rule_error(rule, detail))
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
