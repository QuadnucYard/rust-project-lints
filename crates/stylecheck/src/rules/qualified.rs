//! Qualified type references must not spell a `module::Type` path.
//!
//! A usage position that writes a snake_case module segment immediately before
//! a PascalCase type segment (`crate::identity::SchemaDescriptor`,
//! `registry::DuplicateSchema`) is a violation: import the type and use its
//! bare name, or allow the literal spelling explicitly in
//! `stylecheck.toml`. `use`/`pub use` items, macro template bodies
//! (`quote!{…}`, `macro_rules! …`), and paths rooted at external crates pass
//! by default. The check is purely syntactic and compiler-free: it examines
//! literal token paths and never resolves `use` aliases. The rule reads its
//! options from the `[qualified-name]` section of the one style config; the
//! scan itself has no path policy, so the caller applies `exempt-paths`.

use std::{
    collections::{BTreeSet, VecDeque},
    str::FromStr,
};

use proc_macro2::{LineColumn, Spacing, TokenStream, TokenTree};

use crate::{ColumnSpan, Rule, Violation, config::ConfigError};

/// How a pattern matches a candidate path spelling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PatternKind {
    /// The spelling must equal the full path.
    Exact,
    /// `"P::*"` matches any path beginning with `P::`.
    Prefix,
    /// `"*::P"` matches any path ending with `::P`.
    Suffix,
}

/// One allow/deny pattern for the `qualified-name` and `expect-over-allow`
/// policies.
///
/// `Pattern::from_spelling("P::*")` builds a prefix pattern matching any
/// canonical spelling beginning `P::`; `Pattern::from_spelling("*::P")`
/// builds a suffix pattern matching any spelling ending in `::P`; any other
/// spelling is matched exactly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pattern {
    kind: PatternKind,
    /// The matched text: the full spelling for exact, `"P::"` for prefix,
    /// `"::P"` for suffix.
    text: String,
}

impl Pattern {
    /// Build a pattern from a canonical spelling (no whitespace), rejecting
    /// spellings that are not a supported form. A `*` at exactly one end is a
    /// wildcard: `"P::*"` matches any `P::`-prefixed path, `"*::P"` matches
    /// any `::P`-suffixed path; anything else is an exact path spelling.
    pub fn from_spelling(spelling: &str) -> Result<Pattern, ConfigError> {
        if let Some(suffix) = spelling.strip_prefix("*::") {
            validate_path_form(suffix, spelling)?;
            Ok(Pattern {
                kind: PatternKind::Suffix,
                text: format!("::{suffix}"),
            })
        } else if let Some(prefix) = spelling.strip_suffix("::*") {
            validate_path_form(prefix, spelling)?;
            Ok(Pattern {
                kind: PatternKind::Prefix,
                text: format!("{prefix}::"),
            })
        } else {
            validate_path_form(spelling, spelling)?;
            Ok(Pattern {
                kind: PatternKind::Exact,
                text: spelling.to_owned(),
            })
        }
    }

    /// Whether the canonical spelling of the candidate matches this pattern.
    pub(crate) fn matches(&self, spelled: &str) -> bool {
        match self.kind {
            PatternKind::Exact => spelled == self.text,
            PatternKind::Prefix => spelled.starts_with(&self.text),
            PatternKind::Suffix => spelled.ends_with(&self.text),
        }
    }
}

/// Build one policy's pattern list from the whitelist or blacklist spellings
/// deserialized out of a config section. `"P::*"` entries become prefix
/// patterns, `"*::P"` entries suffix patterns, and anything else an exact
/// spelling; an unsupported spelling is a hard [`ConfigError`].
pub(crate) fn parse_patterns(spellings: &[String]) -> Result<Vec<Pattern>, ConfigError> {
    spellings
        .iter()
        .map(|spelling| Pattern::from_spelling(spelling))
        .collect()
}

