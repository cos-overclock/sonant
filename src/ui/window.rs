use std::sync::Arc;
use std::time::Duration;

use gpui::{
    App, AppContext, Context, Entity, ExternalPaths, IntoElement, PathPromptOptions, Render,
    Subscription, Task, Timer, Window, div, prelude::*, px,
};
use gpui_component::{
    Disableable,
    button::{Button, ButtonVariants as _},
    input::{Input, InputEvent, InputState},
    label::Label,
    scroll::ScrollableElement,
    select::{Select, SelectEvent, SelectState},
};
use sonant::{
    app::{
        ChannelMapping, GenerationJobManager, GenerationJobState, GenerationJobUpdate,
        InputTrackModel, LIVE_INPUT_IPC_SOCKET_ENV, LiveInputEvent, LiveInputEventSource,
        LiveInputIpcSource, LiveMidiCapture, LoadMidiCommand, LoadMidiUseCase, MIDI_CHANNEL_MAX,
        MIDI_CHANNEL_MIN, MidiInputRouter,
    },
    domain::{
        GenerationMode, LlmError, MidiReferenceEvent, MidiReferenceSummary, ReferenceSlot,
        ReferenceSource, calculate_reference_density_hint, has_supported_midi_extension,
    },
};

use super::backend::{
    GenerationBackend, build_generation_backend, build_generation_backend_from_api_key,
};
use super::request::PromptSubmissionModel;
use super::state::{
    HelperGenerationStatus, MidiSlotErrorState, SettingsDraftState, SettingsField, SettingsTab,
    SettingsUiState, mode_reference_requirement, mode_reference_requirement_satisfied,
};
use super::theme::SonantTheme;
use super::utils::{
    choose_dropped_midi_path, display_file_name_from_path, dropped_path_to_load,
    log_generation_request_submission, normalize_api_key_input,
};
use super::{
    API_KEY_PLACEHOLDER, JOB_UPDATE_POLL_INTERVAL_MS, MIDI_SLOT_DROP_ERROR_MESSAGE,
    MIDI_SLOT_DROP_HINT, MIDI_SLOT_EMPTY_LABEL, MIDI_SLOT_FILE_PICKER_PROMPT,
    MIDI_SLOT_UNSUPPORTED_FILE_MESSAGE, PROMPT_EDITOR_HEIGHT_PX, PROMPT_EDITOR_ROWS,
    PROMPT_PLACEHOLDER, PROMPT_VALIDATION_MESSAGE, SETTINGS_ANTHROPIC_API_KEY_PLACEHOLDER,
    SETTINGS_CONTEXT_WINDOW_PLACEHOLDER, SETTINGS_CUSTOM_BASE_URL_PLACEHOLDER,
    SETTINGS_DEFAULT_MODEL_PLACEHOLDER, SETTINGS_OPENAI_API_KEY_PLACEHOLDER,
};

const LIVE_CAPTURE_POLL_INTERVAL_MS: u64 = 30;
const LIVE_CAPTURE_MAX_EVENTS_PER_POLL: usize = 512;
type DropdownState = SelectState<Vec<&'static str>>;

pub(super) struct SonantMainWindow {
    prompt_input: Entity<InputState>,
    _prompt_input_subscription: Subscription,
    generation_mode_dropdown: Entity<DropdownState>,
    _generation_mode_dropdown_subscription: Subscription,
    reference_slot_dropdown: Entity<DropdownState>,
    _reference_slot_dropdown_subscription: Subscription,
    reference_source_dropdown: Entity<DropdownState>,
    _reference_source_dropdown_subscription: Subscription,
    api_key_input: Entity<InputState>,
    _api_key_input_subscription: Subscription,
    settings_anthropic_api_key_input: Entity<InputState>,
    _settings_anthropic_api_key_subscription: Subscription,
    settings_openai_api_key_input: Entity<InputState>,
    _settings_openai_api_key_subscription: Subscription,
    settings_custom_base_url_input: Entity<InputState>,
    _settings_custom_base_url_subscription: Subscription,
    settings_default_model_input: Entity<InputState>,
    _settings_default_model_subscription: Subscription,
    settings_context_window_input: Entity<InputState>,
    _settings_context_window_subscription: Subscription,
    load_midi_use_case: Arc<LoadMidiUseCase>,
    live_midi_capture: LiveMidiCapture,
    midi_input_router: MidiInputRouter,
    generation_job_manager: Arc<GenerationJobManager>,
    submission_model: PromptSubmissionModel,
    settings_ui_state: SettingsUiState,
    is_syncing_settings_inputs: bool,
    input_track_model: InputTrackModel,
    recording_channel_enabled: [bool; 16],
    live_capture_transport_playing: bool,
    live_capture_playhead_ppq: f64,
    selected_generation_mode: GenerationMode,
    selected_reference_slot: ReferenceSlot,
    generation_status: HelperGenerationStatus,
    validation_error: Option<String>,
    api_key_error: Option<String>,
    input_track_error: Option<String>,
    midi_slot_errors: Vec<MidiSlotErrorState>,
    active_test_api_key: Option<String>,
    startup_notice: Option<String>,
    _update_poll_task: Task<()>,
    _live_capture_poll_task: Task<()>,
    _midi_file_picker_task: Task<()>,
}

