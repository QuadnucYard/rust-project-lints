//! Source-layout rules.

use std::collections::BTreeSet;

use proc_macro2::{TokenStream, TokenTree};

use crate::{Rule, StyleConfig, Violation, rules::qualified::check_qualified_names};

/// Check one Rust source file against the source layout rules. A path the
/// config exempts skips the rules that read a file as authored source
/// (`module-doc-spacing` and `qualified-name`); the structural rules still
/// apply.
pub fn check_rust_source(path: &str, text: &str, config: &StyleConfig) -> Vec<Violation> {
    let mut found = Vec::new();
    if !config.is_exempt(path) {
        found.extend(check_module_doc_spacing(path, text));
        found.extend(check_qualified_names(path, text, config.qualified()));
    }
    found.extend(check_test_mod_spacing(path, text));
    found.extend(check_mod_before_use(path, text));
    found
}

/// Rule `module-doc-spacing`: a module doc block — its `//!` lines plus the
/// inner attributes (`#![doc = …]` among them) and plain `//` comments that
/// follow them — must be followed by exactly one blank line before the next
/// item.
fn check_module_doc_spacing(path: &str, text: &str) -> Vec<Violation> {
    // Parse the source for attribute extents so a multi-line inner attribute
    // joins the header as a whole. A file without inner attributes needs no
    // token pass, and an untokenizable file keeps the line-based fallback.
    let attribute_lines = if text.contains("#![") {
        inner_attribute_lines(text)
    } else {
        BTreeSet::new()
    };
    let mut found = Vec::new();
    let mut last_doc: Option<usize> = None;
    for (index, line) in text.lines().enumerate() {
        let line_no = index + 1;
        let trimmed = line.trim_start();
        // A `//!` line opens the block; any other header line only extends a
        // block that is already open.
        if trimmed.starts_with("//!")
            || (last_doc.is_some()
                && (is_header_line(trimmed) || attribute_lines.contains(&line_no)))
        {
            last_doc = Some(line_no);
        } else if !line.trim().is_empty()
            && let Some(doc) = last_doc.take()
            && line_no - doc - 1 != 1
        {
            found.push(Violation::new(path, line_no, Rule::ModuleDocSpacing, text));
        }
    }
    found
}

/// A line that belongs to the module header rather than to the items the
/// blank line separates: an inner attribute (`#![doc = …]` among them) or a
/// plain `//` comment. A `///` doc comment is excluded — it documents the
/// item below it, so it starts that item.
fn is_header_line(trimmed: &str) -> bool {
    trimmed.starts_with("#![") || (trimmed.starts_with("//") && !trimmed.starts_with("///"))
}

/// Every source line covered by a top-level inner attribute (`#![…]`). The
/// token stream knows where the attribute's bracket group ends, including
/// strings and nested brackets, so each continuation line can join the module
/// header instead of looking like the first item.
fn inner_attribute_lines(text: &str) -> BTreeSet<usize> {
    let Ok(stream) = text.parse::<TokenStream>() else {
        return BTreeSet::new();
    };
    let mut tokens = stream.into_iter().peekable();
    let mut lines = BTreeSet::new();
    while let Some(token) = tokens.next() {
        let TokenTree::Punct(hash) = token else {
            continue;
        };
        if hash.as_char() != '#'
            || !matches!(tokens.peek(), Some(TokenTree::Punct(bang)) if bang.as_char() == '!')
        {
            continue;
        }
        tokens.next();
        let Some(TokenTree::Group(group)) = tokens.next() else {
            continue;
        };
        let start = group.span_open().start().line;
        let end = group.span_close().end().line;
        lines.extend(start..=end);
    }
    lines
}

/// Rule `test-mod-attribute-spacing`: a `#[cfg(test)]` attribute is glued to
/// its `mod` declaration. Rule `test-mod-body-spacing`: the test mod body does
/// not open with a blank line.
fn check_test_mod_spacing(path: &str, text: &str) -> Vec<Violation> {
    let lines: Vec<&str> = text.lines().collect();
    let mut found = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if !line.trim().starts_with("#[cfg(test)]") {
            continue;
        }
        if index + 1 < lines.len() && lines[index + 1].trim().is_empty() {
            found.push(Violation::new(
                path,
                index + 2,
                Rule::TestModBlankAfterAttribute,
                text,
            ));
        } else if index + 1 < lines.len()
            && is_test_mod_open(lines[index + 1])
            && index + 2 < lines.len()
            && lines[index + 2].trim().is_empty()
        {
            found.push(Violation::new(
                path,
                index + 3,
                Rule::TestModLeadingBlankLine,
                text,
            ));
        }
    }
    found
}

/// A `mod <name> {` line that opens a test mod.
fn is_test_mod_open(line: &str) -> bool {
    let Some(rest) = line.trim().strip_prefix("mod ") else {
        return false;
    };
    let Some(name) = rest
        .split(|ch: char| !(ch.is_alphanumeric() || ch == '_'))
        .next()
    else {
        return false;
    };
    !name.is_empty() && rest[name.len()..].trim_start().starts_with('{')
}

