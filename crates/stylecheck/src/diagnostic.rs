//! Structured diagnostics shared by every style rule.

use crate::Rule;

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

/// One style violation with its source location and repair context.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Violation {
    /// Workspace-relative file path with forward slashes.
    pub path: String,
    /// 1-based line number of the offending line.
    pub line: usize,
    /// The violated standing rule.
    pub rule: Rule,
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
            rule,
            detail: None,
            context: SourceContext::around(text, line),
        }
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

    /// The report for this violation: the `[id name]` header line followed by
    /// the marked source excerpt.
    pub fn report(&self) -> String {
        let detail = self
            .detail
            .as_deref()
            .map(|text| format!(": {}", text.replace(['\n', '\r'], " ")))
            .unwrap_or_default();
        let mut report = format!(
            "error: {}:{}: [{} {}] {}{}",
            self.path,
            self.line,
            self.rule.id(),
            self.rule.name(),
            self.rule.message(),
            detail
        );
        for context_line in &self.context.lines {
            let marker = if context_line.highlighted { '>' } else { ' ' };
            report.push_str(&format!(
                "\n  {marker} {:>4} | {}",
                context_line.line, context_line.text
            ));
        }
        report
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_prints_the_rule_id_name_context_and_detail() {
        let violation = Violation::new(
            "src/lib.rs",
            2,
            Rule::ModuleDocSpacing,
            "//! docs\n\nfn item() {}\n",
        );
        let report = violation.report();
        assert_eq!(
            report.lines().next(),
            Some(
                "error: src/lib.rs:2: [S1 module-doc-spacing] module-level `//!` docs need exactly one blank line before the next item"
            )
        );
        assert_eq!(report.lines().count(), 4);
        assert!(report.lines().any(|line| line.starts_with("  >    2 |")));

        // The observed detail is appended to the header line, with its own line
        // breaks collapsed so one violation stays one line.
        let detailed = Violation::with_detail(
            "Cargo.toml",
            1,
            Rule::TomlLineEnding,
            "a = 1\r\n",
            "1 CRLF\nline ending",
        );
        assert_eq!(
            detailed.report().lines().next(),
            Some(
                "error: Cargo.toml:1: [T1 line-endings] TOML must use LF line endings, not CRLF: 1 CRLF line ending"
            )
        );
    }
}