/// Reject a pattern's path part unless it is a supported form: non-empty,
/// whitespace-free, made of `::`-joined identifier segments, and free of any
/// `*` (the wildcard may only sit at one end of the full spelling).
fn validate_path_form(path: &str, spelling: &str) -> Result<(), ConfigError> {
    if path.is_empty() {
        return Err(invalid_pattern(spelling, "the path part must not be empty"));
    }
    if path.contains('*') {
        return Err(invalid_pattern(
            spelling,
            "a `*` wildcard may only appear at one end of the spelling",
        ));
    }
    if path.chars().any(char::is_whitespace) {
        return Err(invalid_pattern(
            spelling,
            "the spelling must not contain whitespace",
        ));
    }
    for segment in path.split("::") {
        if !is_identifier(segment) {
            return Err(invalid_pattern(
                spelling,
                format!("{segment:?} is not a valid path segment"),
            ));
        }
    }
    Ok(())
}

/// Build the config error that names the offending pattern spelling.
fn invalid_pattern(spelling: &str, reason: impl Into<String>) -> ConfigError {
    ConfigError::InvalidConfig {
        detail: format!("invalid pattern {spelling:?}: {}", reason.into()),
    }
}

/// A nonempty path segment, like a Rust path segment.
fn is_identifier(segment: &str) -> bool {
    let mut characters = segment.chars();
    characters
        .next()
        .is_some_and(|first| first.is_alphabetic() || first == '_')
        && segment
            .chars()
            .all(|character| character.is_alphanumeric() || character == '_')
}

/// external-crate exemption. Precedence is whitelist > blacklist > built-in
/// external-crate exemption > default deny.
#[derive(Debug, Clone, Default)]
pub struct QualifiedNamePolicy {
    whitelist: Vec<Pattern>,
    blacklist: Vec<Pattern>,
    external_crates: BTreeSet<String>,
}

impl QualifiedNamePolicy {
    /// The built-in external-crate exemption set.
    pub fn with_external_crates(mut self, external_crates: BTreeSet<String>) -> Self {
        self.external_crates = external_crates;
        self
    }

    /// Build the policy from the `[qualified-name]` spellings deserialized out
    /// of the one style config; an unsupported spelling is a hard
    /// [`ConfigError`].
    pub fn from_lists(whitelist: Vec<String>, blacklist: Vec<String>) -> Result<Self, ConfigError> {
        Ok(Self {
            whitelist: parse_patterns(&whitelist)?,
            blacklist: parse_patterns(&blacklist)?,
            external_crates: BTreeSet::new(),
        })
    }
}

/// Check one Rust source file for `qualified-name` violations — a usage
/// position that spells a `module::Type` path.
pub fn check_qualified_names(
    path: &str,
    text: &str,
    policy: &QualifiedNamePolicy,
) -> Vec<Violation> {
    let Ok(stream) = TokenStream::from_str(text) else {
        // A valid Rust module is a valid token stream; an unparsable file
        // cannot be scanned and is skipped rather than misreported.
        return Vec::new();
    };
    let tokens: Vec<TokenTree> = stream.into_iter().collect();
    check_qualified_tokens(path, text, policy, &tokens)
}

/// [`check_qualified_names`] over a token stream the caller already parsed, so
/// a whole-file check tokenizes each source once.
pub(crate) fn check_qualified_tokens(
    path: &str,
    text: &str,
    policy: &QualifiedNamePolicy,
    tokens: &[TokenTree],
) -> Vec<Violation> {
    let mut found = Vec::new();
    scan_pairs(
        tokens,
        &mut VecDeque::new(),
        &mut RunState::default(),
        path,
        text,
        policy,
        &mut found,
    );
    found
}

/// The recent token kinds a scanner needs to recognize `use` items and macro
/// bodies; kept small so the common case only touches a two/three-token window.
#[derive(Clone, Debug)]
enum PrevKind {
    Ident(String),
    Punct(char),
    Group,
    Literal,
}

fn push_prev(prev: &mut VecDeque<PrevKind>, token: &TokenTree) {
    let kind = match token {
        TokenTree::Ident(ident) => PrevKind::Ident(ident.to_string()),
        TokenTree::Punct(punct) => PrevKind::Punct(punct.as_char()),
        TokenTree::Group(_) => PrevKind::Group,
        TokenTree::Literal(_) => PrevKind::Literal,
    };
    prev.push_back(kind);
    if prev.len() > 3 {
        prev.pop_front();
    }
}