impl SonantMainWindow {
    pub(super) fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let prompt_input = cx.new(|cx| {
            InputState::new(window, cx)
                .multi_line(true)
                .rows(PROMPT_EDITOR_ROWS)
                .placeholder(PROMPT_PLACEHOLDER)
        });
        let prompt_input_subscription =
            cx.subscribe_in(&prompt_input, window, Self::on_prompt_input_event);
        let generation_mode_dropdown =
            cx.new(|cx| SelectState::new(Self::generation_mode_dropdown_items(), None, window, cx));
        let generation_mode_dropdown_subscription = cx.subscribe_in(
            &generation_mode_dropdown,
            window,
            Self::on_generation_mode_dropdown_event,
        );
        let reference_slot_dropdown =
            cx.new(|cx| SelectState::new(Self::reference_slot_dropdown_items(), None, window, cx));
        let reference_slot_dropdown_subscription = cx.subscribe_in(
            &reference_slot_dropdown,
            window,
            Self::on_reference_slot_dropdown_event,
        );
        let reference_source_dropdown = cx
            .new(|cx| SelectState::new(Self::reference_source_dropdown_items(), None, window, cx));
        let reference_source_dropdown_subscription = cx.subscribe_in(
            &reference_source_dropdown,
            window,
            Self::on_reference_source_dropdown_event,
        );
        let api_key_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(API_KEY_PLACEHOLDER)
                .masked(true)
        });
        let api_key_input_subscription =
            cx.subscribe_in(&api_key_input, window, Self::on_api_key_input_event);
        let settings_anthropic_api_key_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(SETTINGS_ANTHROPIC_API_KEY_PLACEHOLDER)
                .masked(true)
        });
        let settings_anthropic_api_key_subscription = cx.subscribe_in(
            &settings_anthropic_api_key_input,
            window,
            Self::on_settings_input_event,
        );
        let settings_openai_api_key_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(SETTINGS_OPENAI_API_KEY_PLACEHOLDER)
                .masked(true)
        });
        let settings_openai_api_key_subscription = cx.subscribe_in(
            &settings_openai_api_key_input,
            window,
            Self::on_settings_input_event,
        );
        let settings_custom_base_url_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder(SETTINGS_CUSTOM_BASE_URL_PLACEHOLDER)
        });
        let settings_custom_base_url_subscription = cx.subscribe_in(
            &settings_custom_base_url_input,
            window,
            Self::on_settings_input_event,
        );
        let settings_default_model_input = cx
            .new(|cx| InputState::new(window, cx).placeholder(SETTINGS_DEFAULT_MODEL_PLACEHOLDER));
        let settings_default_model_subscription = cx.subscribe_in(
            &settings_default_model_input,
            window,
            Self::on_settings_input_event,
        );
        let settings_context_window_input = cx
            .new(|cx| InputState::new(window, cx).placeholder(SETTINGS_CONTEXT_WINDOW_PLACEHOLDER));
        let settings_context_window_subscription = cx.subscribe_in(
            &settings_context_window_input,
            window,
            Self::on_settings_input_event,
        );

        let backend = build_generation_backend();
        let settings_ui_state = SettingsUiState::new(SettingsDraftState::with_default_model(
            backend.default_model.model.clone(),
        ));
        let input_track_model = InputTrackModel::new();
        let recording_channel_enabled = [false; 16];
        let (live_input_source, live_input_error) = resolve_live_input_source();
        let live_midi_capture = LiveMidiCapture::new(live_input_source);
        let midi_input_router = MidiInputRouter::new();

        let mut this = Self {
            prompt_input,
            _prompt_input_subscription: prompt_input_subscription,
            generation_mode_dropdown,
            _generation_mode_dropdown_subscription: generation_mode_dropdown_subscription,
            reference_slot_dropdown,
            _reference_slot_dropdown_subscription: reference_slot_dropdown_subscription,
            reference_source_dropdown,
            _reference_source_dropdown_subscription: reference_source_dropdown_subscription,
            api_key_input,
            _api_key_input_subscription: api_key_input_subscription,
            settings_anthropic_api_key_input,
            _settings_anthropic_api_key_subscription: settings_anthropic_api_key_subscription,
            settings_openai_api_key_input,
            _settings_openai_api_key_subscription: settings_openai_api_key_subscription,
            settings_custom_base_url_input,
            _settings_custom_base_url_subscription: settings_custom_base_url_subscription,
            settings_default_model_input,
            _settings_default_model_subscription: settings_default_model_subscription,
            settings_context_window_input,
            _settings_context_window_subscription: settings_context_window_subscription,
            load_midi_use_case: Arc::new(LoadMidiUseCase::new()),
            live_midi_capture,
            midi_input_router,
            generation_job_manager: Arc::clone(&backend.job_manager),
            submission_model: PromptSubmissionModel::new(backend.default_model),
            settings_ui_state,
            is_syncing_settings_inputs: false,
            input_track_model,
            recording_channel_enabled,
            live_capture_transport_playing: false,
            live_capture_playhead_ppq: 0.0,
            selected_generation_mode: GenerationMode::Melody,
            selected_reference_slot: ReferenceSlot::Melody,
            generation_status: HelperGenerationStatus::Idle,
            validation_error: None,
            api_key_error: None,
            input_track_error: live_input_error,
            midi_slot_errors: Vec::new(),
            active_test_api_key: None,
            startup_notice: backend.startup_notice,
            _update_poll_task: Task::ready(()),
            _live_capture_poll_task: Task::ready(()),
            _midi_file_picker_task: Task::ready(()),
        };
        if let Err(error) = this.sync_midi_input_router_config() {
            this.input_track_error = Some(error);
        }
        this.sync_dropdowns(window, cx);
        this.sync_settings_inputs_from_draft(window, cx);
        this.start_live_capture_polling(window, cx);
        this
    }

    fn on_prompt_input_event(
        &mut self,
        _state: &Entity<InputState>,
        event: &InputEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if matches!(event, InputEvent::Change) && self.validation_error.take().is_some() {
            cx.notify();
        }
    }

    fn on_api_key_input_event(
        &mut self,
        _state: &Entity<InputState>,
        event: &InputEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if matches!(event, InputEvent::Change) && self.api_key_error.take().is_some() {
            cx.notify();
        }
    }

    fn on_settings_input_event(
        &mut self,
        state: &Entity<InputState>,
        event: &InputEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if matches!(event, InputEvent::Change)
            && !self.is_syncing_settings_inputs
            && self.sync_settings_draft_field_from_input(state, cx)
        {
            cx.notify();
        }
    }

    fn generation_mode_dropdown_items() -> Vec<&'static str> {
        vec![
            Self::generation_mode_label(GenerationMode::Melody),
            Self::generation_mode_label(GenerationMode::ChordProgression),
            Self::generation_mode_label(GenerationMode::DrumPattern),
            Self::generation_mode_label(GenerationMode::Bassline),
            Self::generation_mode_label(GenerationMode::CounterMelody),
            Self::generation_mode_label(GenerationMode::Harmony),
            Self::generation_mode_label(GenerationMode::Continuation),
        ]
    }

    fn reference_slot_dropdown_items() -> Vec<&'static str> {
        Self::reference_slots()
            .iter()
            .copied()
            .map(Self::reference_slot_label)
            .collect()
    }

    fn reference_source_dropdown_items() -> Vec<&'static str> {
        vec![
            Self::reference_source_label(ReferenceSource::File),
            Self::reference_source_label(ReferenceSource::Live),
        ]
    }

    fn generation_mode_from_label(label: &str) -> Option<GenerationMode> {
        // Derive the reverse mapping from the single-sourced label helper
        let all_modes = [
            GenerationMode::Melody,
            GenerationMode::ChordProgression,
            GenerationMode::DrumPattern,
            GenerationMode::Bassline,
            GenerationMode::CounterMelody,
            GenerationMode::Harmony,
            GenerationMode::Continuation,
        ];

        all_modes
            .iter()
            .copied()
            .find(|mode| Self::generation_mode_label(*mode) == label)
    }

    fn reference_slot_from_label(label: &str) -> Option<ReferenceSlot> {
        // Derive the reverse mapping from the single-sourced label helper
        let all_slots = [
            ReferenceSlot::Melody,
            ReferenceSlot::ChordProgression,
            ReferenceSlot::DrumPattern,
            ReferenceSlot::Bassline,
            ReferenceSlot::CounterMelody,
            ReferenceSlot::Harmony,
            ReferenceSlot::ContinuationSeed,
        ];

        all_slots
            .iter()
            .copied()
            .find(|slot| Self::reference_slot_label(*slot) == label)
    }

    fn reference_source_from_label(label: &str) -> Option<ReferenceSource> {
        // Derive the reverse mapping from the single-sourced label helper
        let all_sources = [ReferenceSource::File, ReferenceSource::Live];

        all_sources
            .iter()
            .copied()
            .find(|source| Self::reference_source_label(*source) == label)
    }

    fn sync_dropdowns(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mode_label = Self::generation_mode_label(self.selected_generation_mode);
        self.generation_mode_dropdown.update(cx, |state, cx| {
            state.set_selected_value(&mode_label, window, cx);
        });

        let slot_label = Self::reference_slot_label(self.selected_reference_slot);
        self.reference_slot_dropdown.update(cx, |state, cx| {
            state.set_selected_value(&slot_label, window, cx);
        });

        let source_label =
            Self::reference_source_label(self.source_for_slot(self.selected_reference_slot));
        self.reference_source_dropdown.update(cx, |state, cx| {
            state.set_selected_value(&source_label, window, cx);
        });
    }

    fn on_generation_mode_dropdown_event(
        &mut self,
        _state: &Entity<DropdownState>,
        event: &SelectEvent<Vec<&'static str>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let SelectEvent::Confirm(selected_label) = event;
        let Some(selected_label) = selected_label.as_deref() else {
            return;
        };
        let Some(mode) = Self::generation_mode_from_label(selected_label) else {
            return;
        };
        self.on_generation_mode_selected(mode, cx);
    }

    fn on_reference_slot_dropdown_event(
        &mut self,
        _state: &Entity<DropdownState>,
        event: &SelectEvent<Vec<&'static str>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let SelectEvent::Confirm(selected_label) = event;
        let Some(selected_label) = selected_label.as_deref() else {
            return;
        };
        let Some(slot) = Self::reference_slot_from_label(selected_label) else {
            return;
        };

        self.on_reference_slot_selected(slot, cx);
        self.sync_dropdowns(window, cx);
    }

    fn on_reference_source_dropdown_event(
        &mut self,
        _state: &Entity<DropdownState>,
        event: &SelectEvent<Vec<&'static str>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let SelectEvent::Confirm(selected_label) = event;
        let Some(selected_label) = selected_label.as_deref() else {
            return;
        };
        let Some(source) = Self::reference_source_from_label(selected_label) else {
            return;
        };

        self.on_reference_source_selected(self.selected_reference_slot, source, cx);
        self.sync_dropdowns(window, cx);
    }

    fn on_open_settings_clicked(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.settings_ui_state.open_settings();
        self.sync_settings_inputs_from_draft(window, cx);
        cx.notify();
    }

    fn on_close_settings_clicked(&mut self, cx: &mut Context<Self>) {
        self.settings_ui_state.close_settings();
        cx.notify();
    }

    fn on_settings_tab_selected(&mut self, tab: SettingsTab, cx: &mut Context<Self>) {
        if self.settings_ui_state.settings_tab != tab {
            self.settings_ui_state.select_settings_tab(tab);
            cx.notify();
        }
    }

    fn on_discard_settings_clicked(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.settings_ui_state.discard_and_close();
        self.sync_settings_inputs_from_draft(window, cx);
        cx.notify();
    }

    fn on_save_settings_clicked(&mut self, cx: &mut Context<Self>) {
        self.sync_settings_state_from_inputs(cx);
        self.settings_ui_state.save_and_close();
        cx.notify();
    }

    fn sync_settings_inputs_from_draft(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let draft = self.settings_ui_state.draft().clone();
        self.is_syncing_settings_inputs = true;
        self.settings_anthropic_api_key_input
            .update(cx, |input, cx| {
                input.set_value(draft.anthropic_api_key.clone(), window, cx);
            });
        self.settings_openai_api_key_input.update(cx, |input, cx| {
            input.set_value(draft.openai_api_key.clone(), window, cx);
        });
        self.settings_custom_base_url_input.update(cx, |input, cx| {
            input.set_value(draft.custom_base_url.clone(), window, cx);
        });
        self.settings_default_model_input.update(cx, |input, cx| {
            input.set_value(draft.default_model.clone(), window, cx);
        });
        self.settings_context_window_input.update(cx, |input, cx| {
            input.set_value(draft.context_window.clone(), window, cx);
        });
        self.is_syncing_settings_inputs = false;
    }

    fn sync_settings_draft_field_from_input(
        &mut self,
        state: &Entity<InputState>,
        cx: &App,
    ) -> bool {
        let field = if state == &self.settings_anthropic_api_key_input {
            Some(SettingsField::AnthropicApiKey)
        } else if state == &self.settings_openai_api_key_input {
            Some(SettingsField::OpenAiApiKey)
        } else if state == &self.settings_custom_base_url_input {
            Some(SettingsField::CustomBaseUrl)
        } else if state == &self.settings_default_model_input {
            Some(SettingsField::DefaultModel)
        } else if state == &self.settings_context_window_input {
            Some(SettingsField::ContextWindow)
        } else {
            None
        };

        let Some(field) = field else {
            return false;
        };

        let value = state.read(cx).value().to_string();
        self.settings_ui_state.update_draft_field(field, value)
    }

    fn collect_settings_draft_from_inputs(&self, cx: &App) -> SettingsDraftState {
        SettingsDraftState {
            anthropic_api_key: self
                .settings_anthropic_api_key_input
                .read(cx)
                .value()
                .to_string(),
            openai_api_key: self
                .settings_openai_api_key_input
                .read(cx)
                .value()
                .to_string(),
            custom_base_url: self
                .settings_custom_base_url_input
                .read(cx)
                .value()
                .to_string(),
            default_model: self
                .settings_default_model_input
                .read(cx)
                .value()
                .to_string(),
            context_window: self
                .settings_context_window_input
                .read(cx)
                .value()
                .to_string(),
        }
    }

    fn sync_settings_state_from_inputs(&mut self, cx: &App) {
        let draft = self.collect_settings_draft_from_inputs(cx);
        self.settings_ui_state.update_draft(draft);
    }

    fn on_generate_clicked(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.validation_error = None;
        self.api_key_error = None;

        if let Err(error) = self.sync_backend_from_api_key_input(cx) {
            self.generation_status = HelperGenerationStatus::Failed {
                message: error.user_message(),
            };
            self.api_key_error = Some(error.user_message());
            cx.notify();
            return;
        }

        let references = self.collect_generation_references();
        if !mode_reference_requirement_satisfied(self.selected_generation_mode, &references) {
            let message = mode_reference_requirement(self.selected_generation_mode)
                .unmet_message
                .unwrap_or("Selected generation mode requires additional MIDI references.")
                .to_string();
            self.generation_status = HelperGenerationStatus::Failed { message };
            cx.notify();
            return;
        }

        let prompt = self.prompt_input.read(cx).value().to_string();
        let request = match self.submission_model.prepare_request(
            self.selected_generation_mode,
            prompt,
            references,
        ) {
            Ok(request) => request,
            Err(LlmError::Validation { .. }) => {
                self.generation_status = HelperGenerationStatus::Idle;
                self.validation_error = Some(PROMPT_VALIDATION_MESSAGE.to_string());
                self.prompt_input
                    .update(cx, |input, cx| input.focus(window, cx));
                cx.notify();
                return;
            }
            Err(error) => {
                self.generation_status = HelperGenerationStatus::Failed {
                    message: error.user_message(),
                };
                cx.notify();
                return;
            }
        };

        // `prepare_request` only validates prompt text; run full contract validation here.
        if let Err(error) = request.validate() {
            self.generation_status = HelperGenerationStatus::Failed {
                message: error.user_message(),
            };
            self.upsert_midi_slot_error(MidiSlotErrorState::non_retryable(
                self.selected_reference_slot,
                error.user_message(),
            ));
            cx.notify();
            return;
        }

        self.generation_status = HelperGenerationStatus::Submitting {
            request_id: request.request_id.clone(),
        };

        log_generation_request_submission(&request);

        if let Err(error) = self.generation_job_manager.submit_generate(request) {
            self.generation_status = HelperGenerationStatus::Failed {
                message: error.user_message(),
            };
        } else {
            self.start_update_polling(window, cx);
        }

        cx.notify();
    }

    fn on_generation_mode_selected(&mut self, mode: GenerationMode, cx: &mut Context<Self>) {
        if self.selected_generation_mode != mode {
            self.selected_generation_mode = mode;
            cx.notify();
        }
    }

    fn generation_mode_label(mode: GenerationMode) -> &'static str {
        match mode {
            GenerationMode::Melody => "Melody",
            GenerationMode::ChordProgression => "Chord Progression",
            GenerationMode::DrumPattern => "Drum Pattern",
            GenerationMode::Bassline => "Bassline",
            GenerationMode::CounterMelody => "Counter Melody",
            GenerationMode::Harmony => "Harmony",
            GenerationMode::Continuation => "Continuation",
        }
    }

    fn reference_slots() -> &'static [ReferenceSlot] {
        const SLOTS: [ReferenceSlot; 7] = [
            ReferenceSlot::Melody,
            ReferenceSlot::ChordProgression,
            ReferenceSlot::DrumPattern,
            ReferenceSlot::Bassline,
            ReferenceSlot::CounterMelody,
            ReferenceSlot::Harmony,
            ReferenceSlot::ContinuationSeed,
        ];
        &SLOTS
    }

    fn reference_slot_label(slot: ReferenceSlot) -> &'static str {
        match slot {
            ReferenceSlot::Melody => "Melody",
            ReferenceSlot::ChordProgression => "Chord Progression",
            ReferenceSlot::DrumPattern => "Drum Pattern",
            ReferenceSlot::Bassline => "Bassline",
            ReferenceSlot::CounterMelody => "Counter Melody",
            ReferenceSlot::Harmony => "Harmony",
            ReferenceSlot::ContinuationSeed => "Continuation Seed",
        }
    }

    fn reference_source_label(source: ReferenceSource) -> &'static str {
        match source {
            ReferenceSource::File => "File",
            ReferenceSource::Live => "Live",
        }
    }

    fn reference_slot_index(slot: ReferenceSlot) -> usize {
        match slot {
            ReferenceSlot::Melody => 0,
            ReferenceSlot::ChordProgression => 1,
            ReferenceSlot::DrumPattern => 2,
            ReferenceSlot::Bassline => 3,
            ReferenceSlot::CounterMelody => 4,
            ReferenceSlot::Harmony => 5,
            ReferenceSlot::ContinuationSeed => 6,
        }
    }

    fn input_track_channel_button_id(slot: ReferenceSlot, channel: u8) -> (&'static str, usize) {
        (
            "input-track-channel",
            Self::reference_slot_index(slot) * 100 + usize::from(channel),
        )
    }

    fn recording_channel_button_id(channel: u8) -> (&'static str, usize) {
        ("recording-channel", usize::from(channel))
    }

    fn settings_tab_button_id(tab: SettingsTab) -> &'static str {
        match tab {
            SettingsTab::ApiKeys => "settings-tab-api-keys",
            SettingsTab::MidiSettings => "settings-tab-midi-settings",
            SettingsTab::General => "settings-tab-general",
        }
    }

    fn on_reference_slot_selected(&mut self, slot: ReferenceSlot, cx: &mut Context<Self>) {
        if self.selected_reference_slot != slot {
            self.selected_reference_slot = slot;
            cx.notify();
        }
    }

    fn source_for_slot(&self, slot: ReferenceSlot) -> ReferenceSource {
        self.input_track_model.source_for_slot(slot)
    }

    fn channel_mapping_for_slot(&self, slot: ReferenceSlot) -> Option<u8> {
        self.input_track_model
            .channel_mappings()
            .iter()
            .find(|mapping| mapping.slot == slot)
            .map(|mapping| mapping.channel)
    }

    fn recording_enabled_for_channel(&self, channel: u8) -> bool {
        recording_enabled_for_channel_array(&self.recording_channel_enabled, channel)
    }

    fn collect_generation_references(&self) -> Vec<MidiReferenceSummary> {
        let mut references = self.load_midi_use_case.snapshot_references();
        references.extend(collect_live_references(
            &self.input_track_model,
            &self.recording_channel_enabled,
            &self.midi_input_router,
        ));
        references
    }

    fn live_channel_used_by_other_slots(&self, slot: ReferenceSlot, channel: u8) -> bool {
        live_channel_used_by_other_slots(&self.input_track_model, slot, channel)
    }

    fn ensure_live_channel_mapping_for_slot(&mut self, slot: ReferenceSlot) -> Result<(), String> {
        let live_channel_mappings = self.input_track_model.live_channel_mappings();
        let target_channel = resolve_live_channel_mapping_for_slot(
            slot,
            self.channel_mapping_for_slot(slot),
            &live_channel_mappings,
        )?;

        self.input_track_model
            .set_channel_mapping(ChannelMapping {
                slot,
                channel: target_channel,
            })
            .map_err(|error| error.to_string())
    }

    fn on_reference_source_selected(
        &mut self,
        slot: ReferenceSlot,
        source: ReferenceSource,
        cx: &mut Context<Self>,
    ) {
        self.input_track_error = None;

        if self.source_for_slot(slot) == source {
            return;
        }

        if source == ReferenceSource::Live
            && let Err(message) = self.ensure_live_channel_mapping_for_slot(slot)
        {
            self.input_track_error = Some(message);
            cx.notify();
            return;
        }

        if let Err(error) = self.input_track_model.set_source_for_slot(slot, source) {
            self.input_track_error = Some(error.to_string());
        } else {
            if let Err(error) = self.sync_midi_input_router_config() {
                self.input_track_error = Some(error);
            }
            self.selected_reference_slot = slot;
        }

        cx.notify();
    }

    fn on_live_channel_selected(
        &mut self,
        slot: ReferenceSlot,
        channel: u8,
        cx: &mut Context<Self>,
    ) {
        self.input_track_error = None;

        if self.source_for_slot(slot) != ReferenceSource::Live {
            return;
        }
        if self.live_channel_used_by_other_slots(slot, channel) {
            return;
        }

        if let Err(error) = self
            .input_track_model
            .set_channel_mapping(ChannelMapping { slot, channel })
        {
            self.input_track_error = Some(error.to_string());
        } else if let Err(error) = self.sync_midi_input_router_config() {
            self.input_track_error = Some(error);
        }

        cx.notify();
    }

    fn on_recording_channel_toggled(&mut self, channel: u8, cx: &mut Context<Self>) {
        if !(MIDI_CHANNEL_MIN..=MIDI_CHANNEL_MAX).contains(&channel) {
            return;
        }

        let index = usize::from(channel - MIDI_CHANNEL_MIN);
        self.recording_channel_enabled[index] = !self.recording_channel_enabled[index];
        if let Err(error) = self.sync_midi_input_router_config() {
            self.input_track_error = Some(error);
        }
        cx.notify();
    }

    fn upsert_midi_slot_error(&mut self, error: MidiSlotErrorState) {
        if let Some(existing) = self
            .midi_slot_errors
            .iter_mut()
            .find(|existing| existing.slot == error.slot)
        {
            *existing = error;
        } else {
            self.midi_slot_errors.push(error);
        }
    }

    fn clear_midi_slot_error(&mut self, slot: ReferenceSlot) {
        self.midi_slot_errors
            .retain(|existing| existing.slot != slot);
    }

    fn midi_slot_error_for_slot(&self, slot: ReferenceSlot) -> Option<&MidiSlotErrorState> {
        self.midi_slot_errors
            .iter()
            .find(|error| error.slot == slot)
    }

    fn sync_midi_input_router_config(&mut self) -> Result<(), String> {
        self.midi_input_router
            .update_channel_mapping(self.input_track_model.live_channel_mappings())
            .map_err(|error| error.to_string())?;

        for channel in MIDI_CHANNEL_MIN..=MIDI_CHANNEL_MAX {
            self.midi_input_router
                .set_recording_channel_enabled(channel, self.recording_enabled_for_channel(channel))
                .map_err(|error| error.to_string())?;
        }

        self.midi_input_router.update_transport_state(
            self.live_capture_transport_playing,
            self.live_capture_playhead_ppq,
        );
        Ok(())
    }

    fn start_live_capture_polling(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self._live_capture_poll_task = cx.spawn_in(window, async move |view, window| {
            loop {
                Timer::after(Duration::from_millis(LIVE_CAPTURE_POLL_INTERVAL_MS)).await;
                let keep_polling = match view.update_in(window, |view, _window, cx| {
                    view.poll_live_capture_events(cx)
                }) {
                    Ok(keep_polling) => keep_polling,
                    Err(_) => break,
                };

                if !keep_polling {
                    break;
                }
            }
        });
    }

    fn poll_live_capture_events(&mut self, cx: &mut Context<Self>) -> bool {
        let _ = self.live_midi_capture.ingest_available();
        let mut routed_any = false;

        loop {
            let events = self
                .live_midi_capture
                .poll_events(LIVE_CAPTURE_MAX_EVENTS_PER_POLL);
            let event_count = events.len();
            if event_count == 0 {
                break;
            }

            self.route_live_events_to_router(events);
            routed_any = true;

            if event_count < LIVE_CAPTURE_MAX_EVENTS_PER_POLL {
                break;
            }
        }

        if routed_any {
            cx.notify();
        }

        true
    }

    fn route_live_events_to_router(&mut self, events: Vec<LiveInputEvent>) {
        let mut routable_events = Vec::with_capacity(events.len());
        let mut last_transport_state = None;

        for event in events {
            last_transport_state = Some((event.is_transport_playing, event.playhead_ppq));

            let Some(channel) = midi_channel_from_status(event.data[0]) else {
                continue;
            };
            routable_events.push((channel, event));
        }

        let last_routable_transport_state = routable_events
            .last()
            .map(|(_channel, event)| (event.is_transport_playing, event.playhead_ppq));

        self.midi_input_router
            .push_live_events_with_transport(&routable_events);

        if let Some((is_transport_playing, playhead_ppq)) = last_transport_state {
            self.live_capture_transport_playing = is_transport_playing;
            self.live_capture_playhead_ppq = playhead_ppq;
            if Some((is_transport_playing, playhead_ppq)) != last_routable_transport_state {
                self.midi_input_router
                    .update_transport_state(is_transport_playing, playhead_ppq);
            }
        }
    }

    fn live_recording_summary_for_slot(&self, slot: ReferenceSlot) -> LiveRecordingSummary {
        let events = self.midi_input_router.snapshot_reference(slot);
        let metrics = self.midi_input_router.reference_metrics(slot);
        summarize_live_recording(&events, metrics.bar_count)
    }

    fn start_update_polling(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self._update_poll_task = cx.spawn_in(window, async move |view, window| {
            loop {
                Timer::after(Duration::from_millis(JOB_UPDATE_POLL_INTERVAL_MS)).await;
                let keep_polling = match view
                    .update_in(window, |view, _window, cx| view.poll_generation_updates(cx))
                {
                    Ok(keep_polling) => keep_polling,
                    Err(_) => break,
                };

                if !keep_polling {
                    break;
                }
            }
        });
    }

    fn on_select_midi_file_clicked(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.source_for_slot(self.selected_reference_slot) != ReferenceSource::File {
            self.input_track_error = Some(format!(
                "{} is set to Live input. Switch source to File to load MIDI files.",
                Self::reference_slot_label(self.selected_reference_slot)
            ));
            cx.notify();
            return;
        }

        // NOTE: gpui::PathPromptOptions (v0.2.2) does not expose extension-based file filters.
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(MIDI_SLOT_FILE_PICKER_PROMPT.into()),
        });

        self._midi_file_picker_task = cx.spawn_in(window, async move |view, window| {
            let result = receiver.await;
            let Ok(result) = result else {
                return;
            };

            match result {
                Ok(Some(paths)) => {
                    if let Some(path) = paths.into_iter().next() {
                        let _ = view.update_in(window, |view, _window, cx| {
                            let slot = view.selected_reference_slot;
                            if !has_supported_midi_extension(&path) {
                                view.upsert_midi_slot_error(MidiSlotErrorState::non_retryable(
                                    slot,
                                    MIDI_SLOT_UNSUPPORTED_FILE_MESSAGE,
                                ));
                                cx.notify();
                                return;
                            }

                            let path = path.to_string_lossy().to_string();
                            view.set_midi_slot_file(slot, path, cx);
                        });
                    }
                }
                Ok(None) => {}
                Err(error) => {
                    let message = format!("Could not open the file dialog: {error}");
                    let _ = view.update_in(window, |view, _window, cx| {
                        let slot = view.selected_reference_slot;
                        view.upsert_midi_slot_error(MidiSlotErrorState::non_retryable(
                            slot, message,
                        ));
                        cx.notify();
                    });
                }
            }
        });
    }

    fn on_midi_slot_drop(&mut self, paths: &ExternalPaths, cx: &mut Context<Self>) {
        let slot = self.selected_reference_slot;
        if self.source_for_slot(slot) != ReferenceSource::File {
            self.input_track_error = Some(format!(
                "{} is set to Live input. Switch source to File to load dropped MIDI files.",
                Self::reference_slot_label(slot)
            ));
            cx.notify();
            return;
        }
        let Some(path) = dropped_path_to_load(paths) else {
            self.upsert_midi_slot_error(MidiSlotErrorState::non_retryable(
                slot,
                MIDI_SLOT_DROP_ERROR_MESSAGE,
            ));
            cx.notify();
            return;
        };

        self.set_midi_slot_file(slot, path, cx);
    }

    fn set_midi_slot_file(&mut self, slot: ReferenceSlot, path: String, cx: &mut Context<Self>) {
        self.clear_midi_slot_error(slot);
        match self.load_midi_use_case.execute(LoadMidiCommand::SetFile {
            slot,
            path: path.clone(),
        }) {
            Ok(_) => cx.notify(),
            Err(error) => {
                self.upsert_midi_slot_error(MidiSlotErrorState::from_load_error(
                    slot, &path, &error,
                ));
                cx.notify();
            }
        }
    }

    fn on_retry_midi_slot_clicked(&mut self, slot: ReferenceSlot, cx: &mut Context<Self>) {
        let retry_path = self
            .midi_slot_error_for_slot(slot)
            .and_then(|error| error.retry_path.clone());
        if let Some(path) = retry_path {
            self.set_midi_slot_file(slot, path, cx);
        }
    }

    fn on_clear_midi_slot_clicked(&mut self, cx: &mut Context<Self>) {
        let slot = self.selected_reference_slot;
        self.clear_midi_slot_error(slot);
        match self
            .load_midi_use_case
            .execute(LoadMidiCommand::ClearSlot { slot })
        {
            Ok(_) => cx.notify(),
            Err(error) => {
                self.upsert_midi_slot_error(MidiSlotErrorState::non_retryable(
                    slot,
                    error.user_message(),
                ));
                cx.notify();
            }
        }
    }

    fn sync_backend_from_api_key_input(&mut self, cx: &mut Context<Self>) -> Result<(), LlmError> {
        let current_input = self.api_key_input_value(cx);

        match current_input {
            Some(ref api_key) if self.active_test_api_key.as_deref() != Some(api_key) => {
                let backend = build_generation_backend_from_api_key(api_key)?;
                self.apply_generation_backend(backend);
                self.active_test_api_key = Some(api_key.clone());
            }
            None if self.active_test_api_key.take().is_some() => {
                let backend = build_generation_backend();
                self.apply_generation_backend(backend);
            }
            _ => {}
        }

        Ok(())
    }

    fn api_key_input_value(&self, cx: &App) -> Option<String> {
        let value = self.api_key_input.read(cx).value();
        normalize_api_key_input(value.as_ref())
    }

    fn apply_generation_backend(&mut self, backend: GenerationBackend) {
        self.generation_job_manager = Arc::clone(&backend.job_manager);
        self.submission_model.set_model(backend.default_model);
        self.startup_notice = backend.startup_notice;
    }

    fn poll_generation_updates(&mut self, cx: &mut Context<Self>) -> bool {
        let updates = self.generation_job_manager.drain_updates();
        if !updates.is_empty() {
            for update in updates {
                self.apply_generation_update(update);
            }

            cx.notify();
        }

        self.generation_status.is_submitting_or_running()
    }

    fn apply_generation_update(&mut self, update: GenerationJobUpdate) {
        self.generation_status = match update.state {
            GenerationJobState::Idle => HelperGenerationStatus::Idle,
            GenerationJobState::Running => HelperGenerationStatus::Running {
                request_id: update.request_id,
            },
            GenerationJobState::Succeeded => {
                let candidate_count = update
                    .result
                    .as_ref()
                    .map(|result| result.candidates.len())
                    .unwrap_or(0);
                HelperGenerationStatus::Succeeded {
                    request_id: update.request_id,
                    candidate_count,
                }
            }
            GenerationJobState::Failed => {
                let message = update
                    .error
                    .map(|error| error.user_message())
                    .unwrap_or_else(|| "Generation failed for an unknown reason.".to_string());
                HelperGenerationStatus::Failed { message }
            }
            GenerationJobState::Cancelled => HelperGenerationStatus::Cancelled {
                request_id: update.request_id,
            },
        };
    }
}

