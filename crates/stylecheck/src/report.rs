//! Rendering violations as human-readable reports with `codespan-reporting`.
//!
//! Each violation becomes one rustc-style report: an `error[id name]` or
//! `warning[id name]` header carrying the violation's level, the
//! `path:line:column` locus, the offending span underlined — the whole
//! line, narrowed to the violating columns when the rule knows them — with
//! one unmarked context line on either side supplied by the renderer, plus
//! the rule-specific observed detail as a note. [`write_reports`] emits to
//! any colored writer (the command's standard error stream);
//! [`render_reports`] renders the same reports as plain text for tests.

use core::ops::Range;

use codespan_reporting::{
    diagnostic::{Diagnostic, Label, Severity},
    files::{Error as FilesError, Files},
    term::{self, Config, termcolor::WriteColor},
};

use crate::{
    Level,
    diagnostic::{ColumnSpan, Violation},
};

/// The rendering configuration shared by both writers: one unmarked context
/// line on either side of a label, supplied by the renderer itself.
fn config() -> Config {
    Config {
        before_label_lines: 1,
        after_label_lines: 1,
        ..Config::default()
    }
}

/// Write every violation to `writer` as one report, in the given order. A
/// violation of a rule disabled through `[levels]` renders nothing.
pub fn write_reports(
    writer: &mut dyn WriteColor,
    violations: &[Violation],
) -> Result<(), FilesError> {
    let config = config();
    for violation in violations {
        let file = ExcerptFile::new(violation);
        if let Some(diagnostic) = violation_diagnostic(violation, &file) {
            term::emit_to_write_style(writer, &config, &file, &diagnostic)?;
        }
    }
    Ok(())
}

/// The same reports as plain text, without color; one per violation.
pub fn render_reports(violations: &[Violation]) -> String {
    let config = config();
    let mut rendered = String::new();
    for violation in violations {
        let file = ExcerptFile::new(violation);
        if let Some(diagnostic) = violation_diagnostic(violation, &file) {
            rendered
                .push_str(&term::emit_into_string(&config, &file, &diagnostic).unwrap_or_default());
        }
    }
    rendered
}

/// The report of one violation: the rule as the diagnostic code and message,
/// the severity of the violation as the diagnostic level, one primary label
/// over the offending span — the whole line, narrowed to its columns when the
/// rule knows them — and the observed detail as a note. The renderer draws
/// the labeled line and the context lines around it, so the neighbors need no
/// labels of their own. `None` means the rule is disabled and reports
/// nothing.
fn violation_diagnostic(violation: &Violation, file: &ExcerptFile<'_>) -> Option<Diagnostic<()>> {
    let labels = violation
        .context
        .lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| {
            if !line.highlighted {
                return None;
            }
            let range = violation.columns.map_or(file.text_range(index), |columns| {
                file.column_range(index, columns)
            });
            Some(Label::primary((), range))
        })
        .collect();
    let severity = match violation.level {
        Level::Off => return None,
        Level::Warn => Severity::Warning,
        Level::Error => Severity::Error,
    };
    let mut diagnostic = Diagnostic::new(severity)
        .with_code(format!("{} {}", violation.rule.id(), violation.rule.name()))
        .with_message(violation.rule.message())
        .with_labels(labels);
    if let Some(detail) = &violation.detail {
        diagnostic = diagnostic.with_notes(vec![detail.clone()]);
    }
    Some(diagnostic)
}

/// One violation's excerpt presented to `codespan-reporting` as the whole
/// file: [`Files::line_number`] reports the excerpt's real line numbers, so
/// reports point at true locations without carrying whole source files.
struct ExcerptFile<'a> {
    /// The workspace-relative path shown as the file name.
    path: &'a str,
    /// The excerpt lines joined with `\n`.
    source: String,
    /// The real 1-based line number of the first excerpt line.
    first_line: usize,
    /// The byte offset at which each excerpt line starts in `source`.
    line_starts: Vec<usize>,
}

impl<'a> ExcerptFile<'a> {
    /// Build the file view of one violation's excerpt.
    fn new(violation: &'a Violation) -> Self {
        let lines = &violation.context.lines;
        let source = lines
            .iter()
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let mut line_starts = Vec::with_capacity(lines.len());
        let mut offset = 0;
        for line in lines {
            line_starts.push(offset);
            offset += line.text.len() + 1;
        }
        Self {
            path: &violation.path,
            first_line: lines.first().map_or(1, |line| line.line),
            source,
            line_starts,
        }
    }

    /// The byte range of excerpt line `index` covering exactly its text,
    /// without the `\n` separator, so the label underlines only that line.
    fn text_range(&self, index: usize) -> Range<usize> {
        let start = self.line_starts[index];
        let end = self
            .line_starts
            .get(index + 1)
            .map_or(self.source.len(), |next| next - 1);
        start..end
    }

    /// The byte range covering `columns` of excerpt line `index`, converting
    /// the character columns to byte offsets and clamping to the line's text.
    fn column_range(&self, index: usize, columns: ColumnSpan) -> Range<usize> {
        let line = self.text_range(index);
        let text = &self.source[line.clone()];
        let start = char_offset(text, columns.start);
        let end = start + char_offset(&text[start..], columns.end.saturating_sub(columns.start));
        line.start + start..line.start + end
    }
}

