use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum LlmError {
    #[error("validation failed: {message}")]
    Validation { message: String },
    #[error("provider authentication failed")]
    Auth,
    #[error("provider rate limit reached")]
    RateLimited,
    #[error("provider request timed out")]
    Timeout,
    #[error("provider returned an invalid response: {message}")]
    InvalidResponse { message: String },
    #[error("provider transport failed: {message}")]
    Transport { message: String },
    #[error("internal error: {message}")]
    Internal { message: String },
}

impl LlmError {
    pub fn validation(message: impl Into<String>) -> Self {
        Self::Validation {
            message: message.into(),
        }
    }

    pub fn invalid_response(message: impl Into<String>) -> Self {
        Self::InvalidResponse {
            message: message.into(),
        }
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::Internal {
            message: message.into(),
        }
    }
}
