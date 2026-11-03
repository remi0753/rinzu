use crate::SourceRange;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Severity {
    Error,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Applicability {
    MachineApplicable,
    MaybeIncorrect,
    Explanation,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiagnosticCode {
    InvalidUtf8,
    InvalidToken,
    UnterminatedComment,
    UnterminatedLiteral,
    InvalidEscape,
    IntegerOutOfRange,
    UnexpectedToken,
    ExpectedToken,
    DuplicateName,
    UnknownName,
    UnknownType,
    TypeMismatch,
    InvalidConstant,
    InvalidOperator,
    InvalidAssignment,
    InvalidControlFlow,
    InvalidAwait,
    InvalidSpawn,
    InvalidFinally,
    InvalidEntryPoint,
    NonExhaustiveSwitch,
    FutureDiscarded,
    UnsupportedSafeFfi,
    CallOnlyEscape,
    InvalidHandlerReference,
    UnsupportedLanguageFeature,
    InvalidCatchPattern,
    NonExhaustiveCatch,
    ErrorEscapesBoundary,
    IrVerification,
    ManifestNotFound,
    ManifestInvalid,
    PackageIo,
    ImportCycle,
    InvalidModulePath,
}

impl DiagnosticCode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidUtf8 => "RZ0001",
            Self::InvalidToken => "RZ0002",
            Self::UnterminatedComment => "RZ0003",
            Self::UnterminatedLiteral => "RZ0004",
            Self::InvalidEscape => "RZ0005",
            Self::IntegerOutOfRange => "RZ0006",
            Self::UnexpectedToken => "RZ0101",
            Self::ExpectedToken => "RZ0102",
            Self::DuplicateName => "RZ0201",
            Self::UnknownName => "RZ0202",
            Self::UnknownType => "RZ0203",
            Self::TypeMismatch => "RZ0301",
            Self::InvalidConstant => "RZ0310",
            Self::InvalidOperator => "RZ0302",
            Self::InvalidAssignment => "RZ0303",
            Self::InvalidControlFlow => "RZ0304",
            Self::InvalidAwait => "RZ0401",
            Self::InvalidSpawn => "RZ0402",
            Self::InvalidFinally => "RZ0403",
            Self::InvalidEntryPoint => "RZ0404",
            Self::NonExhaustiveSwitch => "RZ0405",
            Self::FutureDiscarded => "RZ0406",
            Self::UnsupportedSafeFfi => "RZ0407",
            Self::CallOnlyEscape => "RZ0408",
            Self::InvalidHandlerReference => "RZ0409",
            Self::UnsupportedLanguageFeature => "RZ0410",
            Self::InvalidCatchPattern => "RZ0411",
            Self::NonExhaustiveCatch => "RZ0412",
            Self::ErrorEscapesBoundary => "RZ0413",
            Self::IrVerification => "RZ0501",
            Self::ManifestNotFound => "RZ0601",
            Self::ManifestInvalid => "RZ0602",
            Self::PackageIo => "RZ0603",
            Self::ImportCycle => "RZ0604",
            Self::InvalidModulePath => "RZ0605",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Suggestion {
    pub applicability: Applicability,
    pub message: String,
    pub replacement: Option<(SourceRange, String)>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Diagnostic {
    pub code: DiagnosticCode,
    pub severity: Severity,
    pub primary: SourceRange,
    pub secondary: Vec<(SourceRange, String)>,
    pub message: String,
    pub suggestions: Vec<Suggestion>,
}

impl Diagnostic {
    #[must_use]
    pub fn error(
        code: DiagnosticCode,
        primary: SourceRange,
        message: impl Into<String>,
        suggestion: impl Into<String>,
    ) -> Self {
        Self {
            code,
            severity: Severity::Error,
            primary,
            secondary: Vec::new(),
            message: message.into(),
            suggestions: vec![Suggestion {
                applicability: Applicability::Explanation,
                message: suggestion.into(),
                replacement: None,
            }],
        }
    }

    #[must_use]
    pub fn with_secondary(mut self, range: SourceRange, message: impl Into<String>) -> Self {
        self.secondary.push((range, message.into()));
        self
    }

    #[must_use]
    pub fn with_replacement(
        mut self,
        applicability: Applicability,
        range: SourceRange,
        replacement: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        self.suggestions.push(Suggestion {
            applicability,
            message: message.into(),
            replacement: Some((range, replacement.into())),
        });
        self
    }
}
