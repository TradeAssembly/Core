use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticSeverity {
    Error,
    Warning,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Diagnostic {
    pub severity: DiagnosticSeverity,
    pub code: String,
    pub pointer: String,
    pub message: String,
    pub layer: u8,
}

impl Diagnostic {
    pub(crate) fn error(
        layer: u8,
        code: impl Into<String>,
        pointer: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            severity: DiagnosticSeverity::Error,
            code: code.into(),
            pointer: pointer.into(),
            message: message.into(),
            layer,
        }
    }

    pub fn display_line(&self) -> String {
        format!("[{}] {} {}", self.code, self.pointer, self.message)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValidationReport {
    pub valid: bool,
    pub source_family: Option<String>,
    pub spec_hash: Option<String>,
    pub diagnostics: Vec<Diagnostic>,
}
