use crate::domain::{GenerationRequest, GenerationResult, LlmError};

pub trait LlmProvider: Send + Sync {
    fn provider_id(&self) -> &str;

    fn supports_model(&self, model_id: &str) -> bool;

    fn generate(&self, request: &GenerationRequest) -> Result<GenerationResult, LlmError>;
}
