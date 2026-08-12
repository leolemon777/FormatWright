use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub type Result<T> = std::result::Result<T, FormatWrightError>;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    InputInvalid,
    InputChanged,
    Unsupported,
    EngineMissing,
    EngineIncompatible,
    PolicyBlocked,
    ResourceExhausted,
    ExecutionFailed,
    Cancelled,
    ValidationFailed,
    OutputConflict,
    StorageFailed,
    Internal,
}

impl ErrorCode {
    #[must_use]
    pub const fn exit_code(self) -> u8 {
        match self {
            Self::InputInvalid => 2,
            Self::InputChanged | Self::Unsupported => 3,
            Self::EngineMissing | Self::EngineIncompatible => 4,
            Self::ExecutionFailed => 5,
            Self::ValidationFailed => 6,
            Self::PolicyBlocked | Self::ResourceExhausted | Self::OutputConflict => 8,
            Self::Cancelled => 130,
            Self::StorageFailed | Self::Internal => 1,
        }
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{self:?}")
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Stage {
    Inspect,
    Plan,
    Execute,
    Validate,
    Commit,
    Store,
    Doctor,
}

#[derive(Clone, Debug, Deserialize, Error, Serialize)]
#[error("{code}: {message}")]
pub struct FormatWrightError {
    pub code: ErrorCode,
    pub stage: Stage,
    pub retryable: bool,
    pub message: String,
    pub user_action: String,
    pub diagnostic: Option<String>,
}

impl FormatWrightError {
    #[must_use]
    pub fn new(
        code: ErrorCode,
        stage: Stage,
        message: impl Into<String>,
        user_action: impl Into<String>,
    ) -> Self {
        Self {
            code,
            stage,
            retryable: false,
            message: message.into(),
            user_action: user_action.into(),
            diagnostic: None,
        }
    }

    #[must_use]
    pub const fn retryable(mut self, retryable: bool) -> Self {
        self.retryable = retryable;
        self
    }

    #[must_use]
    pub fn with_diagnostic(mut self, diagnostic: impl Into<String>) -> Self {
        self.diagnostic = Some(diagnostic.into());
        self
    }
}
