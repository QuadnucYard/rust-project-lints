//! Executable implementations of the documented style rules.
//!
//! [`Rule`] is the registry: the `rules!` declaration groups the rules by
//! category and carries every rule's id, canonical name, default level and
//! short report text. [`Rule::ALL`] lists the rules in report order: source
//! layout (`S`), Cargo manifest layout (`M`), then TOML file format (`T`).

mod manifest;
mod qualified;
mod source;

pub use manifest::{
    WorkspaceMembers, check_manifest_with_workspace_members, check_toml_file, external_crate_names,
};
pub use qualified::{Pattern, QualifiedNamePolicy, check_qualified_names};
pub use source::check_rust_source;

/// The severity one rule reports its violations with. Every rule carries a
/// default level in the registry; the `[levels]` section of the style config
/// overrides it per rule by canonical name. A [`Level::Warn`] violation
/// reports without failing the check; a [`Level::Error`] violation fails it;
/// a [`Level::Off`] rule is disabled and reports nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    /// The rule is disabled: its violations are dropped before reporting.
    Off,
    /// Reported, but a run whose violations are all warnings passes.
    Warn,
    /// Reported, and fails the run.
    Error,
}

/// One category of style rules; the category fixes the letter every rule id of
/// the group starts with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RuleGroup {
    /// Source layout (`S`): item and reference placement inside a Rust file.
    Source,
    /// Cargo manifest layout (`M`): sections and dependency tables.
    Manifest,
    /// TOML file format (`T`): the byte-level shape of every TOML file.
    Toml,
}

impl RuleGroup {
    /// Every group in report order.
    pub const ALL: &[RuleGroup] = &[Self::Source, Self::Manifest, Self::Toml];

    /// The letter every rule id of this group starts with.
    pub const fn prefix(self) -> &'static str {
        match self {
            Self::Source => "S",
            Self::Manifest => "M",
            Self::Toml => "T",
        }
    }
}

/// Declare the rule registry: one block per rule group, one entry per rule in
/// report order, each entry carrying the rule's id, canonical name, default
/// level and short report message.
///
/// The expansion owns the whole rule surface — the enum variants in the
/// declared order (the order violations inside one file report in), [`Rule::ALL`]
/// and the group/id/name/level/message lookups — so a rule cannot exist
/// without its metadata and an id is written exactly once. Every id must be
/// its group's letter followed by the rule's 1-based ordinal inside the
/// group; a `rules/mod.rs` test enforces that numbering.
///
/// The `@collect` pass accumulates the group blocks into parallel token lists,
/// because `macro_rules!` cannot repeat the entries of one group inside a
/// repetition over all groups.
macro_rules! rules {
    ($(
        $group:ident {
            $(
                $id:ident: $variant:ident($name:literal) = $level:ident => $message:literal,
            )*
        }
    )*) => {
        rules! {
            @collect
            [] [] [] [] [] [] []
            $(
                $group {
                    $($id: $variant($name) = $level => $message,)*
                }
            )*
        }
    };

    (@collect
        [$($entries:tt)*]
        [$($variants:tt)*]
        [$($group_arms:tt)*]
        [$($id_arms:tt)*]
        [$($name_arms:tt)*]
        [$($level_arms:tt)*]
        [$($message_arms:tt)*]
    ) => {
        /// The stable identifier, canonical name, group and short report text
        /// of one style rule, in report order.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
        pub enum Rule {
            $($entries)*
        }

        impl Rule {
            /// Every rule in report order.
            pub const ALL: &[Rule] = &[$($variants)*];

            /// The group this rule belongs to.
            pub const fn group(self) -> RuleGroup {
                match self {
                    $($group_arms)*
                }
            }

            /// The stable rule id: the group letter followed by the rule's
            /// 1-based ordinal inside the group (`S1`, `M3`, `T2`).
            pub const fn id(self) -> &'static str {
                match self {
                    $($id_arms)*
                }
            }

            /// The canonical kebab-case name (`module-doc-spacing`). A report
            /// prints it next to [`Rule::id`]; a rule with options uses it as
            /// the section key of the one style config.
            pub const fn name(self) -> &'static str {
                match self {
                    $($name_arms)*
                }
            }

            /// The severity the rule reports with before the style config's
            /// `[levels]` section overrides it. Every rule currently defaults
            /// to [`Level::Error`], preserving the check's standing verdict.
            pub const fn default_level(self) -> Level {
                match self {
                    $($level_arms)*
                }
            }

            /// Short actionable text; the full policy is maintained in the
            /// README.
            pub const fn message(self) -> &'static str {
                match self {
                    $($message_arms)*
                }
            }
        }
    };

    (@collect
        [$($entries:tt)*]
        [$($variants:tt)*]
        [$($group_arms:tt)*]
        [$($id_arms:tt)*]
        [$($name_arms:tt)*]
        [$($level_arms:tt)*]
        [$($message_arms:tt)*]
        $group:ident {
            $(
                $id:ident: $variant:ident($name:literal) = $level:ident => $message:literal,
            )*
        }
        $($rest:tt)*
    ) => {
        rules! {
            @collect
            [
                $($entries)*
                $(
                    #[doc = concat!("`", $name, "`: ", $message)]
                    $variant,
                )*
            ]
            [$($variants)* $(Self::$variant,)*]
            [$($group_arms)* $(Self::$variant => RuleGroup::$group,)*]
            [$($id_arms)* $(Self::$variant => stringify!($id),)*]
            [$($name_arms)* $(Self::$variant => $name,)*]
            [$($level_arms)* $(Self::$variant => Level::$level,)*]
            [$($message_arms)* $(Self::$variant => $message,)*]
            $($rest)*
        }
    };
}