/// Rule `mod-before-use`: every file-module declaration (`mod name;`) precedes
/// the file's first `use` item. Inline modules (`mod name { … }`) are exempt
/// because they are definitions the file hosts in place — the trailing
/// `#[cfg(test)] mod tests { … }` block, for instance, may follow the items it
/// exercises. Only unindented items are observed: nested items live inside an
/// inline module or a function body, where this file-level ordering rule does
/// not apply.
fn check_mod_before_use(path: &str, text: &str) -> Vec<Violation> {
    let mut found = Vec::new();
    let mut seen_use = false;
    for (index, line) in text.lines().enumerate() {
        match classify_item_line(line) {
            Some(ItemLine::Use) => seen_use = true,
            Some(ItemLine::ModuleDeclaration) if seen_use => {
                found.push(Violation::new(path, index + 1, Rule::ModBeforeUse, text));
            }
            _ => {}
        }
    }
    found
}

/// One top-level item line the module/import ordering rule observes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ItemLine {
    /// A `use` item, including `pub use` re-exports.
    Use,
    /// A file-module declaration such as `mod parser;`.
    ModuleDeclaration,
}

/// Classify one unindented line as a `use` item or a file-module declaration.
/// Indented lines are inside an inline module or a function body and are not
/// observed; an inline module (`mod name {`) is not a declaration.
fn classify_item_line(line: &str) -> Option<ItemLine> {
    if line.starts_with(' ') || line.starts_with('\t') {
        return None;
    }
    let (keyword, remainder) = item_keyword(line)?;
    match keyword {
        "use" => Some(ItemLine::Use),
        "mod" => {
            // Skip the module name; only a terminating `;` marks a file
            // module. A `{` (or a name with no terminator at all) is inline.
            let name_end = remainder.find(|ch: char| !(ch.is_alphanumeric() || ch == '_'));
            let after_name = name_end.map_or("", |index| remainder[index..].trim_start());
            after_name
                .starts_with(';')
                .then_some(ItemLine::ModuleDeclaration)
        }
        _ => None,
    }
}

