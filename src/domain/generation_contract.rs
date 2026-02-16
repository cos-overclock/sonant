use serde::{Deserialize, Serialize};

use super::{LlmError, has_supported_midi_extension};

const DENSITY_NOTES_PER_BAR_AT_MAX_HINT: f32 = 32.0;

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
        if let Some(temperature) = self.temperature
            && !(0.0..=2.0).contains(&temperature)
        {
            return Err(LlmError::validation(format!(
                "temperature must be in 0.0..=2.0 (got {temperature})"
            )));
        }
        if let Some(top_p) = self.top_p
            && !(0.0..=1.0).contains(&top_p)
        {
            return Err(LlmError::validation(format!(
                "top_p must be in 0.0..=1.0 (got {top_p})"
            )));
        }
        if let Some(max_tokens) = self.max_tokens
            && max_tokens == 0
        {
            return Err(LlmError::validation("max_tokens must be greater than 0"));
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReferenceSlot {
    Melody,
    ChordProgression,
    DrumPattern,
    Bassline,
    CounterMelody,
    Harmony,
    ContinuationSeed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileReferenceInput {
    pub path: String,
}

impl FileReferenceInput {
    pub fn validate(&self) -> Result<(), LlmError> {
        let path = self.path.trim();
        if path.is_empty() {
            return Err(LlmError::validation(
                "reference file path must not be empty",
            ));
        }
        if !has_supported_midi_extension(path) {
            return Err(LlmError::validation(format!(
                "reference file path must end with .mid or .midi (got {path})"
            )));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MidiReferenceEvent {
    pub track: u16,
    pub absolute_tick: u32,
    pub delta_tick: u32,
    pub event: String,
}

impl MidiReferenceEvent {
    pub fn validate(&self) -> Result<(), LlmError> {
        if self.event.trim().is_empty() {
            return Err(LlmError::validation(
                "reference event must include a non-empty event payload",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MidiReferenceSummary {
    pub slot: ReferenceSlot,
    pub source: ReferenceSource,
    #[serde(default)]
    pub file: Option<FileReferenceInput>,
    pub bars: u16,
    pub note_count: u32,
    pub density_hint: f32,
    pub min_pitch: u8,
    pub max_pitch: u8,
    #[serde(default)]
    pub events: Vec<MidiReferenceEvent>,
}

impl MidiReferenceSummary {
    pub fn validate(&self) -> Result<(), LlmError> {
        match self.source {
            ReferenceSource::File => {
                let file = self.file.as_ref().ok_or_else(|| {
                    LlmError::validation("reference file source requires file metadata")
                })?;
                file.validate()?;
            }
            ReferenceSource::Live => {
                if self.file.is_some() {
                    return Err(LlmError::validation(
                        "reference file metadata must be empty for live source",
                    ));
                }
            }
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
        if matches!(self.source, ReferenceSource::File) && self.events.is_empty() {
            return Err(LlmError::validation(
                "reference events must not be empty for file source",
            ));
        }
        for event in &self.events {
            event.validate()?;
        }
        Ok(())
    }
}

pub fn calculate_reference_density_hint(note_count: u32, bars: u16) -> f32 {
    if bars == 0 {
        return 1.0;
    }
    let notes_per_bar = note_count as f32 / f32::from(bars);
    (notes_per_bar / DENSITY_NOTES_PER_BAR_AT_MAX_HINT).clamp(0.0, 1.0)
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
        for reference in &self.references {
            reference.validate()?;
        }
        self.validate_mode_reference_requirements()?;
        Ok(())
    }

    fn validate_mode_reference_requirements(&self) -> Result<(), LlmError> {
        match self.mode {
            GenerationMode::Melody
            | GenerationMode::ChordProgression
            | GenerationMode::DrumPattern
            | GenerationMode::Bassline => Ok(()),
            GenerationMode::CounterMelody => {
                if self.has_reference_slot(ReferenceSlot::Melody) {
                    Ok(())
                } else {
                    Err(LlmError::validation(
                        "counter melody mode requires at least one melody MIDI reference",
                    ))
                }
            }
            GenerationMode::Harmony => {
                if self.has_reference_slot(ReferenceSlot::Melody) {
                    Ok(())
                } else {
                    Err(LlmError::validation(
                        "harmony mode requires at least one melody MIDI reference",
                    ))
                }
            }
            GenerationMode::Continuation => {
                if self.references.is_empty() {
                    Err(LlmError::validation(
                        "continuation mode requires at least one MIDI reference",
                    ))
                } else {
                    Ok(())
                }
            }
        }
    }

    fn has_reference_slot(&self, slot: ReferenceSlot) -> bool {
        self.references
            .iter()
            .any(|reference| reference.slot == slot)
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
        if let Some(score_hint) = self.score_hint
            && !(0.0..=1.0).contains(&score_hint)
        {
            return Err(LlmError::validation(format!(
                "score_hint must be in 0.0..=1.0 (got {score_hint})"
            )));
        }
        for note in &self.notes {
            note.validate()?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct GenerationUsage {
    #[serde(default)]
    pub input_tokens: Option<u32>,
    #[serde(default)]
    pub output_tokens: Option<u32>,
    #[serde(default)]
    pub total_tokens: Option<u32>,
    #[serde(default)]
    pub cache_creation_input_tokens: Option<u32>,
    #[serde(default)]
    pub cache_read_input_tokens: Option<u32>,
}

impl GenerationUsage {
    pub fn validate(&self) -> Result<(), LlmError> {
        let has_usage = self.input_tokens.is_some()
            || self.output_tokens.is_some()
            || self.total_tokens.is_some()
            || self.cache_creation_input_tokens.is_some()
            || self.cache_read_input_tokens.is_some();
        if !has_usage {
            return Err(LlmError::validation(
                "usage must include at least one token counter",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct GenerationMetadata {
    #[serde(default)]
    pub latency_ms: Option<u64>,
    #[serde(default)]
    pub provider_request_id: Option<String>,
    #[serde(default)]
    pub stop_reason: Option<String>,
    #[serde(default)]
    pub usage: Option<GenerationUsage>,
}

impl GenerationMetadata {
    pub fn validate(&self) -> Result<(), LlmError> {
        if let Some(provider_request_id) = &self.provider_request_id
            && provider_request_id.trim().is_empty()
        {
            return Err(LlmError::validation(
                "metadata.provider_request_id must not be empty when provided",
            ));
        }
        if let Some(stop_reason) = &self.stop_reason
            && stop_reason.trim().is_empty()
        {
            return Err(LlmError::validation(
                "metadata.stop_reason must not be empty when provided",
            ));
        }
        if let Some(usage) = &self.usage {
            usage.validate()?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GenerationResult {
    pub request_id: String,
    pub model: ModelRef,
    pub candidates: Vec<GenerationCandidate>,
    #[serde(default)]
    pub metadata: GenerationMetadata,
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
        self.metadata.validate()?;
        Ok(())
    }
}

fn default_channel() -> u8 {
    1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_event() -> MidiReferenceEvent {
        MidiReferenceEvent {
            track: 0,
            absolute_tick: 0,
            delta_tick: 0,
            event: "NoteOn channel=0 key=60 vel=100".to_string(),
        }
    }

    fn sample_reference(slot: ReferenceSlot) -> MidiReferenceSummary {
        MidiReferenceSummary {
            slot,
            source: ReferenceSource::File,
            file: Some(FileReferenceInput {
                path: "reference.mid".to_string(),
            }),
            bars: 4,
            note_count: 24,
            density_hint: 0.5,
            min_pitch: 60,
            max_pitch: 72,
            events: vec![sample_event()],
        }
    }

    fn sample_live_reference(slot: ReferenceSlot) -> MidiReferenceSummary {
        MidiReferenceSummary {
            slot,
            source: ReferenceSource::Live,
            file: None,
            bars: 2,
            note_count: 12,
            density_hint: 0.375,
            min_pitch: 55,
            max_pitch: 76,
            events: vec![MidiReferenceEvent {
                track: 1,
                absolute_tick: 120,
                delta_tick: 120,
                event: "LiveMidi channel=2 status=0x91 data1=55 data2=100 port=1 time=120"
                    .to_string(),
            }],
        }
    }

    fn valid_request(
        mode: GenerationMode,
        references: Vec<MidiReferenceSummary>,
    ) -> GenerationRequest {
        GenerationRequest {
            request_id: "req-1".to_string(),
            model: ModelRef {
                provider: "anthropic".to_string(),
                model: "claude-3-5-sonnet".to_string(),
            },
            mode,
            prompt: "generate MIDI".to_string(),
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
            references,
            variation_count: 1,
        }
    }

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
    fn calculate_reference_density_hint_uses_shared_normalization_rule() {
        assert_eq!(calculate_reference_density_hint(16, 4), 0.125);
        assert_eq!(calculate_reference_density_hint(0, 4), 0.0);
        assert_eq!(calculate_reference_density_hint(64, 1), 1.0);
        assert_eq!(calculate_reference_density_hint(4, 0), 1.0);
    }

    #[test]
    fn request_validation_mode_reference_requirements_cover_pass_and_fail_matrix() {
        let cases = [
            (GenerationMode::Melody, Vec::new(), None),
            (GenerationMode::ChordProgression, Vec::new(), None),
            (GenerationMode::DrumPattern, Vec::new(), None),
            (GenerationMode::Bassline, Vec::new(), None),
            (
                GenerationMode::Bassline,
                vec![sample_reference(ReferenceSlot::Melody)],
                None,
            ),
            (
                GenerationMode::Bassline,
                vec![sample_reference(ReferenceSlot::ChordProgression)],
                None,
            ),
            (
                GenerationMode::CounterMelody,
                Vec::new(),
                Some("counter melody mode requires at least one melody MIDI reference"),
            ),
            (
                GenerationMode::CounterMelody,
                vec![sample_reference(ReferenceSlot::ChordProgression)],
                Some("counter melody mode requires at least one melody MIDI reference"),
            ),
            (
                GenerationMode::CounterMelody,
                vec![sample_reference(ReferenceSlot::Melody)],
                None,
            ),
            (
                GenerationMode::CounterMelody,
                vec![sample_live_reference(ReferenceSlot::Melody)],
                None,
            ),
            (
                GenerationMode::Harmony,
                Vec::new(),
                Some("harmony mode requires at least one melody MIDI reference"),
            ),
            (
                GenerationMode::Harmony,
                vec![sample_reference(ReferenceSlot::DrumPattern)],
                Some("harmony mode requires at least one melody MIDI reference"),
            ),
            (
                GenerationMode::Harmony,
                vec![sample_reference(ReferenceSlot::Melody)],
                None,
            ),
            (
                GenerationMode::Harmony,
                vec![sample_live_reference(ReferenceSlot::Melody)],
                None,
            ),
            (
                GenerationMode::Continuation,
                Vec::new(),
                Some("continuation mode requires at least one MIDI reference"),
            ),
            (
                GenerationMode::Continuation,
                vec![sample_reference(ReferenceSlot::Melody)],
                None,
            ),
            (
                GenerationMode::Continuation,
                vec![sample_live_reference(ReferenceSlot::ChordProgression)],
                None,
            ),
            (
                GenerationMode::CounterMelody,
                vec![
                    sample_reference(ReferenceSlot::ChordProgression),
                    sample_live_reference(ReferenceSlot::Melody),
                ],
                None,
            ),
            (
                GenerationMode::Harmony,
                vec![
                    sample_reference(ReferenceSlot::DrumPattern),
                    sample_live_reference(ReferenceSlot::Melody),
                ],
                None,
            ),
        ];

        for (mode, references, expected_error) in cases {
            let request = valid_request(mode, references);
            match expected_error {
                Some(message) => assert!(
                    matches!(
                        request.validate(),
                        Err(LlmError::Validation {
                            message: actual_message
                        }) if actual_message == message
                    ),
                    "mode {mode:?} should fail with '{message}'",
                ),
                None => assert!(
                    request.validate().is_ok(),
                    "mode {mode:?} should pass reference validation"
                ),
            }
        }
    }

    #[test]
    fn reference_validation_requires_file_metadata_for_file_source() {
        let reference = MidiReferenceSummary {
            slot: ReferenceSlot::Melody,
            source: ReferenceSource::File,
            file: None,
            bars: 4,
            note_count: 24,
            density_hint: 0.5,
            min_pitch: 60,
            max_pitch: 72,
            events: vec![sample_event()],
        };

        assert!(matches!(
            reference.validate(),
            Err(LlmError::Validation { message })
            if message == "reference file source requires file metadata"
        ));
    }

    #[test]
    fn reference_validation_rejects_non_midi_file_extension() {
        let reference = MidiReferenceSummary {
            slot: ReferenceSlot::Melody,
            source: ReferenceSource::File,
            file: Some(FileReferenceInput {
                path: "melody_reference.wav".to_string(),
            }),
            bars: 4,
            note_count: 24,
            density_hint: 0.5,
            min_pitch: 60,
            max_pitch: 72,
            events: vec![sample_event()],
        };

        assert!(matches!(
            reference.validate(),
            Err(LlmError::Validation { message })
            if message == "reference file path must end with .mid or .midi (got melody_reference.wav)"
        ));
    }

    #[test]
    fn reference_validation_rejects_file_metadata_for_live_source() {
        let reference = MidiReferenceSummary {
            slot: ReferenceSlot::Melody,
            source: ReferenceSource::Live,
            file: Some(FileReferenceInput {
                path: "melody_reference.mid".to_string(),
            }),
            bars: 4,
            note_count: 24,
            density_hint: 0.5,
            min_pitch: 60,
            max_pitch: 72,
            events: vec![sample_event()],
        };

        assert!(matches!(
            reference.validate(),
            Err(LlmError::Validation { message })
            if message == "reference file metadata must be empty for live source"
        ));
    }

    #[test]
    fn reference_validation_rejects_empty_file_path() {
        let reference = MidiReferenceSummary {
            slot: ReferenceSlot::Melody,
            source: ReferenceSource::File,
            file: Some(FileReferenceInput {
                path: "   ".to_string(),
            }),
            bars: 4,
            note_count: 24,
            density_hint: 0.5,
            min_pitch: 60,
            max_pitch: 72,
            events: vec![sample_event()],
        };

        assert!(matches!(
            reference.validate(),
            Err(LlmError::Validation { .. })
        ));
    }

    #[test]
    fn reference_validation_rejects_file_path_without_extension() {
        let reference = MidiReferenceSummary {
            slot: ReferenceSlot::Melody,
            source: ReferenceSource::File,
            file: Some(FileReferenceInput {
                path: "melody_reference".to_string(),
            }),
            bars: 4,
            note_count: 24,
            density_hint: 0.5,
            min_pitch: 60,
            max_pitch: 72,
            events: vec![sample_event()],
        };

        assert!(matches!(
            reference.validate(),
            Err(LlmError::Validation { .. })
        ));
    }

    #[test]
    fn reference_validation_accepts_lowercase_mid_extension() {
        let reference = MidiReferenceSummary {
            slot: ReferenceSlot::Melody,
            source: ReferenceSource::File,
            file: Some(FileReferenceInput {
                path: "melody_reference.mid".to_string(),
            }),
            bars: 4,
            note_count: 24,
            density_hint: 0.5,
            min_pitch: 60,
            max_pitch: 72,
            events: vec![sample_event()],
        };

        assert!(reference.validate().is_ok());
    }

    #[test]
    fn reference_validation_accepts_lowercase_midi_extension() {
        let reference = MidiReferenceSummary {
            slot: ReferenceSlot::Melody,
            source: ReferenceSource::File,
            file: Some(FileReferenceInput {
                path: "melody_reference.midi".to_string(),
            }),
            bars: 4,
            note_count: 24,
            density_hint: 0.5,
            min_pitch: 60,
            max_pitch: 72,
            events: vec![sample_event()],
        };

        assert!(reference.validate().is_ok());
    }

    #[test]
    fn reference_validation_accepts_uppercase_mid_extension() {
        let reference = MidiReferenceSummary {
            slot: ReferenceSlot::Melody,
            source: ReferenceSource::File,
            file: Some(FileReferenceInput {
                path: "melody_reference.MID".to_string(),
            }),
            bars: 4,
            note_count: 24,
            density_hint: 0.5,
            min_pitch: 60,
            max_pitch: 72,
            events: vec![sample_event()],
        };

        assert!(reference.validate().is_ok());
    }

    #[test]
    fn reference_validation_accepts_mixed_case_midi_extension() {
        let reference = MidiReferenceSummary {
            slot: ReferenceSlot::Melody,
            source: ReferenceSource::File,
            file: Some(FileReferenceInput {
                path: "melody_reference.Midi".to_string(),
            }),
            bars: 4,
            note_count: 24,
            density_hint: 0.5,
            min_pitch: 60,
            max_pitch: 72,
            events: vec![sample_event()],
        };

        assert!(reference.validate().is_ok());
    }

    #[test]
    fn reference_validation_rejects_empty_event_payload() {
        let reference = MidiReferenceSummary {
            slot: ReferenceSlot::Melody,
            source: ReferenceSource::File,
            file: Some(FileReferenceInput {
                path: "melody_reference.mid".to_string(),
            }),
            bars: 4,
            note_count: 24,
            density_hint: 0.5,
            min_pitch: 60,
            max_pitch: 72,
            events: vec![MidiReferenceEvent {
                track: 0,
                absolute_tick: 0,
                delta_tick: 0,
                event: "   ".to_string(),
            }],
        };

        assert!(matches!(
            reference.validate(),
            Err(LlmError::Validation { message })
            if message == "reference event must include a non-empty event payload"
        ));
    }

    #[test]
    fn reference_validation_rejects_missing_events_for_file_source() {
        let reference = MidiReferenceSummary {
            slot: ReferenceSlot::Melody,
            source: ReferenceSource::File,
            file: Some(FileReferenceInput {
                path: "melody_reference.mid".to_string(),
            }),
            bars: 4,
            note_count: 24,
            density_hint: 0.5,
            min_pitch: 60,
            max_pitch: 72,
            events: Vec::new(),
        };

        assert!(matches!(
            reference.validate(),
            Err(LlmError::Validation { message })
            if message == "reference events must not be empty for file source"
        ));
    }

    #[test]
    fn result_validation_rejects_empty_provider_request_id_metadata() {
        let result = GenerationResult {
            request_id: "req-1".to_string(),
            model: ModelRef {
                provider: "anthropic".to_string(),
                model: "claude-3-5-sonnet".to_string(),
            },
            candidates: vec![GenerationCandidate {
                id: "cand-1".to_string(),
                bars: 4,
                notes: vec![GeneratedNote {
                    pitch: 60,
                    start_tick: 0,
                    duration_tick: 120,
                    velocity: 100,
                    channel: 1,
                }],
                score_hint: Some(0.8),
            }],
            metadata: GenerationMetadata {
                provider_request_id: Some("  ".to_string()),
                ..GenerationMetadata::default()
            },
        };

        assert!(matches!(
            result.validate(),
            Err(LlmError::Validation { message })
            if message == "metadata.provider_request_id must not be empty when provided"
        ));
    }
}
