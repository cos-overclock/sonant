use sonant::domain::{
    GenerationMode, GenerationParams, GenerationRequest, LlmError, MidiReferenceSummary, ModelRef,
};

use super::{
    DEFAULT_BPM, DEFAULT_COMPLEXITY, DEFAULT_DENSITY, DEFAULT_MAX_TOKENS, DEFAULT_TEMPERATURE,
    DEFAULT_TOP_P, DEFAULT_VARIATION_COUNT, GPUI_HELPER_REQUEST_ID_PREFIX,
};

#[derive(Debug, Clone)]
pub(super) struct PromptSubmissionModel {
    next_request_number: u64,
    model: ModelRef,
}

impl PromptSubmissionModel {
    pub(super) fn new(model: ModelRef) -> Self {
        Self {
            next_request_number: 1,
            model,
        }
    }

    pub(super) fn prepare_request(
        &mut self,
        prompt: String,
        references: Vec<MidiReferenceSummary>,
    ) -> Result<GenerationRequest, LlmError> {
        let request_id = format!(
            "{GPUI_HELPER_REQUEST_ID_PREFIX}-{}",
            self.next_request_number
        );
        self.next_request_number = self.next_request_number.saturating_add(1);
        build_generation_request_with_prompt_validation(
            request_id,
            self.model.clone(),
            GenerationMode::Melody,
            prompt,
            references,
        )
    }

    pub(super) fn set_model(&mut self, model: ModelRef) {
        self.model = model;
    }
}

/// Builds a request after validating only prompt text.
/// Callers must run `GenerationRequest::validate()` before submission.
pub(super) fn build_generation_request_with_prompt_validation(
    request_id: String,
    model: ModelRef,
    mode: GenerationMode,
    prompt: String,
    references: Vec<MidiReferenceSummary>,
) -> Result<GenerationRequest, LlmError> {
    validate_prompt_input(&prompt)?;

    Ok(GenerationRequest {
        request_id,
        model,
        mode,
        prompt,
        params: GenerationParams {
            bpm: DEFAULT_BPM,
            key: "C".to_string(),
            scale: "major".to_string(),
            density: DEFAULT_DENSITY,
            complexity: DEFAULT_COMPLEXITY,
            temperature: Some(DEFAULT_TEMPERATURE),
            top_p: Some(DEFAULT_TOP_P),
            max_tokens: Some(DEFAULT_MAX_TOKENS),
        },
        references,
        variation_count: DEFAULT_VARIATION_COUNT,
    })
}

pub(super) fn validate_prompt_input(prompt: &str) -> Result<(), LlmError> {
    if prompt.trim().is_empty() {
        return Err(LlmError::validation("prompt must not be empty"));
    }
    Ok(())
}