/// The leading `use`/`mod` keyword of an item line and the text after it, with
/// any `pub`/`pub(...)` visibility prefix removed.
fn item_keyword(line: &str) -> Option<(&str, &str)> {
    let mut rest = line;
    if let Some(after_pub) = rest.strip_prefix("pub") {
        rest = after_pub.trim_start();
        if let Some(after_paren) = rest.strip_prefix('(') {
            let close = after_paren.find(')')?;
            rest = after_paren[close + 1..].trim_start();
        }
    }
    let end = rest
        .find(|ch: char| !(ch.is_alphanumeric() || ch == '_'))
        .unwrap_or(rest.len());
    if end == 0 {
        return None;
    }
    let keyword = &rest[..end];
    matches!(keyword, "use" | "mod").then_some((keyword, rest[end..].trim_start()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Check one source text with the default policy.
    fn check(path: &str, text: &str) -> Vec<Violation> {
        check_rust_source(path, text, &StyleConfig::default())
    }

    #[test]
    fn module_doc_block_needs_exactly_one_blank_line() {
        for clean in [
            "//! Module docs.\n//! More docs.\n\nuse std::fmt;\n",
            "//! Module docs.\n//! Only docs.\n",
        ] {
            assert!(check("src/lib.rs", clean).is_empty(), "{clean:?}");
        }
        for (text, line) in [
            ("//! Module docs.\nuse std::fmt;\n", 2),
            ("//! Module docs.\n\n\nuse std::fmt;\n", 4),
        ] {
            let found = check("src/lib.rs", text);
            assert_eq!(found.len(), 1, "{found:?}");
            assert_eq!(found[0].rule, Rule::ModuleDocSpacing);
            assert_eq!(found[0].line, line, "{text:?}");
        }
    }

    #[test]
    fn inner_attributes_join_the_module_doc_block() {
        for clean in [
            "//! Docs.\n#![doc = include_str!(\"../README.md\")]\n\nuse std::fmt;\n",
            "//! Docs.\n#![allow(dead_code)]\n#![warn(missing_docs)]\n\nuse std::fmt;\n",
            "//! Docs.\n#![allow(\n    dead_code,\n)]\n\nuse std::fmt;\n",
            "//! Docs.\n#![doc = r#\"\nSome text\n\"#]\n\nuse std::fmt;\n",
            "//! Docs.\n\n#![allow(dead_code)]\n\nuse std::fmt;\n",
            // Without a `//!` block an attribute opens nothing and is not checked.
            "#![doc = include_str!(\"../README.md\")]\n#![warn(missing_docs)]\n\nuse std::fmt;\n",
            "#![allow(dead_code)]\nuse std::fmt;\n",
        ] {
            assert!(check("src/lib.rs", clean).is_empty(), "{clean:?}");
        }
        // The item after the header still needs its blank line.
        for (text, line) in [
            (
                "//! Docs.\n#![doc = include_str!(\"../README.md\")]\nuse std::fmt;\n",
                3,
            ),
            ("//! Docs.\n#![allow(dead_code)]\n\n\nuse std::fmt;\n", 5),
            (
                "//! Docs.\n#![allow(\n    dead_code,\n)]\nuse std::fmt;\n",
                5,
            ),
        ] {
            let found = check("src/lib.rs", text);
            assert_eq!(found.len(), 1, "{found:?}");
            assert_eq!(found[0].rule, Rule::ModuleDocSpacing);
            assert_eq!(found[0].line, line, "{text:?}");
        }
    }

    #[test]
    fn plain_comments_join_the_module_doc_block() {
        for clean in [
            "//! Docs.\n// A note.\n\nuse std::fmt;\n",
            "//! Docs.\n// One note.\n// Another note.\n\nuse std::fmt;\n",
            "//! Docs.\n#![allow(dead_code)]\n// A note.\n\nuse std::fmt;\n",
            // A `///` doc comment stays with the item below it.
            "//! Docs.\n\n/// Item docs.\npub fn f() {}\n",
            "// Without `//!` docs a comment opens no block.\nuse std::fmt;\n",
        ] {
            assert!(check("src/lib.rs", clean).is_empty(), "{clean:?}");
        }
        // The item after the header still needs its blank line.
        for (text, line) in [
            ("//! Docs.\n// A note.\nuse std::fmt;\n", 3),
            ("//! Docs.\n// A note.\n\n\nuse std::fmt;\n", 5),
            ("//! Docs.\n/// Item docs.\npub fn f() {}\n", 2),
        ] {
            let found = check("src/lib.rs", text);
            assert_eq!(found.len(), 1, "{found:?}");
            assert_eq!(found[0].rule, Rule::ModuleDocSpacing);
            assert_eq!(found[0].line, line, "{text:?}");
        }
    }

    #[test]
    fn exempt_paths_skip_the_source_layout_rules() {
        let text = "//! Wrong thing must not compile.\nfn f() -> registry::DuplicateSchema {\n    todo!()\n}\n";
        let config = StyleConfig::from_toml("exempt-paths = [\"tests/ui/\"]\n")
            .expect("the exemption list parses");
        assert!(check_rust_source("tests/ui/fail_x.rs", text, &config).is_empty());
        let found = check_rust_source("src/lib.rs", text, &config);
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(
            found
                .iter()
                .any(|violation| violation.rule == Rule::ModuleDocSpacing)
        );
        assert!(
            found
                .iter()
                .any(|violation| violation.rule == Rule::QualifiedName)
        );
        // The structural rules still apply to an exempt path.
        let unstructured = "#[cfg(test)]\n\nmod tests {\n}\n";
        let found = check_rust_source("tests/ui/fail_x.rs", unstructured, &config);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].rule, Rule::TestModBlankAfterAttribute);
        // Without the exemption the same file is checked like any other.
        assert_eq!(check("tests/ui/fail_x.rs", text).len(), 2);
    }

    #[test]
    fn test_mod_is_glued_to_its_attribute_without_leading_blank() {
        assert!(
            check(
                "src/lib.rs",
                "#[cfg(test)]\nmod tests {\n    use super::*;\n}\n"
            )
            .is_empty()
        );
        for (text, rule, line) in [
            (
                "#[cfg(test)]\n\nmod tests {\n}\n",
                Rule::TestModBlankAfterAttribute,
                2,
            ),
            (
                "#[cfg(test)]\nmod tests {\n\n    use super::*;\n}\n",
                Rule::TestModLeadingBlankLine,
                3,
            ),
        ] {
            let found = check("src/lib.rs", text);
            assert_eq!(found.len(), 1, "{found:?}");
            assert_eq!(found[0].rule, rule);
            assert_eq!(found[0].line, line);
        }
    }

    #[test]
    fn file_modules_must_precede_use_items() {
        let clean = "mod parser;\nmod writer;\n\nuse std::fmt;\n\npub use parser::Parser;\n";
        assert!(check("src/lib.rs", clean).is_empty());
        for text in [
            "use std::fmt;\n\nmod parser;\n",
            "use std::fmt;\n\npub(crate) mod support;\n",
        ] {
            let found = check("src/lib.rs", text);
            assert_eq!(found.len(), 1, "{found:?}");
            assert_eq!(found[0].rule, Rule::ModBeforeUse);
            assert_eq!(found[0].line, 3);
        }
    }

    #[test]
    fn inline_modules_are_not_observed() {
        for text in [
            "use std::fmt;\n\n#[cfg(test)]\nmod tests {\n    use super::*;\n}\n",
            "use std::fmt;\n\nmod outer {\n    use super::*;\n    mod inner;\n}\n",
        ] {
            assert!(check("src/lib.rs", text).is_empty(), "{text:?}");
        }
    }
}
