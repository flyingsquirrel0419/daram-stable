//! Diagnostic (error / warning / note) reporting.

use crate::source::Span;
use std::fmt;

/// Severity level of a diagnostic message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    /// A hint or suggestion that does not prevent compilation.
    Note,
    /// A potential problem; compilation continues.
    Warning,
    /// A hard error that prevents successful compilation.
    Error,
}

impl fmt::Display for Level {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Level::Note => f.write_str("note"),
            Level::Warning => f.write_str("warning"),
            Level::Error => f.write_str("error"),
        }
    }
}

/// A secondary label attached to a diagnostic (e.g. "defined here").
#[derive(Debug, Clone)]
pub struct Label {
    pub span: Span,
    pub message: String,
}

impl Label {
    pub fn new(span: Span, message: impl Into<String>) -> Self {
        Self {
            span,
            message: message.into(),
        }
    }
}

/// A single compiler diagnostic with optional sub-labels and help text.
#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub level: Level,
    pub code: Option<String>,
    pub message: String,
    pub primary_span: Option<Span>,
    pub labels: Vec<Label>,
    pub notes: Vec<String>,
}

impl Diagnostic {
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            level: Level::Error,
            code: None,
            message: message.into(),
            primary_span: None,
            labels: Vec::new(),
            notes: Vec::new(),
        }
    }

    pub fn warning(message: impl Into<String>) -> Self {
        Self {
            level: Level::Warning,
            code: None,
            message: message.into(),
            primary_span: None,
            labels: Vec::new(),
            notes: Vec::new(),
        }
    }

    pub fn note(message: impl Into<String>) -> Self {
        Self {
            level: Level::Note,
            code: None,
            message: message.into(),
            primary_span: None,
            labels: Vec::new(),
            notes: Vec::new(),
        }
    }

    pub fn with_code(mut self, code: impl Into<String>) -> Self {
        self.code = Some(code.into());
        self
    }

    pub fn with_span(mut self, span: Span) -> Self {
        self.primary_span = Some(span);
        self
    }

    pub fn with_label(mut self, label: Label) -> Self {
        self.labels.push(label);
        self
    }

    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.notes.push(note.into());
        self
    }
}

/// Renders diagnostics to a string for terminal output.
pub struct Renderer<'sm> {
    source_map: &'sm crate::source::SourceMap,
    use_color: bool,
}

impl<'sm> Renderer<'sm> {
    pub fn new(source_map: &'sm crate::source::SourceMap, use_color: bool) -> Self {
        Self {
            source_map,
            use_color,
        }
    }

    pub fn render(&self, diag: &Diagnostic) -> String {
        let mut out = String::new();

        // Header: "error[E001]: message"
        let level_str = match diag.level {
            Level::Error => {
                if self.use_color {
                    "\x1b[31merror\x1b[0m"
                } else {
                    "error"
                }
            }
            Level::Warning => {
                if self.use_color {
                    "\x1b[33mwarning\x1b[0m"
                } else {
                    "warning"
                }
            }
            Level::Note => {
                if self.use_color {
                    "\x1b[36mnote\x1b[0m"
                } else {
                    "note"
                }
            }
        };

        if let Some(code) = &diag.code {
            out.push_str(&format!("{}[{}]: {}\n", level_str, code, diag.message));
        } else {
            out.push_str(&format!("{}: {}\n", level_str, diag.message));
        }

        // Primary location
        if let Some(span) = &diag.primary_span {
            if let Some(file) = self.source_map.try_get(span.file) {
                let (line, col) = file.line_col(span.start);
                out.push_str(&format!("  --> {}:{}:{}\n", file.name, line, col));
                self.render_source_line(&mut out, file, span, "");
            }
        }

        // Secondary labels
        for label in &diag.labels {
            if let Some(file) = self.source_map.try_get(label.span.file) {
                let (line, col) = file.line_col(label.span.start);
                out.push_str(&format!("  --> {}:{}:{}\n", file.name, line, col));
                self.render_source_line(&mut out, file, &label.span, &label.message);
            }
        }

        // Notes
        for note in &diag.notes {
            out.push_str(&format!("  = note: {}\n", note));
        }

        out
    }

    fn render_source_line(
        &self,
        out: &mut String,
        file: &crate::source::SourceFile,
        span: &Span,
        label: &str,
    ) {
        let (line, col) = file.line_col(span.start);
        let line_text = file.line_text(line);
        let gutter = format!("{}", line);
        let padding = " ".repeat(gutter.len());

        out.push_str(&format!("{} |\n", padding));
        out.push_str(&format!("{} | {}\n", gutter, line_text));

        // Underline carets
        let caret_start = (col as usize).saturating_sub(1);
        let span_len = (span.end.0 - span.start.0) as usize;
        let caret_len = span_len.max(1);
        let carets = "^".repeat(caret_len);
        let spaces = " ".repeat(caret_start);

        if label.is_empty() {
            out.push_str(&format!("{} | {}{}\n", padding, spaces, carets));
        } else {
            out.push_str(&format!("{} | {}{} {}\n", padding, spaces, carets, label));
        }
    }
}