/// Whether `use` at the current position opens a use item: the preceding token
/// is not `::`, `.`, `<`, `>`, or an identifier. A bare `pub` immediately
/// before it is the one identifier that still opens a re-export.
fn is_use_start(prev: &VecDeque<PrevKind>) -> bool {
    match prev.back() {
        None => true,
        Some(PrevKind::Ident(name)) => name == "pub",
        Some(PrevKind::Punct(character)) => !matches!(character, ':' | '.' | '<' | '>'),
        Some(PrevKind::Group) | Some(PrevKind::Literal) => true,
    }
}

/// Token-generating macros whose bodies are templates, not authored code.
pub(crate) const TEMPLATE_MACROS: &[&str] = &[
    "quote",
    "quote_spanned",
    "parse_quote",
    "parse_quote_spanned",
];

/// A group is a macro template body — and out of `qualified-name` scope — only
/// for a declarative macro definition or a known token-generating template
/// macro. Ordinary macro invocation arguments remain usage positions and are
/// scanned.
fn is_macro_template(prev: &VecDeque<PrevKind>) -> bool {
    let tokens: Vec<&PrevKind> = prev.iter().collect();
    match tokens.as_slice() {
        [.., PrevKind::Ident(name), PrevKind::Punct('!')]
            if TEMPLATE_MACROS.contains(&name.as_str()) =>
        {
            true
        }
        [
            ..,
            PrevKind::Ident(prefix),
            PrevKind::Punct('!'),
            PrevKind::Ident(_),
        ] if prefix == "macro_rules" => true,
        _ => false,
    }
}

/// Whether the previous `#[...]` group is a `#[from]` attribute. The type
/// path that follows it names the source error of an enum variant; spelling
/// it qualified is idiomatic and exempt from `qualified-name`.
fn is_from_attribute(prev: &VecDeque<PrevKind>, inner: &[TokenTree]) -> bool {
    matches!(prev.back(), Some(PrevKind::Punct('#')))
        && matches!(inner, [TokenTree::Ident(ident)] if ident == "from")
}

/// The index one past the `;` that terminates the `use` item beginning at
/// `start`. A use item contains no statement-level semicolons, so the first
/// `;` at this group level is its terminator.
fn use_item_end(tokens: &[TokenTree], start: usize) -> usize {
    tokens[start..]
        .iter()
        .position(|token| matches!(token, TokenTree::Punct(punct) if punct.as_char() == ';'))
        .map_or(tokens.len(), |offset| start + offset + 1)
}

/// One segment of a path run under evaluation.
#[derive(Clone, Debug)]
struct Segment {
    text: String,
    /// Where the segment starts and ends in the source file.
    start: LineColumn,
    end: LineColumn,
}

/// The path run currently under construction during Pass B.
#[derive(Default)]
struct RunState {
    run: Vec<Segment>,
    open: bool,
    /// The run is the argument of a `#[from]` attribute and is exempt.
    from_argument: bool,
}

/// Scan literal token paths and report denied `module::Type` pairs.
fn scan_pairs(
    tokens: &[TokenTree],
    prev: &mut VecDeque<PrevKind>,
    state: &mut RunState,
    path: &str,
    text: &str,
    policy: &QualifiedNamePolicy,
    out: &mut Vec<Violation>,
) {
    let mut index = 0;
    while index < tokens.len() {
        let token = &tokens[index];
        if let TokenTree::Ident(ident) = token
            && ident == "use"
            && is_use_start(prev)
        {
            let end = use_item_end(tokens, index);
            state.run.clear();
            state.open = false;
            state.from_argument = false;
            index = end.saturating_sub(1);
            continue; // the ';' is processed normally and keeps the run broken
        }
        match token {
            TokenTree::Ident(ident) => {
                let segment = Segment {
                    text: ident.to_string(),
                    start: ident.span().start(),
                    end: ident.span().end(),
                };
                if state.open {
                    state.run.push(segment);
                } else {
                    state.run = vec![segment];
                }
                state.open = false;
                if !state.from_argument && state.run.len() >= 2 {
                    check_pair(&state.run, state.run.len() - 2, path, text, policy, out);
                }
            }
            TokenTree::Punct(punct)
                if punct.as_char() == ':' && punct.spacing() == Spacing::Joint =>
            {
                state.open = true;
                if matches!(
                    tokens.get(index + 1),
                    Some(TokenTree::Punct(next)) if next.as_char() == ':'
                ) {
                    index += 1;
                }
            }
            TokenTree::Group(group) => {
                if !is_macro_template(prev) {
                    let inner: Vec<TokenTree> = group.stream().into_iter().collect();
                    let from_attribute = is_from_attribute(prev, &inner);
                    if from_attribute {
                        state.from_argument = true;
                    }
                    scan_pairs(
                        &inner,
                        &mut VecDeque::new(),
                        &mut RunState::default(),
                        path,
                        text,
                        policy,
                        out,
                    );
                    // A `#[from]` group hands the exemption to the path after
                    // it; any other group breaks the run and ends it.
                    if !from_attribute {
                        state.from_argument = false;
                    }
                } else {
                    state.from_argument = false;
                }
                state.run.clear();
                state.open = false;
            }
            _ => {
                // A literal or a non-`::` punct breaks the run.
                state.run.clear();
                state.open = false;
                state.from_argument = false;
            }
        }
        push_prev(prev, token);
        index += 1;
    }
}