rules! {
    Source {
        S1: ModuleDocSpacing("module-doc-spacing") = Error
            => "module-level `//!` docs need exactly one blank line before the next item",
        S2: TestModBlankAfterAttribute("test-mod-attribute-spacing") = Error
            => "no blank line may separate `#[cfg(test)]` from the test mod",
        S3: TestModLeadingBlankLine("test-mod-body-spacing") = Error
            => "the test mod body must not open with a blank line",
        S4: QualifiedName("qualified-name") = Error
            => "qualified reference spells a module path; import the type and use its bare name, or whitelist the spelling in stylecheck.toml",
        S5: ModBeforeUse("mod-before-use") = Error
            => "file-module declarations (`mod name;`) must precede every `use` item",
    }

    Manifest {
        M1: ManifestParse("manifest-parse") = Error => "manifest is not valid TOML",
        M2: ManifestSectionOrder("section-order") = Error => "manifest sections are out of order",
        M3: ManifestSectionSpacing("section-spacing") = Error
            => "manifest sections need exactly one blank line between them",
        M4: DependencyGroupOrder("dependency-group-order") = Error
            => "dependencies are in the wrong group order",
        M5: DependencySorted("dependency-sorting") = Error
            => "dependencies inside a group must be sorted",
    }

    Toml {
        T1: TomlLineEnding("line-endings") = Error
            => "TOML must use LF line endings, not CRLF",
        T2: TomlTrailingBlankLine("trailing-blank-line") = Error
            => "TOML must not end with a trailing blank line",
        T3: TomlMissingTrailingNewline("missing-trailing-newline") = Error
            => "TOML must end with a trailing newline",
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    /// The policy reference every rule is documented in.
    const README: &str = include_str!("../../README.md");

    #[test]
    fn registry_metadata_is_canonical() {
        let mut group_rank = 0;
        let mut ordinals = vec![0_usize; RuleGroup::ALL.len()];
        let mut names = BTreeSet::new();
        for rule in Rule::ALL {
            let rank = RuleGroup::ALL
                .iter()
                .position(|group| *group == rule.group())
                .expect("every rule group is listed in RuleGroup::ALL");
            assert!(rank >= group_rank, "{rule:?} breaks the group order");
            group_rank = rank;
            ordinals[rank] += 1;
            let expected = format!("{}{}", rule.group().prefix(), ordinals[rank]);
            assert_eq!(rule.id(), expected, "{rule:?} has a non-canonical id");
            assert!(names.insert(rule.name()), "{rule:?} repeats a rule name");
            assert!(
                rule.name().split('-').all(|word| {
                    !word.is_empty()
                        && word.chars().all(|character| {
                            character.is_ascii_lowercase() || character.is_ascii_digit()
                        })
                }),
                "{rule:?} does not carry a kebab-case name"
            );
        }
    }

    #[test]
    fn readme_documents_every_rule() {
        for rule in Rule::ALL {
            let label = format!("**`{}` `{}`.**", rule.id(), rule.name());
            assert!(
                README.contains(&label),
                "README.md is missing the {label} bullet"
            );
        }
    }
}
