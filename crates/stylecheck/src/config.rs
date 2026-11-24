//! The single config file shared by every style rule's options.
//!
//! `stylecheck.toml` is the one config for all style options; each rule reads
//! its own section from it, keyed by the rule's canonical name. A rule with no
//! options needs no section. A missing file or section is valid (built-in
//! defaults apply); malformed TOML or schema is a hard [`ConfigError`]. An
//! unknown top-level section is tolerated so a future rule can add its section
//! without breaking older checkers, but a known section rejects unknown keys:
//! a misspelled option must be a hard error, never a silent default.
//!
//! The config also carries the two facts no rule may hard-code: which trees the
//! walk descends into (`scan-paths`) and how the root workspace dependency
//! aspects are named, ordered and recognized
//! (`[[dependency-group-order.aspects]]`). Rule severities live in the
//! `[levels]` map: every rule defaults to the level its registry entry
//! declares, and a `[levels]` key naming a rule overrides it — `warn` keeps
//! the run passing, `error` fails it, `off` disables the rule. A key that
//! names no known rule, or a value other than those three, is a hard error,
//! never a silent default.

use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;
use thiserror::Error;

use crate::rules::{ExpectOverAllowPolicy, Level, QualifiedNamePolicy, Rule};

/// Configuration parse failure; reported as a hard error by the command.
#[derive(Debug, Error)]
pub enum ConfigError {
    /// The style config is not valid TOML or does not match the schema.
    #[error("invalid style config: {detail}")]
    InvalidConfig {
        /// The TOML parse or schema diagnostic.
        detail: String,
    },
}

/// The on-disk `stylecheck.toml` schema. One section per rule, keyed by the
/// rule's canonical name; a rule with no options contributes no section.
/// Unknown sections are tolerated so future rules can add theirs without
/// breaking older checkers; a known section rejects unknown keys, so a
/// misspelled option fails loudly instead of silently disabling its rule.
#[derive(Debug, Default, Deserialize)]
struct StyleFile {
    /// Trees below the scanned root the walk descends into. Empty means the
    /// scanned root itself.
    #[serde(rename = "scan-paths", default)]
    scan_paths: Vec<String>,
    /// Path fragments whose files the path-sensitive rules skip.
    #[serde(rename = "exempt-paths", default)]
    exempt_paths: Vec<String>,
    /// Rule canonical name to configured level. Keys are validated against the
    /// registry, so a misspelled rule name cannot silently keep its default.
    #[serde(default)]
    levels: BTreeMap<String, String>,
    #[serde(rename = "qualified-name", default)]
    qualified: PolicyTable,
    #[serde(rename = "expect-over-allow", default)]
    expect_over_allow: PolicyTable,
    #[serde(rename = "dependency-group-order", default)]
    dependency_group_order: DependencyGroupOrderTable,
    /// Top-level keys the schema does not know. A table (or a non-empty array
    /// of tables) is a future rule section and stays tolerated; any other
    /// value is a misspelled option and must fail loudly.
    #[serde(flatten)]
    unknown: BTreeMap<String, toml::Value>,
}

/// The `[qualified-name]` and `[expect-over-allow]` sections share this shape:
/// canonical spellings the rule allows or re-denies. `"P::*"` is a prefix
/// pattern, `"*::P"` a suffix pattern; anything else is exact.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct PolicyTable {
    whitelist: Vec<String>,
    blacklist: Vec<String>,
}

/// The `[dependency-group-order]` section. Its `aspects` array is ordered: the
/// root workspace dependency table must present the aspects in this order
/// before any third-party dependency.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct DependencyGroupOrderTable {
    aspects: Vec<AspectTable>,
}

/// One declared root dependency aspect and the workspace paths it covers.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AspectTable {
    name: String,
    paths: Vec<String>,
}

/// Reject an unknown top-level key unless it is shaped like a future rule
/// section: a table or a non-empty array of tables. A scalar or a plain array
/// is a misspelled option — silently ignoring it would change the check
/// without telling the user.
fn reject_unknown_options(unknown: &BTreeMap<String, toml::Value>) -> Result<(), ConfigError> {
    for (key, value) in unknown {
        let is_section = match value {
            toml::Value::Table(_) => true,
            toml::Value::Array(items) => {
                !items.is_empty() && items.iter().all(toml::Value::is_table)
            }
            _ => false,
        };
        if !is_section {
            return Err(ConfigError::InvalidConfig {
                detail: format!(
                    "unknown top-level option `{key}`; expected one of the documented options or a rule section"
                ),
            });
        }
    }
    Ok(())
}

