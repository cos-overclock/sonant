use gpui::rgb;
use sonant::app::LoadMidiError;
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
    pub(super) message: String,
    pub(super) retry_path: Option<String>,
}

impl MidiSlotErrorState {
    pub(super) fn non_retryable(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            retry_path: None,
        }
    }

    pub(super) fn from_load_error(path: &str, error: &LoadMidiError) -> Self {
        let retry_path = can_retry_midi_load_error(error).then(|| path.to_string());
        Self {
            message: error.user_message(),
            retry_path,
        }
    }

    pub(super) fn can_retry(&self) -> bool {
        self.retry_path.is_some()
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