impl<'a> Files<'a> for ExcerptFile<'a> {
    type FileId = ();
    type Name = &'a str;
    type Source = &'a str;

    fn name(&self, (): ()) -> Result<Self::Name, FilesError> {
        Ok(self.path)
    }

    // The elided return borrows from `self` like `SimpleFile`'s, which keeps
    // the impl compatible with the trait's `&'a self` receiver.
    fn source(&self, (): ()) -> Result<&str, FilesError> {
        Ok(self.source.as_str())
    }

    fn line_index(&self, (): (), byte_index: usize) -> Result<usize, FilesError> {
        let index = self
            .line_starts
            .binary_search(&byte_index)
            .unwrap_or_else(|next| next.saturating_sub(1));
        Ok(index.min(self.line_starts.len().saturating_sub(1)))
    }

    fn line_number(&self, (): (), line_index: usize) -> Result<usize, FilesError> {
        let index = line_index.min(self.line_starts.len().saturating_sub(1));
        Ok(self.first_line + index)
    }

    fn line_range(&self, (): (), line_index: usize) -> Result<Range<usize>, FilesError> {
        if line_index >= self.line_starts.len() {
            return Err(FilesError::LineTooLarge {
                given: line_index,
                max: self.line_starts.len().saturating_sub(1),
            });
        }
        // Like `SimpleFile`, the range carries the line's `\n` separator; the
        // renderer trims it when displaying.
        let end = self
            .line_starts
            .get(line_index + 1)
            .copied()
            .unwrap_or(self.source.len());
        Ok(self.line_starts[line_index]..end)
    }
}

/// The byte offset of the `chars`-th character of `text`, or the text's
/// length when `chars` reaches past its end.
fn char_offset(text: &str, chars: usize) -> usize {
    text.char_indices()
        .nth(chars)
        .map_or(text.len(), |(offset, _)| offset)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Rule;

    /// A file whose violation sits far enough down that excerpt-relative line
    /// numbers (1, 2, 3) would differ from the real ones.
    fn late_source(line_count: usize) -> String {
        (1..line_count)
            .map(|number| format!("line {number}\n"))
            .collect()
    }

    #[test]
    fn reports_carry_the_code_message_locus_and_excerpt() {
        let violation = Violation::new(
            "src/lib.rs",
            2,
            Rule::ModuleDocSpacing,
            "//! docs\nfn item() {}\nlet after = 3;\n",
        );
        let rendered = render_reports(&[violation]);
        assert!(
            rendered.contains("error[S1 module-doc-spacing]:"),
            "{rendered}"
        );
        assert!(
            rendered.contains(
                "module-level `//!` docs need exactly one blank line before the next item"
            ),
            "{rendered}"
        );
        assert!(rendered.contains("src/lib.rs:2:1"), "{rendered}");
        // The offending line is underlined and both neighbors appear beside it
        // as unmarked context lines: no marker row separates them.
        assert!(rendered.contains("//! docs"), "{rendered}");
        assert!(rendered.contains("fn item() {}"), "{rendered}");
        assert!(rendered.contains("let after = 3;"), "{rendered}");
        assert!(rendered.contains("^^^"), "{rendered}");
        assert!(
            rendered.contains("1 │ //! docs\n2 │ fn item() {}"),
            "{rendered}"
        );
        assert!(!rendered.contains("---"), "{rendered}");
    }

    #[test]
    fn excerpts_show_real_line_numbers() {
        let text = format!("{}fn item() {{}}\n", late_source(42));
        let violation = Violation::new("src/deep.rs", 42, Rule::ModBeforeUse, &text);
        let rendered = render_reports(&[violation]);
        assert!(rendered.contains("src/deep.rs:42:1"), "{rendered}");
        // The gutter shows the real 41 and 42, not excerpt-relative 1 and 2.
        assert!(rendered.contains("41 │ line 41"), "{rendered}");
        assert!(rendered.contains("42 │ fn item() {}"), "{rendered}");
        assert!(!rendered.contains(" 1 │ "), "{rendered}");
    }

    #[test]
    fn details_render_as_notes() {
        let violation = Violation::with_detail(
            "Cargo.toml",
            1,
            Rule::TomlLineEnding,
            "a = 1\r\n",
            "1 CRLF line ending",
        );
        let rendered = render_reports(&[violation]);
        assert!(rendered.contains("= 1 CRLF line ending"), "{rendered}");
    }

    #[test]
    fn known_columns_underline_exactly_the_offending_span() {
        // The columns name `registry::DuplicateSchema` (characters 15..40 of
        // line 1), so the carets must skip the 15 leading characters and
        // cover nothing else on the line.
        let violation = Violation::new(
            "src/lib.rs",
            1,
            Rule::QualifiedName,
            "fn decode() -> registry::DuplicateSchema {}\n",
        )
        .narrowed_to(ColumnSpan { start: 15, end: 40 });
        let rendered = render_reports(&[violation]);
        let expected = format!("  │ {}{}", " ".repeat(15), "^".repeat(25));
        assert!(rendered.lines().any(|line| line == expected), "{rendered}");
    }

    #[test]
    fn blank_offending_lines_still_render() {
        // `test-mod-body-spacing` fires on a blank line: an empty label range
        // must render, not panic or vanish.
        let violation = Violation::new(
            "src/lib.rs",
            3,
            Rule::TestModLeadingBlankLine,
            "#[cfg(test)]\nmod tests {\n\n    fn case() {}\n}\n",
        );
        let rendered = render_reports(&[violation]);
        assert!(rendered.contains("src/lib.rs:3:1"), "{rendered}");
        assert!(rendered.contains("mod tests {"), "{rendered}");
    }

    #[test]
    fn warn_level_violations_render_as_warnings() {
        let mut violation = Violation::new(
            "src/lib.rs",
            3,
            Rule::ModBeforeUse,
            "use std::fmt;\n\nmod parser;\n",
        );
        violation.level = Level::Warn;
        let rendered = render_reports(&[violation]);
        assert!(
            rendered.contains("warning[S5 mod-before-use]"),
            "{rendered}"
        );
        assert!(!rendered.contains("error[S5"), "{rendered}");
    }
}
