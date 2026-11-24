//! Lint escape hatches must be written as expectations.
//!
//! An `#[allow(…)]` hides its lint forever, so an escape hatch that stops
//! suppressing anything lingers unnoticed. `#[expect(…)]` reports itself
//! through `unfulfilled_lint_expectations` instead, so this rule reports every
//! `allow` that could be an expectation. Macro template bodies are out of
//! scope, a built-in set of lint names stays on `allow` because an expectation
//! cannot serve their purpose, and the `[expect-over-allow]` section of the one
//! style config widens or narrows that set.

use proc_macro2::{Delimiter, Group, Ident, TokenTree};

use crate::{
    ColumnSpan, ConfigError, Rule, Violation,
    rules::qualified::{Pattern, TEMPLATE_MACROS, parse_patterns},
};

/// Lint names that must stay `allow`: `expect` cannot serve their purpose.
///
/// `#[expect(unfulfilled_lint_expectations)]` is defined to always emit the
/// lint, so the expectation can never be fulfilled; `unknown_lints` and
/// `renamed_and_removed_lints` exist to tolerate lint names that another
/// supported compiler version does not know, and an expectation would be
/// unfulfilled on a compiler that knows them.
const ALLOW_ONLY_LINTS: &[&str] = &[
    "unfulfilled_lint_expectations",
    "unknown_lints",
    "renamed_and_removed_lints",
];

/// The `expect-over-allow` rule's lint-name allow/deny policy.
#[derive(Debug, Clone, Default)]
pub struct ExpectOverAllowPolicy {
    whitelist: Vec<Pattern>,
    blacklist: Vec<Pattern>,
}

impl ExpectOverAllowPolicy {
    /// Build the policy from the `[expect-over-allow]` spellings deserialized
    /// out of the one style config; an unsupported spelling is a hard
    /// [`ConfigError`]. A missing section yields two empty lists, leaving the
    /// built-in allow-only set as the whole policy.
    pub fn from_lists(whitelist: Vec<String>, blacklist: Vec<String>) -> Result<Self, ConfigError> {
        Ok(Self {
            whitelist: parse_patterns(&whitelist)?,
            blacklist: parse_patterns(&blacklist)?,
        })
    }

    /// Whether `lint` may stay in an `allow` attribute: whitelist wins over
    /// blacklist, which win over the built-in allow-only set.
    pub(crate) fn allows(&self, lint: &str) -> bool {
        if self.whitelist.iter().any(|pattern| pattern.matches(lint)) {
            return true;
        }
        if self.blacklist.iter().any(|pattern| pattern.matches(lint)) {
            return false;
        }
        ALLOW_ONLY_LINTS.contains(&lint)
    }
}

/// Rule `expect-over-allow`: a lint escape hatch is written with `#[expect(…)]`
/// so an escape hatch that no longer suppresses anything is reported by
/// `unfulfilled_lint_expectations` instead of lingering silently.
pub(crate) fn check_allow_attributes(
    path: &str,
    text: &str,
    tokens: &[TokenTree],
    policy: &ExpectOverAllowPolicy,
) -> Vec<Violation> {
    let mut found = Vec::new();
    scan_allow_attributes(tokens, path, text, policy, &mut found);
    found
}

/// Report every non-exempt `allow` meta item in one token sequence. Every
/// group is scanned in place — attribute arguments, item bodies, expression
/// blocks, closure bodies and ordinary macro invocation bodies — except a
/// template body, whose tokens are not authored code.
fn scan_allow_attributes(
    tokens: &[TokenTree],
    path: &str,
    text: &str,
    policy: &ExpectOverAllowPolicy,
    found: &mut Vec<Violation>,
) {
    for (index, token) in tokens.iter().enumerate() {
        match token {
            TokenTree::Group(group) if !is_template_body(tokens, index) => {
                let inner: Vec<TokenTree> = group.stream().into_iter().collect();
                scan_allow_attributes(&inner, path, text, policy, found);
            }
            TokenTree::Punct(punct) if punct.as_char() == '#' => {
                // Skip the `!` of an inner attribute (`#![allow(…)]`).
                let mut next = index + 1;
                if tokens.get(next).is_some_and(is_bang) {
                    next += 1;
                }
                let Some(TokenTree::Group(attribute)) = tokens.get(next) else {
                    continue;
                };
                if attribute.delimiter() != Delimiter::Bracket {
                    continue;
                }
                for ident in allow_metas(attribute, policy) {
                    // An identifier is one line, so its span always yields the
                    // report line and the exact columns; two `allow` metas on
                    // one line stay distinguishable.
                    let span = ident.span();
                    found.push(
                        Violation::new(path, span.start().line, Rule::ExpectOverAllow, text)
                            .narrowed_to(ColumnSpan {
                                start: span.start().column,
                                end: span.end().column,
                            }),
                    );
                }
            }
            _ => {}
        }
    }
}