/// Resolve one `[levels]` entry. The key must name a known rule and the value
/// must be a level spelling: a misspelled rule name would otherwise silently
/// keep the rule's default level, and a misspelled value silently change
/// nothing.
fn parse_level(name: &str, value: &str) -> Result<(Rule, Level), ConfigError> {
    let rule = Rule::ALL
        .iter()
        .copied()
        .find(|rule| rule.name() == name)
        .ok_or_else(|| ConfigError::InvalidConfig {
            detail: format!("unknown rule `{name}` in [levels]"),
        })?;
    let level = match value {
        "off" => Level::Off,
        "warn" => Level::Warn,
        "error" => Level::Error,
        _ => {
            return Err(ConfigError::InvalidConfig {
                detail: format!(
                    "rule `{name}` has an invalid level `{value}`; expected `off`, `warn` or `error`"
                ),
            });
        }
    };
    Ok((rule, level))
}

/// One root workspace dependency aspect: a display name and the ordered path
/// prefixes that assign a workspace member to it.
#[derive(Debug, Clone)]
pub(crate) struct DependencyAspect {
    name: String,
    paths: Vec<String>,
}

impl DependencyAspect {
    /// Validate one declared aspect. An empty name cannot label a report, and
    /// an empty prefix would silently claim every path, so both are rejected.
    fn from_table(table: AspectTable) -> Result<Self, ConfigError> {
        if table.name.trim().is_empty() {
            return Err(ConfigError::InvalidConfig {
                detail: "a dependency aspect needs a non-empty name".to_owned(),
            });
        }
        if table.paths.is_empty() {
            return Err(ConfigError::InvalidConfig {
                detail: format!(
                    "dependency aspect `{}` must declare at least one path prefix",
                    table.name
                ),
            });
        }
        if table.paths.iter().any(|path| path.is_empty()) {
            return Err(ConfigError::InvalidConfig {
                detail: format!(
                    "dependency aspect `{}` declares an empty path prefix",
                    table.name
                ),
            });
        }
        Ok(Self {
            name: table.name,
            paths: table.paths,
        })
    }

    /// The aspect name used in reports.
    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    /// Whether a declared workspace path belongs to this aspect.
    fn matches(&self, path: &str) -> bool {
        self.paths.iter().any(|prefix| path.starts_with(prefix))
    }
}

/// One config carrying every style rule's options, parsed from the
/// `stylecheck.toml` at the scanned root. The `qualified-name` rule's options
/// live in the `[qualified-name]` section and the `expect-over-allow` rule's in
/// the `[expect-over-allow]` section; the top-level `exempt-paths` array
/// lists the paths the path-sensitive rules skip; `scan-paths` lists the trees
/// the walk descends into; `[[dependency-group-order.aspects]]` declares the
/// root dependency aspects in their required order; the `[levels]` map
/// overrides rule severities, which otherwise default to each rule's
/// registered level.
#[derive(Debug, Clone)]
pub struct StyleConfig {
    scan_paths: Vec<String>,
    exempt_paths: Vec<String>,
    levels: BTreeMap<Rule, Level>,
    qualified: QualifiedNamePolicy,
    expect_over_allow: ExpectOverAllowPolicy,
    aspects: Vec<DependencyAspect>,
}

impl Default for StyleConfig {
    fn default() -> Self {
        Self {
            scan_paths: vec![".".to_owned()],
            exempt_paths: Vec::new(),
            levels: BTreeMap::new(),
            qualified: QualifiedNamePolicy::default(),
            expect_over_allow: ExpectOverAllowPolicy::default(),
            aspects: Vec::new(),
        }
    }
}

