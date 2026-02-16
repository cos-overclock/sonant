use gpui::rgb;
use sonant::app::LoadMidiError;
use sonant::domain::{GenerationMode, MidiReferenceSummary, ReferenceSlot};
use sonant::infra::midi::MidiLoadError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum HelperGenerationStatus {
    Idle,
    Submitting {
        request_id: String,
    },
    Running {
        request_id: String,
    },
    Succeeded {
        request_id: String,
        candidate_count: usize,
    },
    Failed {
        message: String,
    },
    Cancelled {
        request_id: String,
    },
}

impl HelperGenerationStatus {
    pub(super) fn label(&self) -> String {
        match self {
            Self::Idle => "Idle".to_string(),
            Self::Submitting { request_id } => format!("Submitting {request_id}..."),
            Self::Running { request_id } => format!("Running {request_id}..."),
            Self::Succeeded {
                request_id,
                candidate_count,
            } => {
                format!("Succeeded {request_id} ({candidate_count} candidate(s))")
            }
            Self::Failed { message } => format!("Failed: {message}"),
            Self::Cancelled { request_id } => format!("Cancelled {request_id}"),
        }
    }

    pub(super) fn color(&self) -> gpui::Hsla {
        match self {
            Self::Idle => rgb(0x93c5fd).into(),
            Self::Submitting { .. } | Self::Running { .. } => rgb(0xfbbf24).into(),
            Self::Succeeded { .. } => rgb(0x86efac).into(),
            Self::Failed { .. } => rgb(0xfca5a5).into(),
            Self::Cancelled { .. } => rgb(0xfcd34d).into(),
        }
    }

    pub(super) fn is_submitting_or_running(&self) -> bool {
        matches!(self, Self::Submitting { .. } | Self::Running { .. })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct MidiSlotErrorState {
    pub(super) slot: ReferenceSlot,
    pub(super) message: String,
    pub(super) retry_path: Option<String>,
}

impl MidiSlotErrorState {
    pub(super) fn non_retryable(slot: ReferenceSlot, message: impl Into<String>) -> Self {
        Self {
            slot,
            message: message.into(),
            retry_path: None,
        }
    }

    pub(super) fn from_load_error(slot: ReferenceSlot, path: &str, error: &LoadMidiError) -> Self {
        let retry_path = can_retry_midi_load_error(error).then(|| path.to_string());
        Self {
            slot,
            message: error.user_message(),
            retry_path,
        }
    }

    pub(super) fn can_retry(&self) -> bool {
        self.retry_path.is_some()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ModeReferenceRequirement {
    pub(super) description: &'static str,
    pub(super) unmet_message: Option<&'static str>,
}

pub(super) fn mode_reference_requirement(mode: GenerationMode) -> ModeReferenceRequirement {
    match mode {
        GenerationMode::Melody
        | GenerationMode::ChordProgression
        | GenerationMode::DrumPattern
        | GenerationMode::Bassline => ModeReferenceRequirement {
            description: "Reference MIDI: Optional.",
            unmet_message: None,
        },
        GenerationMode::CounterMelody => ModeReferenceRequirement {
            description: "Reference MIDI required: Melody.",
            unmet_message: Some(
                "Counter Melody mode requires a Melody reference MIDI before generating.",
            ),
        },
        GenerationMode::Harmony => ModeReferenceRequirement {
            description: "Reference MIDI required: Melody.",
            unmet_message: Some("Harmony mode requires a Melody reference MIDI before generating."),
        },
        GenerationMode::Continuation => ModeReferenceRequirement {
            description: "Reference MIDI required: At least one slot.",
            unmet_message: Some(
                "Continuation mode requires at least one reference MIDI before generating.",
            ),
        },
    }
}

pub(super) fn mode_reference_requirement_satisfied(
    mode: GenerationMode,
    references: &[MidiReferenceSummary],
) -> bool {
    match mode {
        GenerationMode::Melody
        | GenerationMode::ChordProgression
        | GenerationMode::DrumPattern
        | GenerationMode::Bassline => true,
        GenerationMode::CounterMelody | GenerationMode::Harmony => references
            .iter()
            .any(|reference| reference.slot == ReferenceSlot::Melody),
        GenerationMode::Continuation => !references.is_empty(),
    }
}

pub(super) fn can_retry_midi_load_error(error: &LoadMidiError) -> bool {
    matches!(
        error,
        LoadMidiError::LoadFailed {
            source: MidiLoadError::Io { .. } | MidiLoadError::Parse { .. }
        }
    )
}