/// Whether the group at `index` is a macro template body: the body of a
/// `macro_rules!` definition or of a known token-generating macro
/// ([`TEMPLATE_MACROS`]). Ordinary macro invocation arguments are authored
/// tokens and are scanned, and a block after a negated condition
/// (`if !ready() { … }`) is code, not a template.
fn is_template_body(tokens: &[TokenTree], index: usize) -> bool {
    match &tokens[..index] {
        [.., TokenTree::Ident(name), bang]
            if is_bang(bang) && TEMPLATE_MACROS.iter().any(|macros| name == *macros) =>
        {
            true
        }
        [.., TokenTree::Ident(macros), bang, TokenTree::Ident(_)]
            if macros == "macro_rules" && is_bang(bang) =>
        {
            true
        }
        _ => false,
    }
}

/// The `allow` meta identifiers one attribute group would report. `#[allow(…)]`
/// reports its own meta item when the lint list is not wholly allowed, and
/// `#[cfg_attr(…)]` reports every non-exempt `allow` in its argument list.
fn allow_metas(attribute: &Group, policy: &ExpectOverAllowPolicy) -> Vec<Ident> {
    let tokens: Vec<TokenTree> = attribute.stream().into_iter().collect();
    // An attribute meta item is exactly `name(…)`; anything carrying more
    // tokens is a macro-DSL fragment such as `dsl!(#[allow(x) = 1])`, not an
    // attribute.
    let [TokenTree::Ident(meta), TokenTree::Group(arguments)] = tokens.as_slice() else {
        return Vec::new();
    };
    if arguments.delimiter() != Delimiter::Parenthesis {
        return Vec::new();
    }
    if meta == "allow" {
        return if all_lints_allowed(arguments, policy) {
            Vec::new()
        } else {
            vec![meta.clone()]
        };
    }
    if meta == "cfg_attr" {
        return cfg_attr_allow_metas(arguments, policy);
    }
    Vec::new()
}

/// The `allow` meta identifiers nested in one `cfg_attr` argument list: the
/// tokens before the first `,` are the cfg predicate, every item after it is an
/// inner attribute meta item, and a nested `cfg_attr` is followed recursively.
fn cfg_attr_allow_metas(arguments: &Group, policy: &ExpectOverAllowPolicy) -> Vec<Ident> {
    let items: Vec<TokenTree> = arguments.stream().into_iter().collect();
    let Some(comma) = items.iter().position(is_comma) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for meta in items[comma + 1..].split(is_comma) {
        if let [TokenTree::Ident(name), TokenTree::Group(group)] = meta
            && group.delimiter() == Delimiter::Parenthesis
        {
            if name == "allow" && !all_lints_allowed(group, policy) {
                found.push(name.clone());
            } else if name == "cfg_attr" {
                found.extend(cfg_attr_allow_metas(group, policy));
            }
        }
    }
    found
}

/// Whether every lint in one `allow(…)` argument group may stay `allow`. The
/// `reason = "…"` argument is not a lint; anything that does not spell a lint
/// path makes the list not allowed, so an unreadable list reports rather than
/// passes. An empty list is allowed.
fn all_lints_allowed(arguments: &Group, policy: &ExpectOverAllowPolicy) -> bool {
    let tokens: Vec<TokenTree> = arguments.stream().into_iter().collect();
    tokens.split(is_comma).all(|item| {
        item.is_empty()
            || is_reason_argument(item)
            || lint_path(item).is_some_and(|lint| policy.allows(&lint))
    })
}

/// Whether `item` is the `reason = "…"` argument of an attribute meta list.
fn is_reason_argument(item: &[TokenTree]) -> bool {
    matches!(
        item,
        [TokenTree::Ident(name), TokenTree::Punct(equals), TokenTree::Literal(_)]
            if name == "reason" && equals.as_char() == '='
    )
}

/// The `module::lint` spelling of one meta list item, or `None` when the item
/// is not a lint path. Only an identifier followed by `::`-separated
/// identifiers spells a lint, so a malformed item reports instead of passing.
fn lint_path(item: &[TokenTree]) -> Option<String> {
    let (first, rest) = item.split_first()?;
    let TokenTree::Ident(first) = first else {
        return None;
    };
    let mut lint = first.to_string();
    // Every following segment is exactly `::` and an identifier; a partial
    // chunk means the item is not a lint path.
    for segment in rest.chunks(3) {
        let [
            TokenTree::Punct(left),
            TokenTree::Punct(right),
            TokenTree::Ident(name),
        ] = segment
        else {
            return None;
        };
        if left.as_char() != ':' || right.as_char() != ':' {
            return None;
        }
        lint.push_str("::");
        lint.push_str(&name.to_string());
    }
    Some(lint)
}

