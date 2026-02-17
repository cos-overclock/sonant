use sonant::domain::{
    GenerationMode, GenerationParams, GenerationRequest, LlmError, MidiReferenceSummary, ModelRef,
};

use super::{
    DEFAULT_BPM, DEFAULT_COMPLEXITY, DEFAULT_DENSITY, DEFAULT_MAX_TOKENS, DEFAULT_TEMPERATURE,
    DEFAULT_TOP_P, DEFAULT_VARIATION_COUNT, GPUI_HELPER_REQUEST_ID_PREFIX,
};

const PARAM_LEVEL_MIN: u8 = 1;
const PARAM_LEVEL_MAX: u8 = 5;
const BPM_MIN: u16 = 20;
const BPM_MAX: u16 = 300;
const DEFAULT_KEY: &str = "C";
const DEFAULT_SCALE: &str = "Major";

#[derive(Debug, Clone)]
pub(super) struct PromptSubmissionModel {
    next_request_number: u64,
    model: ModelRef,
    bpm: u16,
    key: String,
    scale: String,
    density: u8,
    complexity: u8,
}

impl PromptSubmissionModel {
    pub(super) fn new(model: ModelRef) -> Self {
        Self {
            next_request_number: 1,
            model,
            bpm: clamp_bpm(DEFAULT_BPM),
            key: DEFAULT_KEY.to_string(),
            scale: DEFAULT_SCALE.to_string(),
            density: clamp_param_level(DEFAULT_DENSITY),
            complexity: clamp_param_level(DEFAULT_COMPLEXITY),
        }
    }

    pub(super) fn prepare_request(
        &mut self,
        mode: GenerationMode,
        prompt: String,
        references: Vec<MidiReferenceSummary>,
    ) -> Result<GenerationRequest, LlmError> {
        let request_id = format!(
            "{GPUI_HELPER_REQUEST_ID_PREFIX}-{}",
            self.next_request_number
        );
        self.next_request_number = self.next_request_number.saturating_add(1);
        let mut request = build_generation_request_with_prompt_validation(
            request_id,
            self.model.clone(),
            mode,
            prompt,
            references,
        )?;
        request.params.bpm = self.bpm;
        request.params.key = self.key.clone();
        request.params.scale = self.scale.clone();
        request.params.density = self.density;
        request.params.complexity = self.complexity;
        Ok(request)
    }

    pub(super) fn set_model(&mut self, model: ModelRef) {
        self.model = model;
    }

    pub(super) fn set_bpm(&mut self, bpm: u16) {
        self.bpm = clamp_bpm(bpm);
    }

    pub(super) fn bpm(&self) -> u16 {
        self.bpm
    }

    pub(super) fn set_key(&mut self, key: &str) {
        if !key.trim().is_empty() {
            self.key = key.to_string();
        }
    }

    pub(super) fn key(&self) -> &str {
        self.key.as_str()
    }

    pub(super) fn set_scale(&mut self, scale: &str) {
        if !scale.trim().is_empty() {
            self.scale = scale.to_string();
        }
    }

    pub(super) fn scale(&self) -> &str {
        self.scale.as_str()
    }

    pub(super) fn set_density(&mut self, density: u8) {
        self.density = clamp_param_level(density);
    }

    pub(super) fn density(&self) -> u8 {
        self.density
    }

    pub(super) fn set_complexity(&mut self, complexity: u8) {
        self.complexity = clamp_param_level(complexity);
    }

    pub(super) fn complexity(&self) -> u8 {
        self.complexity
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
            key: DEFAULT_KEY.to_string(),
            scale: DEFAULT_SCALE.to_string(),
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

fn clamp_param_level(level: u8) -> u8 {
    level.clamp(PARAM_LEVEL_MIN, PARAM_LEVEL_MAX)
}

fn clamp_bpm(bpm: u16) -> u16 {
    bpm.clamp(BPM_MIN, BPM_MAX)
}