impl StyleConfig {
    /// Parse the `stylecheck.toml` at the scanned root into the one config for
    /// all style options. A missing section is valid (defaults apply);
    /// malformed TOML or schema is a hard [`ConfigError`]. The returned config
    /// carries no external-crate exemptions; a caller that runs a rule
    /// directly must also call [`Self::with_external_crates`], or the
    /// `qualified-name` rule denies the standard prelude.
    pub fn from_toml(config_text: &str) -> Result<Self, ConfigError> {
        let file: StyleFile =
            toml::from_str(config_text).map_err(|error| ConfigError::InvalidConfig {
                detail: error.to_string(),
            })?;
        reject_unknown_options(&file.unknown)?;
        if file.exempt_paths.iter().any(String::is_empty) {
            return Err(ConfigError::InvalidConfig {
                detail: "`exempt-paths` must not contain an empty entry".to_owned(),
            });
        }
        let qualified =
            QualifiedNamePolicy::from_lists(file.qualified.whitelist, file.qualified.blacklist)?;
        let expect_over_allow = ExpectOverAllowPolicy::from_lists(
            file.expect_over_allow.whitelist,
            file.expect_over_allow.blacklist,
        )?;
        let levels = file
            .levels
            .iter()
            .map(|(name, value)| parse_level(name, value))
            .collect::<Result<BTreeMap<_, _>, _>>()?;
        let aspects = file
            .dependency_group_order
            .aspects
            .into_iter()
            .map(DependencyAspect::from_table)
            .collect::<Result<Vec<_>, _>>()?;
        let scan_paths = if file.scan_paths.is_empty() {
            vec![".".to_owned()]
        } else {
            if file.scan_paths.iter().any(|path| path.is_empty()) {
                return Err(ConfigError::InvalidConfig {
                    detail: "`scan-paths` must not contain an empty entry".to_owned(),
                });
            }
            file.scan_paths
        };
        Ok(Self {
            scan_paths,
            exempt_paths: file.exempt_paths,
            levels,
            qualified,
            expect_over_allow,
            aspects,
        })
    }

    /// Whether `path` is exempt from the path-sensitive rules: the path
    /// contains one of the configured `exempt-paths` fragments. A tree exempts
    /// its trybuild programs, for instance, because such a program is a
    /// fragment whose doc comments introduce deliberately failing code.
    pub fn is_exempt(&self, path: &str) -> bool {
        self.exempt_paths
            .iter()
            .any(|fragment| path.contains(fragment.as_str()))
    }

    /// The `qualified-name` rule's allow/deny policy.
    pub fn qualified(&self) -> &QualifiedNamePolicy {
        &self.qualified
    }

    /// The `expect-over-allow` rule's lint-name allow/deny policy.
    pub fn expect_over_allow(&self) -> &ExpectOverAllowPolicy {
        &self.expect_over_allow
    }

    /// The severity `rule` reports with: its configured level, else the
    /// default level the rule's registry entry declares.
    pub fn level_of(&self, rule: Rule) -> Level {
        self.levels
            .get(&rule)
            .copied()
            .unwrap_or(rule.default_level())
    }

    /// Set the built-in external-crate exemption set.
    pub fn with_external_crates(mut self, external_crates: BTreeSet<String>) -> Self {
        self.qualified = self.qualified.with_external_crates(external_crates);
        self
    }

    /// The trees below the scanned root the walk descends into, relative to
    /// that root.
    pub fn scan_paths(&self) -> &[String] {
        &self.scan_paths
    }

    /// The declared root dependency aspects in their required order.
    pub(crate) fn dependency_aspects(&self) -> &[DependencyAspect] {
        &self.aspects
    }

