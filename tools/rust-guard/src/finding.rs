//! Finding type, printing and exit logic.
//!
//! Every gate reports structured findings; the process exits non-zero if
//! any exist (fail-closed).

use std::fmt;

/// A single violation, printable as `path:line: code: message`.
#[derive(Debug)]
pub struct Finding {
    pub path: String,
    pub line: usize,
    pub code: &'static str,
    pub message: String,
}

impl fmt::Display for Finding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}:{}: {}: {}",
            self.path, self.line, self.code, self.message
        )
    }
}

/// Collected findings for one subcommand run.
#[derive(Default)]
pub struct Report {
    findings: Vec<Finding>,
}

impl Report {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records a finding.
    pub fn add(&mut self, path: &str, line: usize, code: &'static str, message: String) {
        self.findings.push(Finding {
            path: path.to_owned(),
            line,
            code,
            message,
        });
    }

    /// True when nothing was reported.
    pub fn is_empty(&self) -> bool {
        self.findings.is_empty()
    }

    /// Prints all findings (sorted for stable output) to stderr.
    pub fn print(&self) {
        let mut sorted: Vec<&Finding> = self.findings.iter().collect();
        sorted.sort_by(|a, b| (&a.path, a.line, a.code).cmp(&(&b.path, b.line, b.code)));
        for f in sorted {
            eprintln!("{f}");
        }
        eprintln!("rust-guard: {} finding(s)", self.findings.len());
    }
}

/// Exit code for a finished gate: 0 = clean, 1 = findings.
pub fn exit_code(report: &Report) -> u8 {
    if report.is_empty() {
        0
    } else {
        1
    }
}