/// Whether `token` is a meta list separator.
fn is_comma(token: &TokenTree) -> bool {
    matches!(token, TokenTree::Punct(punct) if punct.as_char() == ',')
}

/// Whether `token` is the `!` opening a macro body or an inner attribute.
fn is_bang(token: &TokenTree) -> bool {
    matches!(token, TokenTree::Punct(punct) if punct.as_char() == '!')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{StyleConfig, rules::source::check_rust_source};

    /// Check one source text against the source rules with the default policy.
    fn check(text: &str) -> Vec<Violation> {
        check_rust_source("src/lib.rs", text, &StyleConfig::default())
    }

    #[test]
    fn allow_attributes_are_reported_wherever_they_are_written() {
        for (text, line) in [
            ("#[allow(dead_code)]\nfn f() {}\n", 1),
            ("#[allow(\n    dead_code,\n)]\nfn f() {}\n", 1),
            ("#![allow(dead_code)]\n", 1),
            // Item bodies, inline modules, fields, closure bodies and
            // expression blocks all carry authored attributes.
            ("fn run() {\n    #[allow(unused)]\n    let x = 1;\n}\n", 2),
            ("mod inner {\n    #[allow(unused)]\n    fn f() {}\n}\n", 2),
            (
                "struct S {\n    #[allow(dead_code)]\n    field: u8,\n}\n",
                2,
            ),
            (
                "fn run() {\n    probe(|| {\n        #[allow(unused)]\n        let x = 1;\n    });\n}\n",
                3,
            ),
            (
                "fn run() {\n    if !ready() {\n        #[allow(unused)]\n        let x = 1;\n    }\n}\n",
                3,
            ),
        ] {
            let found = check(text);
            assert_eq!(found.len(), 1, "{text:?} {found:?}");
            assert_eq!(found[0].rule, Rule::ExpectOverAllow);
            assert_eq!(found[0].line, line, "{text:?}");
        }
    }

    #[test]
    fn allow_reports_underline_the_allow_meta() {
        let found = check("#[allow(dead_code)]\nfn f() {}\n");
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].columns, Some(ColumnSpan { start: 2, end: 7 }));

        // Two metas on one line stay distinguishable by their columns.
        let found = check("#[cfg_attr(test, allow(dead_code), allow(unused))]\nfn f() {}\n");
        assert_eq!(found.len(), 2, "{found:?}");
        assert_eq!(found[0].columns, Some(ColumnSpan { start: 17, end: 22 }));
        assert_eq!(found[1].columns, Some(ColumnSpan { start: 35, end: 40 }));
    }

    #[test]
    fn expectations_are_accepted() {
        for text in [
            "#[expect(dead_code)]\nfn f() {}\n",
            "#![expect(dead_code)]\n",
            "#[warn(dead_code)]\nfn f() {}\n",
            "#[deny(dead_code)]\nfn f() {}\n",
            "#[forbid(dead_code)]\nfn f() {}\n",
        ] {
            assert!(check(text).is_empty(), "{text:?}");
        }
    }

    #[test]
    fn allow_only_lints_are_exempt() {
        for text in [
            "#![allow(unfulfilled_lint_expectations)]\n",
            "#[allow(unknown_lints, renamed_and_removed_lints)]\nfn f() {}\n",
            "#[allow(unknown_lints, reason = \"supports older compilers\")]\nfn f() {}\n",
        ] {
            assert!(check(text).is_empty(), "{text:?}");
        }
        for text in [
            "#[allow(unknown_lints, dead_code)]\nfn f() {}\n",
            "#[allow(dead_code, reason = \"mixed\")]\nfn f() {}\n",
        ] {
            let found = check(text);
            assert_eq!(found.len(), 1, "{text:?} {found:?}");
            assert_eq!(found[0].rule, Rule::ExpectOverAllow);
            assert_eq!(found[0].line, 1, "{text:?}");
        }
    }

    #[test]
    fn cfg_attr_wrapped_allow_attributes_are_reported() {
        for (text, expected) in [
            ("#[cfg_attr(test, allow(dead_code))]\nfn f() {}\n", 1),
            ("#![cfg_attr(test, allow(dead_code))]\n", 1),
            ("#[cfg_attr(test, allow(unknown_lints))]\nfn f() {}\n", 0),
            (
                "#[cfg_attr(test, allow(dead_code), allow(unused))]\nfn f() {}\n",
                2,
            ),
            (
                "#[cfg_attr(all(test, feature = \"x\"), allow(dead_code))]\nfn f() {}\n",
                1,
            ),
            (
                "#[cfg_attr(test, cfg_attr(feature = \"probe\", allow(dead_code)))]\nfn f() {}\n",
                1,
            ),
        ] {
            let found = check(text);
            assert_eq!(found.len(), expected, "{text:?} {found:?}");
            assert!(
                found
                    .iter()
                    .all(|violation| violation.rule == Rule::ExpectOverAllow),
                "{text:?} {found:?}"
            );
        }
        // A multiline `cfg_attr` anchors on the `allow` meta it reports, not
        // on the `cfg_attr` keyword.
        let found = check("#[cfg_attr(\n    test,\n    allow(dead_code),\n)]\nfn f() {}\n");
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].line, 3, "{found:?}");
    }

    #[test]
    fn macro_templates_and_unrelated_identifiers_are_not_reported() {
        for text in [
            "macro_rules! generated { () => { #[allow(dead_code)] fn f() {} }; }\n",
            "quote! { #[allow(dead_code)] fn f() {} }\n",
            "quote_spanned! { span=> #[allow(dead_code)] fn f() {} }\n",
            "parse_quote! { #[allow(dead_code)] fn f() {} }\n",
            "parse_quote_spanned! { span=> #[allow(dead_code)] fn f() {} }\n",
            "#[cfg(allow)]\nfn f() {}\n",
            "fn f() {\n    let _ = \"#[allow(dead_code)]\";\n}\n",
            // A macro DSL that reuses attribute-shaped tokens is not an
            // attribute: the meta item carries a trailing token.
            "dsl!(#[allow(dead_code) = 1]);\n",
        ] {
            assert!(check(text).is_empty(), "{text:?}");
        }
        // Unreadable `allow` arguments are not a lint path, so they report
        // instead of passing silently; an empty list is nothing to report.
        let found = check("#[allow(foo = \"bar\")]\nfn f() {}\n");
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].rule, Rule::ExpectOverAllow);
        assert!(check("#[allow()]\nfn f() {}\n").is_empty());
    }

    #[test]
    fn ordinary_macro_invocation_bodies_are_scanned() {
        for text in [
            "wrapper! { #[allow(dead_code)] fn f() {} }\n",
            "wrapper!(#[allow(dead_code)] fn f() {});\n",
        ] {
            let found = check(text);
            assert_eq!(found.len(), 1, "{text:?} {found:?}");
            assert_eq!(found[0].rule, Rule::ExpectOverAllow);
            assert_eq!(found[0].line, 1, "{text:?}");
        }
        assert!(check("quote! { #[allow(dead_code)] fn f() {} }\n").is_empty());
    }

    #[test]
    fn configured_lint_lists_change_which_allows_are_reported() {
        let reported = |text: &str, config: &str| {
            let config = StyleConfig::from_toml(config).expect("the policy parses");
            check_rust_source("src/lib.rs", text, &config).len()
        };
        let dead_code = "#[allow(dead_code)]\nfn f() {}\n";
        let unknown_lints = "#[allow(unknown_lints)]\nfn f() {}\n";
        for (text, config, expected) in [
            // A whitelist accepts the lint the built-in policy reports.
            (
                dead_code,
                "[expect-over-allow]\nwhitelist = [\"dead_code\"]\n",
                0,
            ),
            (
                dead_code,
                "[expect-over-allow]\nblacklist = [\"dead_code\"]\n",
                1,
            ),
            // The blacklist re-denies a name from the built-in allow-only set.
            (
                unknown_lints,
                "[expect-over-allow]\nblacklist = [\"unknown_lints\"]\n",
                1,
            ),
            // The whitelist wins over the blacklist.
            (
                unknown_lints,
                "[expect-over-allow]\nwhitelist = [\"unknown_lints\"]\nblacklist = [\"unknown_lints\"]\n",
                0,
            ),
            // Prefix and suffix patterns match like `qualified-name`'s.
            (
                "#[allow(clippy::all)]\nfn f() {}\n",
                "[expect-over-allow]\nwhitelist = [\"clippy::*\"]\n",
                0,
            ),
            (
                "#[allow(clippy::pedantic)]\nfn f() {}\n",
                "[expect-over-allow]\nwhitelist = [\"*::pedantic\"]\n",
                0,
            ),
        ] {
            assert_eq!(reported(text, config), expected, "{text:?} {config:?}");
        }
    }
}