    /// The index of the first declared aspect whose prefix matches `path`.
    pub(crate) fn aspect_of_path(&self, path: &str) -> Option<usize> {
        self.aspects.iter().position(|aspect| aspect.matches(path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aspects_keep_their_declared_order() {
        let config = StyleConfig::from_toml(
            r#"[[dependency-group-order.aspects]]
name = "alpha"
paths = ["crates/alpha/"]

[[dependency-group-order.aspects]]
name = "beta"
paths = ["crates/beta/", "crates/shared/"]
"#,
        )
        .expect("the aspect declarations parse");
        let aspects = config.dependency_aspects();
        assert_eq!(aspects.len(), 2);
        assert_eq!(aspects[0].name(), "alpha");
        assert_eq!(aspects[1].name(), "beta");
        assert_eq!(config.aspect_of_path("crates/beta/lib"), Some(1));
        assert_eq!(config.aspect_of_path("crates/shared/lib"), Some(1));
        assert_eq!(config.aspect_of_path("crates/unknown/lib"), None);
    }

    #[test]
    fn empty_aspect_name_or_path_prefix_is_rejected() {
        for text in [
            "[[dependency-group-order.aspects]]\nname = \"\"\npaths = [\"crates/a/\"]\n",
            "[[dependency-group-order.aspects]]\nname = \"a\"\npaths = []\n",
            "[[dependency-group-order.aspects]]\nname = \"a\"\npaths = [\"\"]\n",
        ] {
            assert!(StyleConfig::from_toml(text).is_err(), "{text:?}");
        }
    }

    #[test]
    fn scan_paths_default_to_the_scanned_root() {
        assert_eq!(StyleConfig::default().scan_paths(), ["."]);
        assert_eq!(
            StyleConfig::from_toml("scan-paths = [\"src\"]\n")
                .expect("scan paths parse")
                .scan_paths(),
            ["src"]
        );
        // An empty entry would silently widen the walk to the scanned root.
        assert!(StyleConfig::from_toml("scan-paths = [\"\"]\n").is_err());
    }

    #[test]
    fn levels_override_defaults_and_reject_unknown_entries() {
        let config = StyleConfig::from_toml(
            "[levels]\nmod-before-use = \"warn\"\nqualified-name = \"off\"\n",
        )
        .expect("levels parse");
        assert_eq!(config.level_of(Rule::ModBeforeUse), Level::Warn);
        assert_eq!(config.level_of(Rule::QualifiedName), Level::Off);
        // A rule with no entry keeps its registered default.
        assert_eq!(config.level_of(Rule::ModuleDocSpacing), Level::Error);
        // A misspelled rule name or level silently keeping the default is the
        // failure mode the validation exists to prevent.
        for text in [
            "[levels]\nmod-before-us = \"warn\"\n",
            "[levels]\nmod-before-use = \"fatal\"\n",
        ] {
            assert!(StyleConfig::from_toml(text).is_err(), "{text:?}");
        }
    }

    #[test]
    fn unknown_keys_in_a_known_section_are_rejected() {
        // Only whole unknown sections are tolerated; a misspelled key inside a
        // known section must fail loudly instead of silently disabling the
        // rule that section configures.
        for text in [
            "[qualified-name]\nwhite-list = [\"std::*\"]\n",
            "[dependency-group-order]\naspect = []\n",
            "[[dependency-group-order.aspects]]\nname = \"a\"\npath = [\"crates/a/\"]\n",
        ] {
            assert!(StyleConfig::from_toml(text).is_err(), "{text:?}");
        }
    }

    #[test]
    fn top_level_option_typos_are_rejected_but_sections_are_tolerated() {
        // A misspelled scalar or array option must not silently fall back to
        // its default.
        for text in [
            "scan-path = [\"crates\"]\n",
            "exempt-path = [\"tests/ui/\"]\n",
            "unknown = 1\n",
            "unknown = [\"value\"]\n",
        ] {
            assert!(StyleConfig::from_toml(text).is_err(), "{text:?}");
        }
        // A future rule's section remains accepted in both table spellings.
        for text in [
            "[future-rule]\noption = true\n",
            "[[future-rule]]\noption = true\n",
        ] {
            assert!(StyleConfig::from_toml(text).is_ok(), "{text:?}");
        }
    }

    #[test]
    fn empty_exempt_path_is_rejected() {
        // `path.contains("")` is always true, so an empty fragment would
        // silently exempt every source file from the path-sensitive rules.
        assert!(StyleConfig::from_toml("exempt-paths = [\"\"]\n").is_err());
    }

    #[test]
    fn expect_over_allow_config_parses_and_validates() {
        let config = StyleConfig::from_toml(
            "[expect-over-allow]\nwhitelist = [\"dead_code\", \"clippy::*\"]\nblacklist = [\"clippy::pedantic\"]\n",
        )
        .expect("the expect-over-allow policy parses");
        let policy = config.expect_over_allow();
        assert!(policy.allows("dead_code"));
        // The built-in allow-only set stays in force by default.
        assert!(policy.allows("unknown_lints"));
        // The whitelist wins over the blacklist.
        assert!(policy.allows("clippy::pedantic"));
        // Anything unmatched is reported.
        assert!(!policy.allows("dead_code_ish"));

        // A blacklist re-denies a name from the built-in allow-only set.
        let config = StyleConfig::from_toml(
            "[expect-over-allow]\nwhitelist = [\"dead_code\"]\nblacklist = [\"unknown_lints\"]\n",
        )
        .expect("the expect-over-allow policy parses");
        assert!(!config.expect_over_allow().allows("unknown_lints"));

        // An empty, whitespace-carrying or misplaced-wildcard spelling is a
        // hard error, exactly as in the `[qualified-name]` table, and so is a
        // misspelled key inside the known section.
        for text in [
            "[expect-over-allow]\nwhitelist = [\"\"]\n",
            "[expect-over-allow]\nwhitelist = [\"bad name\"]\n",
            "[expect-over-allow]\nblacklist = [\"a*b\"]\n",
            "[expect-over-allow]\nwhite-list = [\"dead_code\"]\n",
        ] {
            assert!(StyleConfig::from_toml(text).is_err(), "{text:?}");
        }
    }
}
