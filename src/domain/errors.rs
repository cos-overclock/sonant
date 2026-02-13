use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LlmErrorCategory {
    UserActionRequired,
    TemporaryFailure,
    InternalFailure,
}

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

    pub fn category(&self) -> LlmErrorCategory {
        match self {
            Self::Validation { .. } | Self::Auth => LlmErrorCategory::UserActionRequired,
            Self::RateLimited | Self::Timeout | Self::Transport { .. } => {
                LlmErrorCategory::TemporaryFailure
            }
            Self::InvalidResponse { .. } | Self::Internal { .. } => {
                LlmErrorCategory::InternalFailure
            }
        }
    }

    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::RateLimited | Self::Timeout | Self::Transport { .. }
        )
    }

    pub fn user_message(&self) -> String {
        match self {
            Self::Validation { message } => {
                format!("Please review the generation input settings: {message}")
            }
            Self::Auth => {
                "Authentication failed. Check your provider API key and configuration.".to_string()
            }
            Self::RateLimited => {
                "The provider is rate limiting requests. Please retry in a moment.".to_string()
            }
            Self::Timeout => "The provider did not respond in time. Please retry.".to_string(),
            Self::InvalidResponse { message } => {
                format!("The provider returned an invalid response format: {message}")
            }
            Self::Transport { message } => {
                format!("Could not reach the provider service: {message}")
            }
            Self::Internal { message } => {
                format!("An internal error occurred while generating: {message}")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{LlmError, LlmErrorCategory};

    #[test]
    fn category_maps_user_action_errors() {
        assert_eq!(
            LlmError::validation("prompt must not be empty").category(),
            LlmErrorCategory::UserActionRequired
        );
        assert_eq!(
            LlmError::Auth.category(),
            LlmErrorCategory::UserActionRequired
        );
    }

    #[test]
    fn category_maps_temporary_and_internal_errors() {
        assert_eq!(
            LlmError::RateLimited.category(),
            LlmErrorCategory::TemporaryFailure
        );
        assert_eq!(
            LlmError::Timeout.category(),
            LlmErrorCategory::TemporaryFailure
        );
        assert_eq!(
            LlmError::Transport {
                message: "connection reset".to_string()
            }
            .category(),
            LlmErrorCategory::TemporaryFailure
        );
        assert_eq!(
            LlmError::invalid_response("missing candidates").category(),
            LlmErrorCategory::InternalFailure
        );
    }

    #[test]
    fn is_retryable_matches_retry_policy() {
        assert!(LlmError::RateLimited.is_retryable());
        assert!(LlmError::Timeout.is_retryable());
        assert!(
            LlmError::Transport {
                message: "network".to_string()
            }
            .is_retryable()
        );
        assert!(!LlmError::Auth.is_retryable());
        assert!(!LlmError::validation("invalid request").is_retryable());
        assert!(!LlmError::invalid_response("bad JSON").is_retryable());
    }

    #[test]
    fn user_message_returns_actionable_message() {
        assert!(
            LlmError::Auth
                .user_message()
                .contains("Check your provider API key")
        );
        assert!(
            LlmError::RateLimited
                .user_message()
                .contains("rate limiting")
        );
        assert!(
            LlmError::invalid_response("expected object")
                .user_message()
                .contains("expected object")
        );
    }
}
