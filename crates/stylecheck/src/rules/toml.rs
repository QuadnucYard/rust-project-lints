//! TOML file-format rules shared by manifests and authored data files.

use toml_edit::Item;

use crate::{Rule, Violation};

/// Rules `line-endings`, `trailing-blank-line` and `missing-trailing-newline`
/// over one TOML file: LF-only line endings and exactly one trailing newline.
/// Both apply to every TOML file the tree contains — the Cargo manifests and
/// the authored data files alike — so a data file cannot drift from the
/// manifest convention.
pub fn check_toml_file(path: &str, text: &str) -> Vec<Violation> {
    let mut found = check_line_endings(path, text);
    found.extend(check_trailing_newline(path, text));
    found
}

/// Rule `line-endings`: a TOML file must end its lines with LF only. The whole
/// file yields one report anchored at its first carriage return, so a CRLF file
/// does not flood the report with one violation per line; a file that also
/// fails to parse still reports its line endings, because the parse error alone
/// does not disclose them.
fn check_line_endings(path: &str, text: &str) -> Vec<Violation> {
    let Some(offset) = text.find('\r') else {
        return Vec::new();
    };
    let crlf = text.matches("\r\n").count();
    let bare = text.matches('\r').count() - crlf;
    let detail = if bare == 0 {
        format!("{crlf} CRLF line endings")
    } else {
        format!("{crlf} CRLF line endings and {bare} bare carriage returns")
    };
    vec![Violation::with_detail(
        path,
        line_of(text, offset),
        Rule::TomlLineEnding,
        text,
        detail,
    )]
}

/// Rules `trailing-blank-line` and `missing-trailing-newline`: a TOML file ends
/// with exactly one trailing newline. A missing final newline leaves the last
/// line ambiguous in a diff and a trailing blank line adds noise after the last
/// entry; either is reported. A CRLF file is left to the line-ending rule so it
/// is not reported twice for the same trailing bytes.
fn check_trailing_newline(path: &str, text: &str) -> Vec<Violation> {
    if text.is_empty() || text.contains('\r') {
        return Vec::new();
    }
    let Some(last_content) = text.rfind(|ch: char| !ch.is_whitespace()) else {
        return Vec::new();
    };
    // `rfind` returns the start of a character, so advance by its full UTF-8
    // width: a multibyte final character must not be split at a byte boundary.
    let content_end = last_content
        + text[last_content..]
            .chars()
            .next()
            .map_or(0, char::len_utf8);
    let tail = &text[content_end..];
    // Exactly one newline closes the content line. Whitespace before that
    // newline belongs to the content line, not to a new blank line.
    if tail.ends_with('\n') && tail.matches('\n').count() == 1 {
        return Vec::new();
    }
    if !tail.contains('\n') {
        let line = text.matches('\n').count() + 1;
        return vec![Violation::new(
            path,
            line,
            Rule::TomlMissingTrailingNewline,
            text,
        )];
    }
    // A blank line follows the content line; report the first one. The stored
    // excerpt includes it, so the locus never points past the end of the file.
    let line = line_of(text, content_end) + 1;
    vec![Violation::new(
        path,
        line,
        Rule::TomlTrailingBlankLine,
        text,
    )]
}

/// The 1-based line number of a byte offset in `text`.
pub(crate) fn line_of(text: &str, offset: usize) -> usize {
    text[..offset.min(text.len())].matches('\n').count() + 1
}

/// The 1-based line on which a TOML item starts.
pub(crate) fn span_line(item: &Item, text: &str) -> usize {
    item.span().map_or(1, |span| line_of(text, span.start))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crlf_file_is_reported_once_at_its_first_carriage_return() {
        let text = "[package]\r\nname = \"probe\"\r\n\r\n[dependencies]\r\nfoundation-alpha.workspace = true\r\n";
        let found = check_toml_file("Cargo.toml", text);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].rule, Rule::TomlLineEnding);
        assert_eq!(found[0].line, 1);
        assert!(
            found[0]
                .detail
                .as_deref()
                .is_some_and(|detail| detail.contains("CRLF"))
        );
    }

    #[test]
    fn trailing_blank_line_is_reported() {
        for text in [
            "[package]\nname = \"probe\"\n\n",
            "[package]\nname = \"probe\"\n   \n",
        ] {
            let found = check_toml_file("Cargo.toml", text);
            assert_eq!(found.len(), 1, "{found:?}");
            assert_eq!(found[0].rule, Rule::TomlTrailingBlankLine);
        }
    }

    #[test]
    fn missing_final_newline_is_reported() {
        let text = "[package]\nname = \"probe\"";
        let found = check_toml_file("Cargo.toml", text);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].rule, Rule::TomlMissingTrailingNewline);
        assert_eq!(found[0].line, 2);
    }

    #[test]
    fn multibyte_final_content_is_not_split_at_a_byte_boundary() {
        // The final non-whitespace character is `é` (two bytes), so the tail
        // slice must start after the whole character, not in the middle.
        for text in ["# café\n", "# café   \n"] {
            let found = check_toml_file("data.toml", text);
            assert!(found.is_empty(), "{text:?}: {found:?}");
        }

        let found = check_toml_file("data.toml", "# café");
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].rule, Rule::TomlMissingTrailingNewline);
        assert_eq!(found[0].line, 1);

        let found = check_toml_file("data.toml", "# café\n\n");
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].rule, Rule::TomlTrailingBlankLine);
        assert_eq!(found[0].line, 2);
    }

    #[test]
    fn trailing_spaces_do_not_make_a_blank_line() {
        // Spaces on the last content line are not a separate blank line.
        let found = check_toml_file("Cargo.toml", "[package]\nname = \"probe\"   \n");
        assert!(found.is_empty(), "{found:?}");

        // Without the closing newline the file is missing T3, not a blank line.
        let found = check_toml_file("Cargo.toml", "[package]\nname = \"probe\"   ");
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].rule, Rule::TomlMissingTrailingNewline);
        assert_eq!(found[0].line, 2);
    }
}
