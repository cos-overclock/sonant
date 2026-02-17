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
pub(super) enum ProviderStatus {
    Connected,
    InvalidKey,
    NotConfigured,
}

impl ProviderStatus {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Connected => "API CONNECTED",
            Self::InvalidKey => "API INVALID KEY",
            Self::NotConfigured => "API NOT CONFIGURED",
        }
    }

    pub(super) fn color(self) -> gpui::Hsla {
        match self {
            Self::Connected => rgb(0x86efac).into(),
            Self::InvalidKey => rgb(0xfca5a5).into(),
            Self::NotConfigured => rgb(0xfcd34d).into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SettingsTab {
    ApiKeys,
    MidiSettings,
    General,
}

impl SettingsTab {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::ApiKeys => "API Keys",
            Self::MidiSettings => "MIDI Settings",
            Self::General => "General",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum UiScreen {
    Main,
    Settings,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SettingsField {
    AnthropicApiKey,
    OpenAiApiKey,
    CustomBaseUrl,
    DefaultModel,
    ContextWindow,
}

impl SettingsField {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::AnthropicApiKey => "Anthropic API Key",
            Self::OpenAiApiKey => "OpenAI API Key",
            Self::CustomBaseUrl => "Custom Base URL",
            Self::DefaultModel => "Default Model",
            Self::ContextWindow => "Context Window",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SettingsDraftState {
    pub(super) anthropic_api_key: String,
    pub(super) openai_api_key: String,
    pub(super) custom_base_url: String,
    pub(super) default_model: String,
    pub(super) context_window: String,
}

impl SettingsDraftState {
    pub(super) fn with_default_model(default_model: impl Into<String>) -> Self {
        Self {
            default_model: default_model.into(),
            ..Self::default()
        }
    }
}

impl Default for SettingsDraftState {
    fn default() -> Self {
        Self {
            anthropic_api_key: String::new(),
            openai_api_key: String::new(),
            custom_base_url: String::new(),
            default_model: "claude-3-5-sonnet".to_string(),
            context_window: "8192".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SettingsUiState {
    pub(super) provider_status: ProviderStatus,
    pub(super) settings_tab: SettingsTab,
    pub(super) settings_dirty: bool,
    pub(super) screen: UiScreen,
    saved: SettingsDraftState,
    draft: SettingsDraftState,
}

impl SettingsUiState {
    pub(super) fn new(saved: SettingsDraftState) -> Self {
        let provider_status = provider_status_from_draft(&saved);
        Self {
            provider_status,
            settings_tab: SettingsTab::ApiKeys,
            settings_dirty: false,
            screen: UiScreen::Main,
            saved: saved.clone(),
            draft: saved,
        }
    }

    pub(super) fn open_settings(&mut self) {
        self.screen = UiScreen::Settings;
    }

    pub(super) fn close_settings(&mut self) {
        self.screen = UiScreen::Main;
    }

    pub(super) fn is_settings_open(&self) -> bool {
        self.screen == UiScreen::Settings
    }

    pub(super) fn select_settings_tab(&mut self, tab: SettingsTab) {
        self.settings_tab = tab;
    }

    pub(super) fn saved(&self) -> &SettingsDraftState {
        &self.saved
    }

    pub(super) fn draft(&self) -> &SettingsDraftState {
        &self.draft
    }

    pub(super) fn update_draft(&mut self, draft: SettingsDraftState) {
        self.draft = draft;
        self.settings_dirty = self.saved != self.draft;
    }

    pub(super) fn update_draft_field(
        &mut self,
        field: SettingsField,
        value: impl Into<String>,
    ) -> bool {
        let value = value.into();
        let target = match field {
            SettingsField::AnthropicApiKey => &mut self.draft.anthropic_api_key,
            SettingsField::OpenAiApiKey => &mut self.draft.openai_api_key,
            SettingsField::CustomBaseUrl => &mut self.draft.custom_base_url,
            SettingsField::DefaultModel => &mut self.draft.default_model,
            SettingsField::ContextWindow => &mut self.draft.context_window,
        };

        if *target == value {
            return false;
        }

        *target = value;
        self.settings_dirty = self.saved != self.draft;
        true
    }

    pub(super) fn draft_provider_status(&self) -> ProviderStatus {
        provider_status_from_draft(&self.draft)
    }

    pub(super) fn dirty_fields(&self) -> Vec<SettingsField> {
        const FIELDS: [SettingsField; 5] = [
            SettingsField::AnthropicApiKey,
            SettingsField::OpenAiApiKey,
            SettingsField::CustomBaseUrl,
            SettingsField::DefaultModel,
            SettingsField::ContextWindow,
        ];
        FIELDS
            .into_iter()
            .filter(|field| self.is_field_dirty(*field))
            .collect()
    }

    pub(super) fn is_field_dirty(&self, field: SettingsField) -> bool {
        match field {
            SettingsField::AnthropicApiKey => {
                self.saved.anthropic_api_key != self.draft.anthropic_api_key
            }
            SettingsField::OpenAiApiKey => self.saved.openai_api_key != self.draft.openai_api_key,
            SettingsField::CustomBaseUrl => {
                self.saved.custom_base_url != self.draft.custom_base_url
            }
            SettingsField::DefaultModel => self.saved.default_model != self.draft.default_model,
            SettingsField::ContextWindow => self.saved.context_window != self.draft.context_window,
        }
    }

    pub(super) fn save_and_close(&mut self) -> bool {
        let changed = self.settings_dirty;
        self.saved = self.draft.clone();
        self.settings_dirty = false;
        self.provider_status = provider_status_from_draft(&self.saved);
        self.close_settings();
        changed
    }

    pub(super) fn discard_and_close(&mut self) -> bool {
        let had_changes = self.settings_dirty;
        self.draft = self.saved.clone();
        self.settings_dirty = false;
        self.close_settings();
        had_changes
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

fn provider_status_from_draft(draft: &SettingsDraftState) -> ProviderStatus {
    let anthropic_key = draft.anthropic_api_key.trim();
    let openai_key = draft.openai_api_key.trim();
    let configured_keys = [anthropic_key, openai_key]
        .into_iter()
        .filter(|key| !key.is_empty())
        .collect::<Vec<_>>();

    if configured_keys.is_empty() {
        return ProviderStatus::NotConfigured;
    }

    if configured_keys
        .iter()
        .any(|key| key.chars().any(char::is_whitespace))
    {
        return ProviderStatus::InvalidKey;
    }

    ProviderStatus::Connected
}

#[cfg(test)]
mod tests {
    use super::{
        ProviderStatus, SettingsDraftState, SettingsField, SettingsTab, SettingsUiState, UiScreen,
    };

    #[test]
    fn open_and_close_settings_updates_screen_state() {
        let mut state = SettingsUiState::new(SettingsDraftState::default());

        assert_eq!(state.screen, UiScreen::Main);

        state.open_settings();
        assert!(state.is_settings_open());
        assert_eq!(state.screen, UiScreen::Settings);

        state.close_settings();
        assert!(!state.is_settings_open());
        assert_eq!(state.screen, UiScreen::Main);
    }

    #[test]
    fn select_settings_tab_switches_sidebar_tab_state() {
        let mut state = SettingsUiState::new(SettingsDraftState::default());
        assert_eq!(state.settings_tab, SettingsTab::ApiKeys);

        state.select_settings_tab(SettingsTab::MidiSettings);
        assert_eq!(state.settings_tab, SettingsTab::MidiSettings);

        state.select_settings_tab(SettingsTab::General);
        assert_eq!(state.settings_tab, SettingsTab::General);
    }

    #[test]
    fn draft_update_marks_dirty_and_tracks_changed_fields() {
        let mut state = SettingsUiState::new(SettingsDraftState::default());
        assert!(!state.settings_dirty);

        let mut draft = state.draft().clone();
        draft.default_model = "gpt-5.2".to_string();
        draft.context_window = "32768".to_string();
        state.update_draft(draft);

        assert!(state.settings_dirty);
        assert!(state.is_field_dirty(SettingsField::DefaultModel));
        assert!(state.is_field_dirty(SettingsField::ContextWindow));
        assert!(!state.is_field_dirty(SettingsField::AnthropicApiKey));
        assert_eq!(state.dirty_fields().len(), 2);
    }

    #[test]
    fn update_draft_field_updates_only_target_field() {
        let mut state = SettingsUiState::new(SettingsDraftState::default());
        assert!(!state.settings_dirty);

        let changed = state.update_draft_field(SettingsField::ContextWindow, "16384".to_string());
        assert!(changed);
        assert!(state.settings_dirty);
        assert_eq!(state.draft().context_window, "16384");
        assert_eq!(state.draft().default_model, "claude-3-5-sonnet");

        let unchanged = state.update_draft_field(SettingsField::ContextWindow, "16384".to_string());
        assert!(!unchanged);
    }

    #[test]
    fn save_and_close_promotes_draft_and_updates_provider_status() {
        let mut state = SettingsUiState::new(SettingsDraftState::default());
        state.open_settings();

        let mut draft = state.draft().clone();
        draft.anthropic_api_key = "sk-ant-valid-key".to_string();
        state.update_draft(draft.clone());

        assert_eq!(state.draft_provider_status(), ProviderStatus::Connected);
        assert!(state.settings_dirty);

        let changed = state.save_and_close();
        assert!(changed);
        assert_eq!(state.screen, UiScreen::Main);
        assert!(!state.settings_dirty);
        assert_eq!(state.provider_status, ProviderStatus::Connected);
        assert_eq!(state.saved(), &draft);
    }

    #[test]
    fn discard_and_close_reverts_draft_to_saved_state() {
        let mut state = SettingsUiState::new(SettingsDraftState::with_default_model("stub-model"));
        state.open_settings();

        let mut draft = state.draft().clone();
        draft.custom_base_url = "https://localhost:8080/v1".to_string();
        state.update_draft(draft);

        assert!(state.settings_dirty);
        assert_eq!(state.dirty_fields(), vec![SettingsField::CustomBaseUrl]);

        let discarded = state.discard_and_close();
        assert!(discarded);
        assert_eq!(state.screen, UiScreen::Main);
        assert!(!state.settings_dirty);
        assert_eq!(state.draft(), state.saved());
    }

    #[test]
    fn provider_status_detects_not_configured_and_invalid_key() {
        let mut state = SettingsUiState::new(SettingsDraftState::default());
        assert_eq!(state.provider_status, ProviderStatus::NotConfigured);

        let mut invalid_key_draft = state.draft().clone();
        invalid_key_draft.anthropic_api_key = "invalid key".to_string();
        state.update_draft(invalid_key_draft);
        assert_eq!(state.draft_provider_status(), ProviderStatus::InvalidKey);
    }
}