struct NoopLiveInputSource;

impl LiveInputEventSource for NoopLiveInputSource {
    fn try_pop_live_input_event(&self) -> Option<LiveInputEvent> {
        None
    }
}

fn resolve_live_input_source() -> (Arc<dyn LiveInputEventSource>, Option<String>) {
    let Ok(socket_path) = std::env::var(LIVE_INPUT_IPC_SOCKET_ENV) else {
        return (Arc::new(NoopLiveInputSource), None);
    };
    match LiveInputIpcSource::bind(&socket_path) {
        Ok(source) => (Arc::new(source), None),
        Err(error) => (
            Arc::new(NoopLiveInputSource),
            Some(format!(
                "Live input socket could not be opened ({socket_path}): {error}"
            )),
        ),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct LiveRecordingSummary {
    bar_count: usize,
    event_count: usize,
    note_count: usize,
    min_pitch: Option<u8>,
    max_pitch: Option<u8>,
}

fn summarize_live_recording(events: &[LiveInputEvent], bar_count: usize) -> LiveRecordingSummary {
    let mut summary = LiveRecordingSummary {
        bar_count,
        event_count: events.len(),
        ..Default::default()
    };

    for event in events {
        if is_note_on_event(*event) {
            summary.note_count += 1;
            let pitch = event.data[1];
            summary.min_pitch = Some(summary.min_pitch.map_or(pitch, |min| min.min(pitch)));
            summary.max_pitch = Some(summary.max_pitch.map_or(pitch, |max| max.max(pitch)));
        }
    }

    summary
}

fn is_note_on_event(event: LiveInputEvent) -> bool {
    (event.data[0] & 0xF0) == 0x90 && event.data[2] > 0
}

fn midi_channel_from_status(status: u8) -> Option<u8> {
    if (status & 0x80) == 0 {
        return None;
    }

    match status & 0xF0 {
        0x80 | 0x90 | 0xA0 | 0xB0 | 0xC0 | 0xD0 | 0xE0 => Some((status & 0x0F) + 1),
        _ => None,
    }
}

fn channel_mapping_for_slot_in_mappings(
    channel_mappings: &[ChannelMapping],
    slot: ReferenceSlot,
) -> Option<u8> {
    channel_mappings
        .iter()
        .find(|mapping| mapping.slot == slot)
        .map(|mapping| mapping.channel)
}

fn recording_enabled_for_channel_array(
    recording_channel_enabled: &[bool; 16],
    channel: u8,
) -> bool {
    if !(MIDI_CHANNEL_MIN..=MIDI_CHANNEL_MAX).contains(&channel) {
        return false;
    }
    let index = usize::from(channel - MIDI_CHANNEL_MIN);
    recording_channel_enabled[index]
}

fn collect_live_references(
    input_track_model: &InputTrackModel,
    recording_channel_enabled: &[bool; 16],
    midi_input_router: &MidiInputRouter,
) -> Vec<MidiReferenceSummary> {
    let channel_mappings = input_track_model.channel_mappings();
    SonantMainWindow::reference_slots()
        .iter()
        .copied()
        .filter_map(|slot| {
            if input_track_model.source_for_slot(slot) != ReferenceSource::Live {
                return None;
            }
            let channel = channel_mapping_for_slot_in_mappings(channel_mappings, slot)?;
            if !recording_enabled_for_channel_array(recording_channel_enabled, channel) {
                return None;
            }

            let events = midi_input_router.snapshot_reference(slot);
            let metrics = midi_input_router.reference_metrics(slot);
            build_live_reference_summary(slot, &events, metrics.bar_count)
        })
        .collect()
}

fn build_live_reference_summary(
    slot: ReferenceSlot,
    events: &[LiveInputEvent],
    bar_count: usize,
) -> Option<MidiReferenceSummary> {
    let summary = summarize_live_recording(events, bar_count);
    let (Some(min_pitch), Some(max_pitch)) = (summary.min_pitch, summary.max_pitch) else {
        return None;
    };
    if summary.note_count == 0 {
        return None;
    }

    let bars = u16::try_from(summary.bar_count.max(1)).unwrap_or(u16::MAX);
    let note_count = u32::try_from(summary.note_count).unwrap_or(u32::MAX);
    let reference = MidiReferenceSummary {
        slot,
        source: ReferenceSource::Live,
        file: None,
        bars,
        note_count,
        density_hint: calculate_reference_density_hint(note_count, bars),
        min_pitch,
        max_pitch,
        events: build_live_reference_events(events),
    };

    reference.validate().ok().map(|_| reference)
}
fn build_live_reference_events(events: &[LiveInputEvent]) -> Vec<MidiReferenceEvent> {
    let mut absolute_tick = 0_u32;
    events
        .iter()
        .copied()
        .map(|event| {
            let delta_tick = event.time;
            absolute_tick = absolute_tick.saturating_add(delta_tick);
            MidiReferenceEvent {
                track: event.port_index,
                absolute_tick,
                delta_tick,
                event: format_live_reference_event_payload(event),
            }
        })
        .collect()
}

fn format_live_reference_event_payload(event: LiveInputEvent) -> String {
    let channel = midi_channel_from_status(event.data[0])
        .map(|channel| channel.to_string())
        .unwrap_or_else(|| "n/a".to_string());
    format!(
        "LiveMidi channel={channel} status=0x{:02X} data1={} data2={} port={} time={}",
        event.data[0], event.data[1], event.data[2], event.port_index, event.time
    )
}

fn live_channel_used_by_other_slots(
    model: &InputTrackModel,
    slot: ReferenceSlot,
    channel: u8,
) -> bool {
    model.channel_mappings().iter().any(|mapping| {
        mapping.slot != slot
            && mapping.channel == channel
            && model.source_for_slot(mapping.slot) == ReferenceSource::Live
    })
}

fn live_channel_used_by_other_slots_in_mappings(
    live_channel_mappings: &[ChannelMapping],
    slot: ReferenceSlot,
    channel: u8,
) -> bool {
    live_channel_mappings
        .iter()
        .any(|mapping| mapping.slot != slot && mapping.channel == channel)
}

fn first_available_live_channel_for_slot(
    slot: ReferenceSlot,
    live_channel_mappings: &[ChannelMapping],
) -> Option<u8> {
    (MIDI_CHANNEL_MIN..=MIDI_CHANNEL_MAX).find(|channel| {
        !live_channel_used_by_other_slots_in_mappings(live_channel_mappings, slot, *channel)
    })
}

#[cfg(test)]
fn first_available_live_channel_for_slot_in_model(
    model: &InputTrackModel,
    slot: ReferenceSlot,
) -> Option<u8> {
    let live_channel_mappings = model.live_channel_mappings();
    first_available_live_channel_for_slot(slot, &live_channel_mappings)
}

#[cfg(test)]
fn preferred_live_channel_for_slot(model: &InputTrackModel, slot: ReferenceSlot) -> Option<u8> {
    model
        .channel_mappings()
        .iter()
        .find(|mapping| mapping.slot == slot)
        .map(|mapping| mapping.channel)
        .filter(|channel| !live_channel_used_by_other_slots(model, slot, *channel))
}

fn resolve_live_channel_mapping_for_slot(
    slot: ReferenceSlot,
    preferred_channel: Option<u8>,
    live_channel_mappings: &[ChannelMapping],
) -> Result<u8, String> {
    preferred_channel
        .filter(|channel| {
            !live_channel_used_by_other_slots_in_mappings(live_channel_mappings, slot, *channel)
        })
        .or_else(|| first_available_live_channel_for_slot(slot, live_channel_mappings))
        .ok_or_else(|| {
            format!(
                "No free MIDI channel is available for {}.",
                SonantMainWindow::reference_slot_label(slot)
            )
        })
}

impl Render for SonantMainWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.read_global(|theme: &SonantTheme, _| theme.clone());
        let colors = theme.colors;
        let spacing = theme.spacing;
        let radius = theme.radius;

        if self.settings_ui_state.is_settings_open() {
            let selected_tab = self.settings_ui_state.settings_tab;
            let saved_provider_status = self.settings_ui_state.provider_status;
            let draft_provider_status = self.settings_ui_state.draft_provider_status();
            let settings_dirty = self.settings_ui_state.settings_dirty;
            let dirty_fields = self.settings_ui_state.dirty_fields();
            let dirty_count = dirty_fields.len();
            let saved_settings = self.settings_ui_state.saved();
            let draft_settings = self.settings_ui_state.draft();
            let tab_button = |tab: SettingsTab| {
                let button = Button::new(Self::settings_tab_button_id(tab))
                    .label(tab.label())
                    .on_click(cx.listener(move |this, _, _window, cx| {
                        this.on_settings_tab_selected(tab, cx)
                    }));
                if selected_tab == tab {
                    button.primary()
                } else {
                    button
                }
            };

            return div()
                .size_full()
                .overflow_y_scrollbar()
                .overflow_x_hidden()
                .flex()
                .flex_col()
                .gap(spacing.section_gap)
                .p(spacing.window_padding)
                .bg(colors.surface_background)
                .text_color(colors.surface_foreground)
                .child(
                    div()
                        .id("settings-header")
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(Label::new("Settings"))
                                .child(div().text_color(colors.muted_foreground).child(
                                    "FR-09 foundation: screen transitions and draft diff state.",
                                )),
                        )
                        .child(Button::new("close-settings-button").label("Back").on_click(
                            cx.listener(|this, _, _window, cx| this.on_close_settings_clicked(cx)),
                        )),
                )
                .child(
                    div()
                        .id("provider-status-panel")
                        .flex()
                        .flex_col()
                        .gap_1()
                        .p(spacing.panel_padding)
                        .rounded(radius.panel)
                        .border_1()
                        .border_color(colors.panel_border)
                        .bg(colors.panel_background)
                        .child(
                            div()
                                .text_color(saved_provider_status.color(colors))
                                .child(format!("Saved Status: {}", saved_provider_status.label())),
                        )
                        .child(
                            div()
                                .text_color(draft_provider_status.color(colors))
                                .child(format!("Draft Status: {}", draft_provider_status.label())),
                        ),
                )
                .child(
                    div()
                        .id("settings-nav")
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(tab_button(SettingsTab::ApiKeys))
                        .child(tab_button(SettingsTab::MidiSettings))
                        .child(tab_button(SettingsTab::General)),
                )
                .child(match selected_tab {
                    SettingsTab::ApiKeys => div()
                        .id("settings-tab-api-keys-panel")
                        .flex()
                        .flex_col()
                        .gap_2()
                        .p(spacing.panel_padding)
                        .rounded(radius.panel)
                        .border_1()
                        .border_color(colors.panel_border)
                        .bg(colors.panel_background)
                        .child(Label::new("Anthropic API Key"))
                        .child(Input::new(&self.settings_anthropic_api_key_input).mask_toggle())
                        .child(Label::new("OpenAI-Compatible API Key"))
                        .child(Input::new(&self.settings_openai_api_key_input).mask_toggle())
                        .child(Label::new("Custom Base URL"))
                        .child(Input::new(&self.settings_custom_base_url_input)),
                    SettingsTab::MidiSettings => div()
                        .id("settings-tab-midi-panel")
                        .flex()
                        .flex_col()
                        .gap_2()
                        .p(spacing.panel_padding)
                        .rounded(radius.panel)
                        .border_1()
                        .border_color(colors.panel_border)
                        .bg(colors.panel_background)
                        .child(Label::new("MIDI Settings"))
                        .child(
                            div()
                                .text_color(colors.muted_foreground)
                                .child("MIDI settings UI will be added in FR-09 follow-up issues."),
                        )
                        .child(div().text_color(colors.muted_foreground).child(
                            "Current issue focuses on state transitions and dirty tracking.",
                        )),
                    SettingsTab::General => div()
                        .id("settings-tab-general-panel")
                        .flex()
                        .flex_col()
                        .gap_2()
                        .p(spacing.panel_padding)
                        .rounded(radius.panel)
                        .border_1()
                        .border_color(colors.panel_border)
                        .bg(colors.panel_background)
                        .child(Label::new("Default Model"))
                        .child(Input::new(&self.settings_default_model_input))
                        .child(Label::new("Context Window"))
                        .child(Input::new(&self.settings_context_window_input)),
                })
                .child(
                    div()
                        .id("settings-diff-panel")
                        .flex()
                        .flex_col()
                        .gap_1()
                        .p(spacing.panel_padding)
                        .rounded(radius.panel)
                        .border_1()
                        .border_color(colors.selectable_panel_border(settings_dirty))
                        .bg(colors.selectable_panel_background(settings_dirty))
                        .child(div().child(format!(
                            "settings_dirty: {} (changed fields: {dirty_count})",
                            settings_dirty
                        )))
                        .child(div().text_color(colors.muted_foreground).child(format!(
                            "Saved default model: {} / Draft default model: {}",
                            saved_settings.default_model, draft_settings.default_model
                        )))
                        .children(dirty_fields.into_iter().map(|field| {
                            div()
                                .text_color(colors.accent_foreground)
                                .child(format!("Changed: {}", field.label()))
                        })),
                )
                .child(
                    div()
                        .id("settings-footer-actions")
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .child(
                            Button::new("settings-discard-button")
                                .label("Cancel")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.on_discard_settings_clicked(window, cx)
                                })),
                        )
                        .child(
                            Button::new("settings-save-close-button")
                                .primary()
                                .label("Save & Close")
                                .disabled(!settings_dirty)
                                .on_click(cx.listener(|this, _, _window, cx| {
                                    this.on_save_settings_clicked(cx)
                                })),
                        ),
                );
        }

        let provider_status_label = self.settings_ui_state.provider_status.label();
        let provider_status_color = self.settings_ui_state.provider_status.color(colors);
        let saved_default_model = self.settings_ui_state.saved().default_model.clone();
        let status_label = self.generation_status.label();
        let status_color = self.generation_status.color(colors);
        let generating = self.generation_status.is_submitting_or_running();
        let selected_mode_label = Self::generation_mode_label(self.selected_generation_mode);
        let selected_slot = self.selected_reference_slot;
        let selected_reference_slot_label = Self::reference_slot_label(selected_slot);
        let selected_reference_source = self.source_for_slot(selected_slot);
        let selected_reference_source_label =
            Self::reference_source_label(selected_reference_source);
        let selected_live_channel = self.channel_mapping_for_slot(selected_slot);
        let selected_slot_accepts_file_drop = selected_reference_source == ReferenceSource::File;
        let selected_live_recording_summary = self.live_recording_summary_for_slot(selected_slot);
        let file_references = self.load_midi_use_case.snapshot_references();
        let generation_references = self.collect_generation_references();
        let live_channel_mappings = self.input_track_model.live_channel_mappings();
        let mode_requirement = mode_reference_requirement(self.selected_generation_mode);
        let mode_requirement_satisfied = mode_reference_requirement_satisfied(
            self.selected_generation_mode,
            &generation_references,
        );
        let selected_slot_references: Vec<&_> = file_references
            .iter()
            .filter(|reference| reference.slot == selected_slot)
            .collect();
        let selected_slot_reference_count = selected_slot_references.len();
        let selected_slot_set = selected_slot_reference_count > 0;
        let selected_slot_error = self.midi_slot_error_for_slot(selected_slot).cloned();

        div()
            .size_full()
            .overflow_y_scrollbar()
            .overflow_x_hidden()
            .flex()
            .flex_col()
            .gap(spacing.section_gap)
            .p(spacing.window_padding)
            .bg(colors.surface_background)
            .text_color(colors.surface_foreground)
            .child(
                div()
                    .id("main-header")
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(spacing.section_gap)
                    .p(spacing.panel_padding)
                    .rounded(radius.panel)
                    .bg(colors.panel_background)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .w(px(28.0))
                                    .h(px(28.0))
                                    .rounded(radius.control)
                                    .border_1()
                                    .border_color(colors.panel_active_border)
                                    .bg(colors.panel_active_background)
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child("S"),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .child(Label::new("Sonant"))
                                    .child(
                                        div()
                                            .text_color(colors.muted_foreground)
                                            .child("FR-09 Main Interface"),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .id("api-status-badge")
                                    .px_2()
                                    .py_1()
                                    .rounded(radius.control)
                                    .border_1()
                                    .border_color(colors.panel_border)
                                    .bg(colors.surface_background)
                                    .text_color(provider_status_color)
                                    .child(provider_status_label),
                            )
                            .child(
                                Button::new("settings-button")
                                    .label("Settings")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.on_open_settings_clicked(window, cx)
                                    })),
                            ),
                    ),
            )
            .child(
                div()
                    .id("main-layout")
                    .flex()
                    .gap(spacing.section_gap)
                    .h_full()
                    .min_h(px(480.0))
                    .child(
                        div()
                            .id("left-sidebar")
                            .w(px(320.0))
                            .flex_none()
                            .flex()
                            .flex_col()
                            .gap(spacing.section_gap)
                            .overflow_y_scrollbar()
                            .child(
                                div()
                                    .id("prompt-mode-model-panel")
                                    .w_full()
                                    .flex()
                                    .flex_col()
                                    .gap_2()
                                    .p(spacing.panel_padding)
                                    .rounded(radius.panel)
                                    .border_1()
                                    .border_color(colors.panel_border)
                                    .bg(colors.panel_background)
                                    .child(Label::new("Prompt / Mode / Model"))
                                    .child(
                                        div()
                                            .text_color(colors.muted_foreground)
                                            .child("Prompt drives `on_generate_clicked` with the selected mode and collected references."),
                                    )
                                    .child(
                                        div().w_full().h(px(PROMPT_EDITOR_HEIGHT_PX)).child(
                                            Input::new(&self.prompt_input),
                                        ),
                                    )
                                    .children(self.validation_error.iter().map(|message| {
                                        div()
                                            .text_color(colors.error_foreground)
                                            .child(format!("Validation: {message}"))
                                    }))
                                    .child(Label::new("Generation Mode"))
                                    .child(
                                        div().w_full().h(px(36.0)).child(
                                            Select::new(&self.generation_mode_dropdown)
                                                .placeholder("Select generation mode"),
                                        ),
                                    )
                                    .child(
                                        div()
                                            .text_color(colors.accent_foreground)
                                            .child(format!("Selected Mode: {selected_mode_label}")),
                                    )
                                    .child(
                                        div()
                                            .text_color(colors.muted_foreground)
                                            .child(format!("Requirement: {}", mode_requirement.description)),
                                    )
                                    .children(
                                        std::iter::once(mode_requirement_satisfied)
                                            .filter(|ready| *ready)
                                            .map(|_| {
                                                div()
                                                    .text_color(colors.success_foreground)
                                                    .child("Reference requirement satisfied.")
                                            }),
                                    )
                                    .children(
                                        mode_requirement
                                            .unmet_message
                                            .iter()
                                            .filter(|_| !mode_requirement_satisfied)
                                            .map(|message| {
                                                div().text_color(colors.error_foreground).child(*message)
                                            }),
                                    )
                                    .child(
                                        div()
                                            .text_color(colors.accent_foreground)
                                            .child(format!("AI Model: {saved_default_model}")),
                                    )
                                    .child(Label::new("API Key (testing)"))
                                    .child(
                                        div().w_full().h(px(36.0)).child(
                                            Input::new(&self.api_key_input).mask_toggle(),
                                        ),
                                    )
                                    .children(self.api_key_error.iter().map(|message| {
                                        div()
                                            .text_color(colors.error_foreground)
                                            .child(format!("API Key: {message}"))
                                    })),
                            )
                            .child(
                                div()
                                    .id("input-tracks-panel")
                                    .w_full()
                                    .flex()
                                    .flex_col()
                                    .gap_2()
                                    .p(spacing.panel_padding)
                                    .rounded(radius.panel)
                                    .border_1()
                                    .border_color(colors.panel_border)
                                    .bg(colors.panel_background)
                                    .child(Label::new("Input Tracks"))
                                    .child(
                                        div()
                                            .text_color(colors.muted_foreground)
                                            .child("Select a slot, configure source/channel, and register references."),
                                    )
                                    .child(Label::new("Reference Slot"))
                                    .child(
                                        div().w_full().h(px(36.0)).child(
                                            Select::new(&self.reference_slot_dropdown)
                                                .placeholder("Select reference slot"),
                                        ),
                                    )
                                    .child(
                                        div()
                                            .text_color(colors.accent_foreground)
                                            .child(format!("Selected Slot: {selected_reference_slot_label}")),
                                    )
                                    .child(Label::new("Source"))
                                    .child(
                                        div().w_full().h(px(36.0)).child(
                                            Select::new(&self.reference_source_dropdown)
                                                .placeholder("Select source"),
                                        ),
                                    )
                                    .child(
                                        div()
                                            .text_color(colors.muted_foreground)
                                            .child(format!("Source: {selected_reference_source_label}")),
                                    )
                                    .children(
                                        std::iter::once(selected_reference_source)
                                            .filter(|source| *source == ReferenceSource::Live)
                                            .map(|_| {
                                                let live_channel_button_row = |start: u8, end: u8| {
                                                    div()
                                                        .flex()
                                                        .items_center()
                                                        .gap_1()
                                                        .children((start..=end).map(|channel| {
                                                            let disabled = live_channel_used_by_other_slots_in_mappings(
                                                                &live_channel_mappings,
                                                                selected_slot,
                                                                channel,
                                                            );
                                                            let button = Button::new(
                                                                Self::input_track_channel_button_id(
                                                                    selected_slot,
                                                                    channel,
                                                                ),
                                                            )
                                                            .label(if disabled {
                                                                format!("{channel}*")
                                                            } else {
                                                                channel.to_string()
                                                            })
                                                            .disabled(disabled)
                                                            .on_click(cx.listener(
                                                                move |this, _, _window, cx| {
                                                                    this.on_live_channel_selected(
                                                                        selected_slot,
                                                                        channel,
                                                                        cx,
                                                                    )
                                                                },
                                                            ));
                                                            if selected_live_channel == Some(channel) {
                                                                button.primary()
                                                            } else {
                                                                button
                                                            }
                                                        }))
                                                };
                                                div()
                                                    .flex()
                                                    .flex_col()
                                                    .gap_1()
                                                    .child(
                                                        div()
                                                            .text_color(colors.accent_foreground)
                                                            .child(format!(
                                                                "Live Channel: {}",
                                                                selected_live_channel
                                                                    .map(|channel| channel.to_string())
                                                                    .unwrap_or_else(|| "Not set".to_string())
                                                            )),
                                                    )
                                                    .child(
                                                        div()
                                                            .text_color(colors.accent_foreground)
                                                            .child(format!(
                                                                "Recorded Bars: {} / Notes: {} / Events: {}",
                                                                selected_live_recording_summary.bar_count,
                                                                selected_live_recording_summary.note_count,
                                                                selected_live_recording_summary.event_count
                                                            )),
                                                    )
                                                    .child(
                                                        div()
                                                            .text_color(colors.muted_foreground)
                                                            .child(format!(
                                                                "Pitch Range: {}",
                                                                match (
                                                                    selected_live_recording_summary.min_pitch,
                                                                    selected_live_recording_summary.max_pitch,
                                                                ) {
                                                                    (Some(min), Some(max)) => {
                                                                        format!("{min}..{max}")
                                                                    }
                                                                    _ => "N/A".to_string(),
                                                                }
                                                            )),
                                                    )
                                                    .child(live_channel_button_row(1, 8))
                                                    .child(live_channel_button_row(9, 16))
                                                    .child(
                                                        div()
                                                            .text_color(colors.muted_foreground)
                                                            .child("`*` indicates a channel already assigned to another Live slot."),
                                                    )
                                            }),
                                    )
                                    .child(
                                        div()
                                            .id("midi-slot-selected")
                                            .flex()
                                            .flex_col()
                                            .gap_2()
                                            .p(spacing.panel_compact_padding)
                                            .rounded(radius.control)
                                            .border_1()
                                            .border_color(colors.panel_border)
                                            .bg(colors.surface_background)
                                            .can_drop(move |value, _, _| {
                                                selected_slot_accepts_file_drop
                                                    && value
                                                        .downcast_ref::<ExternalPaths>()
                                                        .is_some_and(|paths| !paths.paths().is_empty())
                                            })
                                            .drag_over::<ExternalPaths>(move |style, paths, _, _| {
                                                if !selected_slot_accepts_file_drop {
                                                    style
                                                        .border_color(colors.panel_border)
                                                        .bg(colors.surface_background)
                                                } else if choose_dropped_midi_path(paths.paths()).is_some() {
                                                    style
                                                        .border_color(colors.panel_active_border)
                                                        .bg(colors.panel_active_background)
                                                } else {
                                                    style
                                                        .border_color(colors.drop_invalid_border)
                                                        .bg(colors.drop_invalid_background)
                                                }
                                            })
                                            .on_drop(cx.listener(
                                                |this, paths: &ExternalPaths, _window, cx| {
                                                    this.on_midi_slot_drop(paths, cx)
                                                },
                                            ))
                                            .child(if selected_slot_accepts_file_drop {
                                                div().child(format!(
                                                    "{MIDI_SLOT_DROP_HINT} Target: {selected_reference_slot_label}"
                                                ))
                                            } else {
                                                div().child(format!(
                                                    "{selected_reference_slot_label} is using Live input. Switch source to File to load MIDI files."
                                                ))
                                            })
                                            .child(div().child(format!(
                                                "Registered File MIDI: {selected_slot_reference_count}"
                                            )))
                                            .children(
                                                std::iter::once((
                                                    selected_slot_reference_count,
                                                    selected_reference_source,
                                                ))
                                                .filter(|(count, source)| {
                                                    *count == 0 && *source == ReferenceSource::File
                                                })
                                                .map(|_| {
                                                    div().text_color(colors.muted_foreground).child(format!(
                                                        "File: {MIDI_SLOT_EMPTY_LABEL}"
                                                    ))
                                                }),
                                            )
                                            .children(selected_slot_references.iter().enumerate().map(
                                                |(index, reference)| {
                                                    let slot_file_path = reference
                                                        .file
                                                        .as_ref()
                                                        .map(|file| file.path.clone())
                                                        .unwrap_or_else(|| {
                                                            MIDI_SLOT_EMPTY_LABEL.to_string()
                                                        });
                                                    let slot_file_label =
                                                        display_file_name_from_path(&slot_file_path);
                                                    let slot_stats = format!(
                                                        "Bars: {} / Notes: {}",
                                                        reference.bars, reference.note_count
                                                    );
                                                    div()
                                                        .flex()
                                                        .flex_col()
                                                        .gap_1()
                                                        .p(spacing.panel_compact_padding)
                                                        .rounded(radius.control)
                                                        .border_1()
                                                        .border_color(colors.panel_border)
                                                        .bg(colors.panel_background)
                                                        .child(
                                                            div()
                                                                .text_color(colors.accent_foreground)
                                                                .child(format!(
                                                                    "#{}: {slot_file_label}",
                                                                    index + 1
                                                                )),
                                                        )
                                                        .child(
                                                            div()
                                                                .text_color(colors.accent_foreground)
                                                                .child(slot_stats),
                                                        )
                                                        .child(
                                                            div()
                                                                .text_color(colors.muted_foreground)
                                                                .child(slot_file_path),
                                                        )
                                                },
                                            ))
                                            .child(
                                                div()
                                                    .flex()
                                                    .items_center()
                                                    .gap_2()
                                                    .child(
                                                        Button::new("midi-slot-select-button")
                                                            .label("Select MIDI File")
                                                            .disabled(!selected_slot_accepts_file_drop)
                                                            .on_click(cx.listener(
                                                                |this, _, window, cx| {
                                                                    this.on_select_midi_file_clicked(window, cx)
                                                                },
                                                            )),
                                                    )
                                                    .child(
                                                        Button::new("midi-slot-clear-button")
                                                            .label("Clear Slot")
                                                            .disabled(
                                                                !selected_slot_set
                                                                    || !selected_slot_accepts_file_drop,
                                                            )
                                                            .on_click(cx.listener(
                                                                |this, _, _window, cx| {
                                                                    this.on_clear_midi_slot_clicked(cx)
                                                                },
                                                            )),
                                                    ),
                                            ),
                                    )
                                    .children(self.input_track_error.iter().map(|message| {
                                        div()
                                            .text_color(colors.error_foreground)
                                            .child(format!("Input Tracks: {message}"))
                                    }))
                                    .children(selected_slot_error.into_iter().map(|error| {
                                        let slot_label = Self::reference_slot_label(error.slot);
                                        let retry_slot = error.slot;
                                        let slot_is_file_source =
                                            self.source_for_slot(error.slot) == ReferenceSource::File;
                                        div()
                                            .flex()
                                            .flex_col()
                                            .gap_2()
                                            .child(
                                                div().text_color(colors.error_foreground).child(format!(
                                                    "Reference MIDI ({slot_label}): {}",
                                                    error.message
                                                )),
                                            )
                                            .child(
                                                div()
                                                    .flex()
                                                    .items_center()
                                                    .gap_2()
                                                    .child(
                                                        Button::new("midi-slot-retry-button")
                                                            .label("Retry")
                                                            .disabled(
                                                                !error.can_retry() || !slot_is_file_source,
                                                            )
                                                            .on_click(cx.listener(
                                                                move |this, _, _window, cx| {
                                                                    this.on_retry_midi_slot_clicked(
                                                                        retry_slot,
                                                                        cx,
                                                                    )
                                                                },
                                                            )),
                                                    )
                                                    .child(
                                                        Button::new("midi-slot-reselect-button")
                                                            .label("Choose Another File")
                                                            .disabled(!slot_is_file_source)
                                                            .on_click(cx.listener(
                                                                |this, _, window, cx| {
                                                                    this.on_select_midi_file_clicked(window, cx)
                                                                },
                                                            )),
                                                    ),
                                            )
                                    }))
                                    .child(
                                        div()
                                            .id("recording-channel-panel")
                                            .flex()
                                            .flex_col()
                                            .gap_2()
                                            .p(spacing.panel_compact_padding)
                                            .rounded(radius.control)
                                            .border_1()
                                            .border_color(colors.panel_border)
                                            .bg(colors.surface_background)
                                            .child(
                                                div()
                                                    .text_color(colors.muted_foreground)
                                                    .child("Recording enable state per MIDI channel."),
                                            )
                                            .child({
                                                let recording_channel_button_row =
                                                    |start: u8, end: u8| {
                                                        div()
                                                            .flex()
                                                            .items_center()
                                                            .gap_1()
                                                            .children((start..=end).map(|channel| {
                                                                let enabled =
                                                                    self.recording_enabled_for_channel(channel);
                                                                let button = Button::new(
                                                                    Self::recording_channel_button_id(
                                                                        channel,
                                                                    ),
                                                                )
                                                                .label(if enabled {
                                                                    format!("Ch {channel} ON")
                                                                } else {
                                                                    format!("Ch {channel} OFF")
                                                                })
                                                                .on_click(cx.listener(
                                                                    move |this, _, _window, cx| {
                                                                        this.on_recording_channel_toggled(
                                                                            channel, cx,
                                                                        )
                                                                    },
                                                                ));
                                                                if enabled {
                                                                    button.primary()
                                                                } else {
                                                                    button
                                                                }
                                                            }))
                                                    };
                                                div()
                                                    .flex()
                                                    .flex_col()
                                                    .gap_1()
                                                    .child(recording_channel_button_row(1, 8))
                                                    .child(recording_channel_button_row(9, 16))
                                            }),
                                    ),
                            )
                            .child(
                                div()
                                    .id("generated-patterns-panel")
                                    .flex()
                                    .flex_col()
                                    .gap_2()
                                    .p(spacing.panel_padding)
                                    .rounded(radius.panel)
                                    .border_1()
                                    .border_color(colors.panel_border)
                                    .bg(colors.panel_background)
                                    .child(Label::new("Generated Patterns"))
                                    .child(
                                        div()
                                            .text_color(colors.warning_foreground)
                                            .child("Placeholder: candidate list / drag-to-DAW will be added in follow-up issues."),
                                    )
                                    .child(
                                        Button::new("generated-pattern-active")
                                            .label("Pattern 1 (placeholder)")
                                            .disabled(true),
                                    )
                                    .child(
                                        Button::new("generated-pattern-variation-a")
                                            .label("Pattern 2 (placeholder)")
                                            .disabled(true),
                                    )
                                    .child(
                                        Button::new("generated-pattern-variation-b")
                                            .label("Pattern 3 (placeholder)")
                                            .disabled(true),
                                    )
                                    .child(
                                        div()
                                            .text_color(colors.muted_foreground)
                                            .child("Unimplemented actions are intentionally disabled."),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .id("right-main")
                            .flex_1()
                            .flex()
                            .flex_col()
                            .gap(spacing.section_gap)
                            .overflow_hidden()
                            .child(
                                div()
                                    .id("params-panel")
                                    .flex()
                                    .flex_col()
                                    .gap_2()
                                    .p(spacing.panel_padding)
                                    .rounded(radius.panel)
                                    .border_1()
                                    .border_color(colors.panel_border)
                                    .bg(colors.panel_background)
                                    .child(Label::new("Params"))
                                    .child(
                                        div()
                                            .text_color(colors.warning_foreground)
                                            .child("Placeholder: Key / Scale / BPM / Complexity / Note Density are not editable yet."),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap_2()
                                            .child(Button::new("param-key").label("Key: D#").disabled(true))
                                            .child(
                                                Button::new("param-scale")
                                                    .label("Scale: Minor (Aeolian)")
                                                    .disabled(true),
                                            )
                                            .child(Button::new("param-bpm").label("BPM: 128").disabled(true)),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap_2()
                                            .child(
                                                Button::new("param-complexity")
                                                    .label("Complexity: 75%")
                                                    .disabled(true),
                                            )
                                            .child(
                                                Button::new("param-note-density")
                                                    .label("Note Density: 40%")
                                                    .disabled(true),
                                            ),
                                    ),
                            )
                            .child(
                                div()
                                    .id("piano-roll-panel")
                                    .flex_1()
                                    .min_h(px(260.0))
                                    .flex()
                                    .flex_col()
                                    .gap_2()
                                    .p(spacing.panel_padding)
                                    .rounded(radius.panel)
                                    .border_1()
                                    .border_color(colors.panel_border)
                                    .bg(colors.panel_background)
                                    .child(Label::new("Piano Roll"))
                                    .child(
                                        div()
                                            .text_color(colors.warning_foreground)
                                            .child("Placeholder: generated MIDI preview and playhead overlay will be implemented in follow-up issues."),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .rounded(radius.control)
                                            .border_1()
                                            .border_color(colors.panel_border)
                                            .bg(colors.surface_background)
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .text_color(colors.muted_foreground)
                                            .child("Piano Roll Placeholder"),
                                    ),
                            )
                            .child(
                                div()
                                    .id("main-footer")
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap(spacing.section_gap)
                                    .p(spacing.panel_padding)
                                    .rounded(radius.panel)
                                    .border_1()
                                    .border_color(colors.panel_border)
                                    .bg(colors.panel_background)
                                    .child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .gap_1()
                                            .child(div().text_color(status_color).child(status_label))
                                            .children(self.startup_notice.iter().map(|notice| {
                                                div()
                                                    .text_color(colors.muted_foreground)
                                                    .child(format!("Backend: {notice}"))
                                            })),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap_2()
                                            .child(
                                                Button::new("apply-to-daw-button")
                                                    .label("Apply to DAW")
                                                    .disabled(true),
                                            )
                                            .child(
                                                Button::new("generate-button")
                                                    .primary()
                                                    .label(if generating {
                                                        "Generating..."
                                                    } else {
                                                        "Generate"
                                                    })
                                                    .loading(generating)
                                                    .disabled(generating || !mode_requirement_satisfied)
                                                    .on_click(cx.listener(|this, _, window, cx| {
                                                        this.on_generate_clicked(window, cx)
                                                    })),
                                            ),
                                    ),
                            ),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::{
        build_live_reference_summary, collect_live_references,
        first_available_live_channel_for_slot, first_available_live_channel_for_slot_in_model,
        live_channel_used_by_other_slots, midi_channel_from_status,
        preferred_live_channel_for_slot, recording_enabled_for_channel_array,
        resolve_live_channel_mapping_for_slot, summarize_live_recording,
    };
    use sonant::app::{ChannelMapping, InputTrackModel, LiveInputEvent, MidiInputRouter};
    use sonant::domain::{
        GenerationMode, GenerationParams, GenerationRequest, ModelRef, ReferenceSlot,
        ReferenceSource,
    };

    #[test]
    fn used_channel_is_excluded_only_for_other_live_slots() {
        let mut model = InputTrackModel::new();
        model
            .set_source_for_slot(ReferenceSlot::Melody, ReferenceSource::Live)
            .expect("melody should switch to live");
        model
            .set_source_for_slot(ReferenceSlot::ChordProgression, ReferenceSource::Live)
            .expect("chord should switch to live");

        assert!(!live_channel_used_by_other_slots(
            &model,
            ReferenceSlot::Melody,
            1
        ));
        assert!(live_channel_used_by_other_slots(
            &model,
            ReferenceSlot::Melody,
            2
        ));
    }

    #[test]
    fn first_available_live_channel_skips_channels_used_by_live_slots() {
        let mut model = InputTrackModel::new();
        model
            .set_source_for_slot(ReferenceSlot::Melody, ReferenceSource::Live)
            .expect("melody should switch to live");
        model
            .set_source_for_slot(ReferenceSlot::ChordProgression, ReferenceSource::Live)
            .expect("chord should switch to live");

        assert_eq!(
            first_available_live_channel_for_slot_in_model(&model, ReferenceSlot::CounterMelody),
            Some(3)
        );
    }

    #[test]
    fn conflicting_preferred_channel_returns_none_for_recovery() {
        let mut model = InputTrackModel::new();
        model
            .set_channel_mapping(ChannelMapping {
                slot: ReferenceSlot::Melody,
                channel: 3,
            })
            .expect("channel update should succeed");
        model
            .set_source_for_slot(ReferenceSlot::Melody, ReferenceSource::Live)
            .expect("melody should switch to live");

        assert_eq!(
            preferred_live_channel_for_slot(&model, ReferenceSlot::Bassline),
            None
        );
        assert_eq!(
            first_available_live_channel_for_slot_in_model(&model, ReferenceSlot::Bassline),
            Some(1)
        );
    }

    #[test]
    fn first_available_live_channel_returns_none_when_all_channels_are_occupied() {
        // Synthetic edge case to validate behavior if all MIDI channels are occupied.
        let occupied_channels: Vec<ChannelMapping> = (1..=16)
            .map(|channel| ChannelMapping {
                slot: ReferenceSlot::Melody,
                channel,
            })
            .collect();

        assert_eq!(
            first_available_live_channel_for_slot(ReferenceSlot::Bassline, &occupied_channels),
            None
        );
    }

    #[test]
    fn resolve_live_channel_mapping_returns_error_when_no_channels_are_available() {
        let occupied_channels: Vec<ChannelMapping> = (1..=16)
            .map(|channel| ChannelMapping {
                slot: ReferenceSlot::Melody,
                channel,
            })
            .collect();

        assert_eq!(
            resolve_live_channel_mapping_for_slot(
                ReferenceSlot::Bassline,
                None,
                &occupied_channels
            ),
            Err("No free MIDI channel is available for Bassline.".to_string())
        );
    }

    #[test]
    fn summarize_live_recording_counts_note_events_and_pitch_range() {
        let events = vec![
            LiveInputEvent {
                time: 0,
                port_index: 0,
                data: [0x90, 60, 96],
                is_transport_playing: true,
                playhead_ppq: 0.0,
            },
            LiveInputEvent {
                time: 1,
                port_index: 0,
                data: [0x90, 72, 100],
                is_transport_playing: true,
                playhead_ppq: 0.0,
            },
            LiveInputEvent {
                time: 2,
                port_index: 0,
                data: [0x80, 60, 0],
                is_transport_playing: true,
                playhead_ppq: 0.0,
            },
        ];

        let summary = summarize_live_recording(&events, 2);
        assert_eq!(summary.bar_count, 2);
        assert_eq!(summary.event_count, 3);
        assert_eq!(summary.note_count, 2);
        assert_eq!(summary.min_pitch, Some(60));
        assert_eq!(summary.max_pitch, Some(72));
    }

    #[test]
    fn build_live_reference_summary_creates_valid_live_reference() {
        let events = vec![
            LiveInputEvent {
                time: 0,
                port_index: 0,
                data: [0x90, 60, 96],
                is_transport_playing: true,
                playhead_ppq: 0.0,
            },
            LiveInputEvent {
                time: 6,
                port_index: 0,
                data: [0x90, 67, 100],
                is_transport_playing: true,
                playhead_ppq: 0.0,
            },
            LiveInputEvent {
                time: 2,
                port_index: 0,
                data: [0x80, 60, 0],
                is_transport_playing: true,
                playhead_ppq: 0.0,
            },
        ];

        let reference = build_live_reference_summary(ReferenceSlot::Melody, &events, 2)
            .expect("live reference should be built");
        assert_eq!(reference.slot, ReferenceSlot::Melody);
        assert_eq!(reference.source, ReferenceSource::Live);
        assert_eq!(reference.file, None);
        assert_eq!(reference.bars, 2);
        assert_eq!(reference.note_count, 2);
        assert_eq!(reference.min_pitch, 60);
        assert_eq!(reference.max_pitch, 67);
        assert_eq!(reference.events.len(), 3);
        assert!(
            reference
                .events
                .iter()
                .all(|event| !event.event.trim().is_empty() && event.event.contains("LiveMidi"))
        );
        assert!(reference.validate().is_ok());
    }

    #[test]
    fn build_live_reference_summary_returns_none_without_note_on_events() {
        let events = vec![LiveInputEvent {
            time: 0,
            port_index: 0,
            data: [0x80, 60, 0],
            is_transport_playing: true,
            playhead_ppq: 0.0,
        }];
        assert!(build_live_reference_summary(ReferenceSlot::Melody, &events, 1).is_none());
    }

    #[test]
    fn collect_live_references_excludes_recording_disabled_channels() {
        let mut model = InputTrackModel::new();
        model
            .set_source_for_slot(ReferenceSlot::Melody, ReferenceSource::Live)
            .expect("melody should switch to live");
        model
            .set_source_for_slot(ReferenceSlot::ChordProgression, ReferenceSource::Live)
            .expect("chord should switch to live");

        let router = MidiInputRouter::new();
        router
            .update_channel_mapping(model.live_channel_mappings())
            .expect("live channel mapping should be valid");
        router
            .set_recording_channel_enabled(1, true)
            .expect("channel 1 should be valid");
        router
            .set_recording_channel_enabled(2, true)
            .expect("channel 2 should be valid");
        router.update_transport_state(true, 0.0);
        router.push_live_event(
            1,
            LiveInputEvent {
                time: 0,
                port_index: 0,
                data: [0x90, 60, 96],
                is_transport_playing: true,
                playhead_ppq: 0.0,
            },
        );
        router.push_live_event(
            2,
            LiveInputEvent {
                time: 0,
                port_index: 0,
                data: [0x91, 64, 96],
                is_transport_playing: true,
                playhead_ppq: 0.0,
            },
        );

        let mut recording_channel_enabled = [false; 16];
        recording_channel_enabled[0] = true;
        recording_channel_enabled[1] = false;

        let references = collect_live_references(&model, &recording_channel_enabled, &router);
        assert_eq!(references.len(), 1);
        assert_eq!(references[0].slot, ReferenceSlot::Melody);
        assert_eq!(references[0].source, ReferenceSource::Live);
    }

    #[test]
    fn live_reference_allows_generation_request_validation() {
        let reference = build_live_reference_summary(
            ReferenceSlot::Melody,
            &[LiveInputEvent {
                time: 0,
                port_index: 0,
                data: [0x90, 60, 100],
                is_transport_playing: true,
                playhead_ppq: 0.0,
            }],
            1,
        )
        .expect("live reference should be built");

        let request = GenerationRequest {
            request_id: "req-live-1".to_string(),
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
                max_tokens: Some(256),
            },
            references: vec![reference],
            variation_count: 1,
        };

        assert!(request.validate().is_ok());
    }

    #[test]
    fn recording_enabled_for_channel_array_checks_bounds() {
        let mut channels = [false; 16];
        channels[0] = true;
        assert!(recording_enabled_for_channel_array(&channels, 1));
        assert!(!recording_enabled_for_channel_array(&channels, 16));
        assert!(!recording_enabled_for_channel_array(&channels, 0));
        assert!(!recording_enabled_for_channel_array(&channels, 17));
    }

    #[test]
    fn midi_channel_from_status_maps_channel_voice_messages() {
        assert_eq!(midi_channel_from_status(0x90), Some(1));
        assert_eq!(midi_channel_from_status(0x9F), Some(16));
        assert_eq!(midi_channel_from_status(0x80), Some(1));
        assert_eq!(midi_channel_from_status(0xF8), None);
        assert_eq!(midi_channel_from_status(0x20), None);
    }
}
