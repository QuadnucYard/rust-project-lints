//! Structured diagnostics shared by every style rule.

use crate::{Level, Rule};

/// One source line in a diagnostic excerpt.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ContextLine {
    /// 1-based source line number.
    pub line: usize,
    /// Source text without its line ending.
    pub text: String,
    /// Whether this is the line that triggered the violation.
    pub highlighted: bool,
}

/// A bounded excerpt around the offending source line.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct SourceContext {
    /// Lines included in source order. The offending line is always included.
    pub lines: Vec<ContextLine>,
}

impl SourceContext {
    /// Capture the offending line and one neighboring line on either side.
    pub(crate) fn around(text: &str, line: usize) -> Self {
        let lines = text.lines().collect::<Vec<_>>();
        if lines.is_empty() {
            return Self { lines: Vec::new() };
        }
        let offending = line.clamp(1, lines.len());
        let start = offending.saturating_sub(1).max(1);
        let end = (offending + 1).min(lines.len());
        let context = (start..=end)
            .map(|line_number| ContextLine {
                line: line_number,
                text: lines[line_number - 1].to_owned(),
                highlighted: line_number == offending,
            })
            .collect();
        Self { lines: context }
    }
}

/// The offending columns of one line: 0-based, `start` inclusive, `end`
/// exclusive, counted in characters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ColumnSpan {
    /// The first offending character, 0-based within the line.
    pub start: usize,
    /// One past the last offending character, 0-based within the line.
    pub end: usize,
}

/// One style violation with its source location and repair context.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Violation {
    /// Workspace-relative file path with forward slashes.
    pub path: String,
    /// 1-based line number of the offending line.
    pub line: usize,
    /// The offending columns, when the rule knows them; the report underlines
    /// exactly these, the whole line otherwise.
    pub columns: Option<ColumnSpan>,
    /// The violated standing rule.
    pub rule: Rule,
    /// The severity the report renders and the exit decision reads: the
    /// rule's default level, unless the style config's `[levels]` section
    /// overrides it.
    pub level: Level,
    /// Extra evidence such as the observed and expected dependency groups.
    pub detail: Option<String>,
    /// Source excerpt containing the offending line and its neighbors.
    pub context: SourceContext,
}

impl Violation {
    pub(crate) fn new(path: &str, line: usize, rule: Rule, text: &str) -> Self {
        Self {
            path: path.to_owned(),
            line,
            columns: None,
            rule,
            level: rule.default_level(),
            detail: None,
            context: SourceContext::around(text, line),
        }
    }

    /// Narrow the report from the whole offending line to these columns.
    pub(crate) fn narrowed_to(mut self, columns: ColumnSpan) -> Self {
        self.columns = Some(columns);
        self
    }

    pub(crate) fn with_detail(
        path: &str,
        line: usize,
        rule: Rule,
        text: &str,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            detail: Some(detail.into()),
            ..Self::new(path, line, rule, text)
        }
    }
}