/// Evaluate one adjacent `module::Type` pair at `run[index]`, `run[index+1]`.
fn check_pair(
    run: &[Segment],
    index: usize,
    path: &str,
    text: &str,
    policy: &QualifiedNamePolicy,
    out: &mut Vec<Violation>,
) {
    let module = &run[index];
    let ty = &run[index + 1];
    if !is_snake(&module.text) || !is_pascal(&ty.text) || is_excluded_segment(&module.text) {
        return;
    }
    if allows(policy, run, index) {
        return;
    }
    let mut violation = Violation::new(path, module.start.line, Rule::QualifiedName, text);
    // The report underlines the whole spelled path — the first segment's
    // first character through the type segment's last, the spelling the
    // whitelist and blacklist match — but only when it sits on one line: a
    // label spanning lines would reach past the excerpt the report stores,
    // so a line-split spelling falls back to its line.
    if run[0].start.line == ty.end.line {
        violation = violation.narrowed_to(ColumnSpan {
            start: run[0].start.column,
            end: ty.end.column,
        });
    }
    out.push(violation);
}

/// Whether the literal candidate path is permitted: whitelist wins over
/// blacklist, which win over built-in root exemptions; everything else denies.
fn allows(policy: &QualifiedNamePolicy, run: &[Segment], index: usize) -> bool {
    let spelled: String = run[..=index + 1]
        .iter()
        .map(|segment| segment.text.as_str())
        .collect::<Vec<_>>()
        .join("::");
    if policy
        .whitelist
        .iter()
        .any(|pattern| pattern.matches(&spelled))
    {
        return true;
    }
    if policy
        .blacklist
        .iter()
        .any(|pattern| pattern.matches(&spelled))
    {
        return false;
    }
    if policy.external_crates.contains(&run[0].text) {
        return true;
    }
    false
}

/// A snake_case identifier: ASCII lowercase first, then lowercase/digit/`_`.
fn is_snake(name: &str) -> bool {
    let mut characters = name.chars();
    if !characters
        .next()
        .is_some_and(|first| first.is_ascii_lowercase())
    {
        return false;
    }
    characters.all(|character| {
        character.is_ascii_lowercase() || character.is_ascii_digit() || character == '_'
    })
}

/// A PascalCase identifier: uppercase first, with at least one lowercase
/// character so screaming-snake constants remain outside the type rule.
fn is_pascal(name: &str) -> bool {
    let mut characters = name.chars();
    if !characters
        .next()
        .is_some_and(|first| first.is_ascii_uppercase())
    {
        return false;
    }
    let mut has_lowercase = false;
    let valid = characters.all(|character| {
        has_lowercase |= character.is_ascii_lowercase();
        character.is_ascii_alphanumeric() || character == '_'
    });
    valid && has_lowercase
}

