use serde::{Deserialize, Serialize};

use super::LlmError;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelRef {
    pub provider: String,
    pub model: String,
}

impl ModelRef {
    pub fn validate(&self) -> Result<(), LlmError> {
        if self.provider.trim().is_empty() {
            return Err(LlmError::validation("model provider must not be empty"));
        }
        if self.model.trim().is_empty() {
            return Err(LlmError::validation("model name must not be empty"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GenerationMode {
    Melody,
    ChordProgression,
    DrumPattern,
    Bassline,
    CounterMelody,
    Harmony,
    Continuation,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GenerationParams {
    pub bpm: u16,
    pub key: String,
    pub scale: String,
    pub density: u8,
    pub complexity: u8,
    #[serde(default)]
    pub temperature: Option<f32>,
    #[serde(default)]
    pub top_p: Option<f32>,
    #[serde(default)]
    pub max_tokens: Option<u16>,
}

impl GenerationParams {
    pub fn validate(&self) -> Result<(), LlmError> {
        if !(20..=300).contains(&self.bpm) {
            return Err(LlmError::validation(format!(
                "bpm must be in 20..=300 (got {})",
                self.bpm
            )));
        }
        if self.key.trim().is_empty() {
            return Err(LlmError::validation("key must not be empty"));
        }
        if self.scale.trim().is_empty() {
            return Err(LlmError::validation("scale must not be empty"));
        }
        if !(1..=5).contains(&self.density) {
            return Err(LlmError::validation(format!(
                "density must be in 1..=5 (got {})",
                self.density
            )));
        }
        if !(1..=5).contains(&self.complexity) {
            return Err(LlmError::validation(format!(
                "complexity must be in 1..=5 (got {})",
                self.complexity
            )));
        }
        if let Some(temperature) = self.temperature {
            if !(0.0..=2.0).contains(&temperature) {
                return Err(LlmError::validation(format!(
                    "temperature must be in 0.0..=2.0 (got {temperature})"
                )));
            }
        }
        if let Some(top_p) = self.top_p {
            if !(0.0..=1.0).contains(&top_p) {
                return Err(LlmError::validation(format!(
                    "top_p must be in 0.0..=1.0 (got {top_p})"
                )));
            }
        }
        if let Some(max_tokens) = self.max_tokens {
            if max_tokens == 0 {
                return Err(LlmError::validation("max_tokens must be greater than 0"));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReferenceSource {
    File,
    Live,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MidiReferenceSummary {
    pub slot: String,
    pub source: ReferenceSource,
    pub bars: u16,
    pub note_count: u32,
    pub density_hint: f32,
    pub min_pitch: u8,
    pub max_pitch: u8,
}

impl MidiReferenceSummary {
    pub fn validate(&self) -> Result<(), LlmError> {
        if self.slot.trim().is_empty() {
            return Err(LlmError::validation("reference slot must not be empty"));
        }
        if self.bars == 0 {
            return Err(LlmError::validation(
                "reference bars must be greater than 0",
            ));
        }
        if self.note_count == 0 {
            return Err(LlmError::validation(
                "reference note_count must be greater than 0",
            ));
        }
        if !(0.0..=1.0).contains(&self.density_hint) {
            return Err(LlmError::validation(format!(
                "reference density_hint must be in 0.0..=1.0 (got {})",
                self.density_hint
            )));
        }
        if self.min_pitch > 127 || self.max_pitch > 127 {
            return Err(LlmError::validation(
                "reference min_pitch/max_pitch must be in 0..=127",
            ));
        }
        if self.min_pitch > self.max_pitch {
            return Err(LlmError::validation(
                "reference min_pitch must be less than or equal to max_pitch",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GenerationRequest {
    pub request_id: String,
    pub model: ModelRef,
    pub mode: GenerationMode,
    pub prompt: String,
    pub params: GenerationParams,
    #[serde(default)]
    pub references: Vec<MidiReferenceSummary>,
    #[serde(default = "default_variation_count")]
    pub variation_count: u8,
}

impl GenerationRequest {
    pub fn validate(&self) -> Result<(), LlmError> {
        if self.request_id.trim().is_empty() {
            return Err(LlmError::validation("request_id must not be empty"));
        }
        self.model.validate()?;
        if self.prompt.trim().is_empty() {
            return Err(LlmError::validation("prompt must not be empty"));
        }
        self.params.validate()?;
        if self.variation_count == 0 {
            return Err(LlmError::validation(
                "variation_count must be greater than 0",
            ));
        }
        if matches!(self.mode, GenerationMode::Continuation) && self.references.is_empty() {
            return Err(LlmError::validation(
                "continuation mode requires at least one MIDI reference",
            ));
        }
        for reference in &self.references {
            reference.validate()?;
        }
        Ok(())
    }
}

fn default_variation_count() -> u8 {
    1
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GeneratedNote {
    pub pitch: u8,
    pub start_tick: u32,
    pub duration_tick: u32,
    pub velocity: u8,
    #[serde(default = "default_channel")]
    pub channel: u8,
}

impl GeneratedNote {
    pub fn validate(&self) -> Result<(), LlmError> {
        if self.pitch > 127 {
            return Err(LlmError::validation("note pitch must be in 0..=127"));
        }
        if self.duration_tick == 0 {
            return Err(LlmError::validation(
                "note duration_tick must be greater than 0",
            ));
        }
        if self.velocity > 127 {
            return Err(LlmError::validation("note velocity must be in 0..=127"));
        }
        if !(1..=16).contains(&self.channel) {
            return Err(LlmError::validation("note channel must be in 1..=16"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GenerationCandidate {
    pub id: String,
    pub bars: u16,
    pub notes: Vec<GeneratedNote>,
    #[serde(default)]
    pub score_hint: Option<f32>,
}

impl GenerationCandidate {
    pub fn validate(&self) -> Result<(), LlmError> {
        if self.id.trim().is_empty() {
            return Err(LlmError::validation("candidate id must not be empty"));
        }
        if self.bars == 0 {
            return Err(LlmError::validation(
                "candidate bars must be greater than 0",
            ));
        }
        if self.notes.is_empty() {
            return Err(LlmError::validation("candidate notes must not be empty"));
        }
        if let Some(score_hint) = self.score_hint {
            if !(0.0..=1.0).contains(&score_hint) {
                return Err(LlmError::validation(format!(
                    "score_hint must be in 0.0..=1.0 (got {score_hint})"
                )));
            }
        }
        for note in &self.notes {
            note.validate()?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GenerationResult {
    pub request_id: String,
    pub model: ModelRef,
    pub candidates: Vec<GenerationCandidate>,
}

impl GenerationResult {
    pub fn validate(&self) -> Result<(), LlmError> {
        if self.request_id.trim().is_empty() {
            return Err(LlmError::validation("request_id must not be empty"));
        }
        self.model.validate()?;
        if self.candidates.is_empty() {
            return Err(LlmError::validation("at least one candidate is required"));
        }
        for candidate in &self.candidates {
            candidate.validate()?;
        }
        Ok(())
    }
}

fn default_channel() -> u8 {
    1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_validation_rejects_empty_prompt() {
        let request = GenerationRequest {
            request_id: "req-1".to_string(),
            model: ModelRef {
                provider: "anthropic".to_string(),
                model: "claude-3-5-sonnet".to_string(),
            },
            mode: GenerationMode::Melody,
            prompt: "   ".to_string(),
            params: GenerationParams {
                bpm: 120,
                key: "C".to_string(),
                scale: "major".to_string(),
                density: 3,
                complexity: 3,
                temperature: Some(0.7),
                top_p: Some(0.9),
                max_tokens: Some(2048),
            },
            references: Vec::new(),
            variation_count: 1,
        };

        assert!(matches!(
            request.validate(),
            Err(LlmError::Validation { message }) if message == "prompt must not be empty"
        ));
    }

    #[test]
    fn request_validation_requires_reference_in_continuation_mode() {
        let request = GenerationRequest {
            request_id: "req-1".to_string(),
            model: ModelRef {
                provider: "anthropic".to_string(),
                model: "claude-3-5-sonnet".to_string(),
            },
            mode: GenerationMode::Continuation,
            prompt: "continue this phrase".to_string(),
            params: GenerationParams {
                bpm: 120,
                key: "C".to_string(),
                scale: "major".to_string(),
                density: 3,
                complexity: 3,
                temperature: Some(0.7),
                top_p: Some(0.9),
                max_tokens: Some(2048),
            },
            references: Vec::new(),
            variation_count: 1,
        };

        assert!(matches!(
            request.validate(),
            Err(LlmError::Validation { message })
            if message == "continuation mode requires at least one MIDI reference"
        ));
    }
}