/// Keywords and primitives are never module segments: `crate::Type`,
/// `self::Type`, `u32::MAX` and `Self::…` are never `qualified-name`
/// candidates.
fn is_excluded_segment(name: &str) -> bool {
    matches!(
        name,
        "crate"
            | "self"
            | "super"
            | "Self"
            | "_"
            | "bool"
            | "char"
            | "str"
            | "u8"
            | "u16"
            | "u32"
            | "u64"
            | "u128"
            | "usize"
            | "i8"
            | "i16"
            | "i32"
            | "i64"
            | "i128"
            | "isize"
            | "f32"
            | "f64"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::StyleConfig;

    /// A policy with the built-in exemption sets the `qualified-name` unit
    /// tests exercise.
    fn test_policy() -> QualifiedNamePolicy {
        QualifiedNamePolicy::default().with_external_crates(BTreeSet::from([
            "std".to_owned(),
            "geometry".to_owned(),
            "markup".to_owned(),
        ]))
    }

    #[test]
    fn clean_usage_positions_are_not_reported() {
        let text = r#"
use crate::identity::SchemaDescriptor;
use markup::ast;
use std::collections::BTreeMap;

fn main() {
    let a = geometry::Vec3::ZERO;
    let b = std::collections::BTreeMap::new();
    let c = markup::ast::Entry::new();
    let d = crate::CompileDiagnostic;
    let f = u32::MAX;
    let g = Self::new();
    quote! { crate::identity::SchemaDescriptor }
    // [crate::contract::X]
    let s = "crate::foo::Bar";
}
"#;
        let found = check_qualified_names("src/lib.rs", text, &test_policy());
        assert!(found.is_empty(), "{found:?}");
    }

    #[test]
    fn literal_qualified_paths_are_reported_inside_macro_arguments() {
        let text = "assert_eq!(registry::DuplicateSchema(value), other::Thing);\nvec![module::Type::new()];\nquote! { hidden::Template }\n";
        let found = check_qualified_names("src/lib.rs", text, &QualifiedNamePolicy::default());
        assert_eq!(found.len(), 3);
        assert_eq!(found[0].line, 1);
        assert_eq!(found[1].line, 1);
        assert_eq!(found[2].line, 2);
    }

    #[test]
    fn macro_template_bodies_are_not_usage_positions() {
        let text = "macro_rules! generated { () => { registry::DuplicateSchema(value) }; }\n";
        assert!(
            check_qualified_names("src/lib.rs", text, &QualifiedNamePolicy::default()).is_empty()
        );
    }

    #[test]
    fn all_caps_constants_are_not_classified_as_types() {
        let text = "fn value() -> u32 { networking::MAX_CONNECTIONS }\n";
        assert!(
            check_qualified_names("src/lib.rs", text, &QualifiedNamePolicy::default()).is_empty()
        );
    }

    #[test]
    fn module_qualified_types_are_reported_at_their_line() {
        let text = "fn decode() -> crate::identity::SchemaDescriptor {\n    registry::DuplicateSchema\n}\n";
        let found = check_qualified_names("src/lib.rs", text, &QualifiedNamePolicy::default());
        assert_eq!(found.len(), 2);
        assert!(
            found
                .iter()
                .all(|violation| violation.rule == Rule::QualifiedName)
        );
        assert!(
            found
                .iter()
                .all(|violation| !violation.context.lines.is_empty())
        );
        assert_eq!(found[0].line, 1);
        assert_eq!(found[1].line, 2);
    }

    #[test]
    fn reports_carry_the_columns_of_the_spelled_path() {
        let text = "fn decode() -> registry::DuplicateSchema {}\n";
        let found = check_qualified_names("src/lib.rs", text, &QualifiedNamePolicy::default());
        assert_eq!(found.len(), 1, "{found:?}");
        // `registry::DuplicateSchema` starts at character 15 of the line and
        // ends one past its last character at 40.
        assert_eq!(found[0].columns, Some(ColumnSpan { start: 15, end: 40 }));
        // A rooted path is underlined from its first segment: the spelling
        // the whitelist and blacklist match is `crate::registry::DuplicateSchema`,
        // characters 8..40.
        let rooted = "let d = crate::registry::DuplicateSchema;\n";
        let found = check_qualified_names("src/lib.rs", rooted, &QualifiedNamePolicy::default());
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].columns, Some(ColumnSpan { start: 8, end: 40 }));
        // A spelling split across lines cannot be one span; it keeps the
        // whole-line underline.
        let split = "fn decode() -> registry::\n    DuplicateSchema {}\n";
        let found = check_qualified_names("src/lib.rs", split, &QualifiedNamePolicy::default());
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].columns, None);
    }

    #[test]
    fn only_from_attribute_arguments_are_exempt() {
        let exempt = "enum Error { Storage(#[from] persist::StorageError) }\n";
        assert!(
            check_qualified_names("src/lib.rs", exempt, &QualifiedNamePolicy::default()).is_empty()
        );
        let reported = "fn f() { let e = persist::StorageError::Other; }\n";
        let found = check_qualified_names("src/lib.rs", reported, &QualifiedNamePolicy::default());
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].rule, Rule::QualifiedName);
    }

    #[test]
    fn blacklist_re_denies_auto_exempt_external_path() {
        let policy =
            StyleConfig::from_toml("[qualified-name]\nblacklist = [\"std::collections::*\"]\n")
                .unwrap()
                .with_external_crates(BTreeSet::from(["std".to_owned()]))
                .qualified()
                .clone();
        let text = "fn map() -> std::collections::BTreeMap<i32, i32> { todo!() }\n";
        let found = check_qualified_names("src/lib.rs", text, &policy);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].rule, Rule::QualifiedName);
    }

    #[test]
    fn whitelist_wins_over_blacklist() {
        let policy = StyleConfig::from_toml(
            "[qualified-name]\nwhitelist = [\"wire::*\"]\nblacklist = [\"wire::*\"]\n",
        )
        .unwrap()
        .qualified()
        .clone();
        let text = "fn stream() -> wire::Stream { todo!() }\n";
        assert!(check_qualified_names("src/lib.rs", text, &policy).is_empty());
    }

    #[test]
    fn config_section_parses_to_expected_patterns() {
        let config = r#"[qualified-name]
whitelist = [
    "wire::*",
]
blacklist = [
    "std::collections::*",
]
"#;
        let policy = StyleConfig::from_toml(config).unwrap().qualified().clone();
        assert_eq!(policy.whitelist.len(), 1);
        assert_eq!(policy.whitelist[0].kind, PatternKind::Prefix);
        assert_eq!(policy.whitelist[0].text, "wire::");
        assert_eq!(policy.blacklist.len(), 1);
        assert_eq!(policy.blacklist[0].kind, PatternKind::Prefix);
        assert_eq!(policy.blacklist[0].text, "std::collections::");
    }

    #[test]
    fn malformed_config_is_a_hard_error() {
        assert!(StyleConfig::from_toml("[qualified\nwhitelist = [").is_err());
        assert!(StyleConfig::from_toml("[qualified-name]\nwhitelist = 3\n").is_err());
        assert!(StyleConfig::from_toml("[qualified-name]\nwhitelist = [3]\n").is_err());
        assert!(StyleConfig::from_toml("[qualified-name]\nwhitelist = [\"*::*\"]\n").is_err());
    }

    #[test]
    fn pattern_spellings() {
        let exact = Pattern::from_spelling("wire::Stream").unwrap();
        assert_eq!(exact.kind, PatternKind::Exact);
        assert_eq!(exact.text, "wire::Stream");
        assert!(exact.matches("wire::Stream"));
        assert!(!exact.matches("wire::StreamX"));

        let prefix = Pattern::from_spelling("wire::*").unwrap();
        assert_eq!(prefix.kind, PatternKind::Prefix);
        assert_eq!(prefix.text, "wire::");
        assert!(prefix.matches("wire::Stream"));
        assert!(!prefix.matches("wired::Stream"));

        let suffix = Pattern::from_spelling("*::Iter").unwrap();
        assert_eq!(suffix.kind, PatternKind::Suffix);
        assert_eq!(suffix.text, "::Iter");
        assert!(suffix.matches("std::collections::btree_set::Iter"));
        assert!(suffix.matches("wire::Iter"));
        assert!(!suffix.matches("std::collections::btree_set::IntoIter"));
        assert!(!suffix.matches("std::iter::Copied"));
    }

    #[test]
    fn unsupported_pattern_spellings_are_rejected() {
        for spelling in [
            "",
            "*",
            "*::*",
            "*::",
            "::*",
            "foo*::Bar",
            "foo::*::Bar",
            "foo bar::Baz",
            "1foo::Bar",
        ] {
            assert!(
                Pattern::from_spelling(spelling).is_err(),
                "{spelling:?} should be rejected"
            );
        }
        for spelling in [
            "wire::Stream",
            "wire::*",
            "*::Iter",
            "std::fmt::*",
            "*::IntoIter",
        ] {
            assert!(Pattern::from_spelling(spelling).is_ok(), "{spelling:?}");
        }
    }
}
