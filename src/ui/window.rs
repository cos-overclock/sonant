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
        GenerationMode, LlmError, MidiReferenceEvent, MidiReferenceSummary, ModelRef,
        ReferenceSlot, ReferenceSource, calculate_reference_density_hint,
        has_supported_midi_extension,
    },
};

use super::backend::build_generation_backend;
use super::request::PromptSubmissionModel;
use super::state::{
    HelperGenerationStatus, MidiSlotErrorState, SettingsDraftState, SettingsField, SettingsTab,
    SettingsUiState, mode_reference_requirement, mode_reference_requirement_satisfied,
};
use super::theme::{SonantTheme, ThemeColors};
use super::utils::{
    choose_dropped_midi_path, display_file_name_from_path, dropped_path_to_load,
    log_generation_request_submission,
};
use super::{
    DEFAULT_ANTHROPIC_MODEL, DEFAULT_OPENAI_COMPAT_MODEL, JOB_UPDATE_POLL_INTERVAL_MS,
    MIDI_SLOT_DROP_ERROR_MESSAGE, MIDI_SLOT_FILE_PICKER_PROMPT, MIDI_SLOT_UNSUPPORTED_FILE_MESSAGE,
    PROMPT_EDITOR_ROWS, PROMPT_PLACEHOLDER, PROMPT_VALIDATION_MESSAGE,
    SETTINGS_ANTHROPIC_API_KEY_PLACEHOLDER, SETTINGS_CONTEXT_WINDOW_PLACEHOLDER,
    SETTINGS_CUSTOM_BASE_URL_PLACEHOLDER, SETTINGS_DEFAULT_MODEL_PLACEHOLDER,
    SETTINGS_OPENAI_API_KEY_PLACEHOLDER,
};

const LIVE_CAPTURE_POLL_INTERVAL_MS: u64 = 30;
const LIVE_CAPTURE_MAX_EVENTS_PER_POLL: usize = 512;
type DropdownState = SelectState<Vec<&'static str>>;

pub(super) struct SonantMainWindow {
    prompt_input: Entity<InputState>,
    _prompt_input_subscription: Subscription,
    generation_mode_dropdown: Entity<DropdownState>,
    _generation_mode_dropdown_subscription: Subscription,
    ai_model_dropdown: Entity<DropdownState>,
    _ai_model_dropdown_subscription: Subscription,
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
    slot_visibility: [bool; 7],
    generation_status: HelperGenerationStatus,
    validation_error: Option<String>,
    input_track_error: Option<String>,
    midi_slot_errors: Vec<MidiSlotErrorState>,
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
        let ai_model_dropdown =
            cx.new(|cx| SelectState::new(Self::ai_model_dropdown_items(), None, window, cx));
        let ai_model_dropdown_subscription =
            cx.subscribe_in(&ai_model_dropdown, window, Self::on_ai_model_dropdown_event);
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
            ai_model_dropdown,
            _ai_model_dropdown_subscription: ai_model_dropdown_subscription,
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
            slot_visibility: [true; 7],
            generation_status: HelperGenerationStatus::Idle,
            validation_error: None,
            input_track_error: live_input_error,
            midi_slot_errors: Vec::new(),
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

    fn sync_dropdowns(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mode_label = Self::generation_mode_label(self.selected_generation_mode);
        self.generation_mode_dropdown.update(cx, |state, cx| {
            state.set_selected_value(&mode_label, window, cx);
        });

        let model_id = self.settings_ui_state.saved().default_model.as_str();
        let model_label = Self::ai_model_dropdown_items()
            .into_iter()
            .find(|item| *item == model_id);
        if let Some(label) = model_label {
            self.ai_model_dropdown.update(cx, |state, cx| {
                state.set_selected_value(&label, window, cx);
            });
        }
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

    fn on_ai_model_dropdown_event(
        &mut self,
        _state: &Entity<DropdownState>,
        event: &SelectEvent<Vec<&'static str>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let SelectEvent::Confirm(selected) = event;
        let Some(selected) = selected.as_deref() else {
            return;
        };
        let provider = if selected == DEFAULT_ANTHROPIC_MODEL {
            "anthropic"
        } else {
            "openai_compatible"
        };
        let model_ref = ModelRef {
            provider: provider.to_string(),
            model: selected.to_string(),
        };
        self.submission_model.set_model(model_ref);
        self.settings_ui_state
            .update_draft_field(SettingsField::DefaultModel, selected);
        cx.notify();
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
                ReferenceSlot::Melody,
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

    fn section_label(text: &str, colors: ThemeColors) -> impl IntoElement {
        div()
            .text_size(px(12.0))
            .font_weight(gpui::FontWeight::BOLD)
            .text_color(colors.muted_foreground)
            .child(text.to_uppercase())
    }

    fn section_label_with_info(text: &str, colors: ThemeColors) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .justify_between()
            .child(Self::section_label(text, colors))
            .child(
                div()
                    .text_size(px(14.0))
                    .text_color(colors.muted_foreground)
                    .cursor_pointer()
                    .hover(|style| style.text_color(colors.primary))
                    .child("ⓘ"),
            )
    }

    fn ai_model_dropdown_items() -> Vec<&'static str> {
        vec![DEFAULT_ANTHROPIC_MODEL, DEFAULT_OPENAI_COMPAT_MODEL]
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

    fn slot_short_label(slot: ReferenceSlot) -> &'static str {
        match slot {
            ReferenceSlot::Melody => "Melody",
            ReferenceSlot::ChordProgression => "Chords",
            ReferenceSlot::DrumPattern => "Drums",
            ReferenceSlot::Bassline => "Bass",
            ReferenceSlot::CounterMelody => "Counter",
            ReferenceSlot::Harmony => "Harmony",
            ReferenceSlot::ContinuationSeed => "Seed",
        }
    }

    fn slot_source_display_label(&self, slot: ReferenceSlot) -> String {
        match self.source_for_slot(slot) {
            ReferenceSource::Live => {
                let ch = self.channel_mapping_for_slot(slot).unwrap_or(1);
                format!("Live: CH {ch}")
            }
            ReferenceSource::File => {
                let file_references = self.load_midi_use_case.snapshot_references();
                let has_file = file_references.iter().any(|r| r.slot == slot);
                if has_file {
                    file_references
                        .iter()
                        .find(|r| r.slot == slot)
                        .and_then(|r| r.file.as_ref())
                        .map(|f| display_file_name_from_path(&f.path))
                        .unwrap_or_else(|| "File".to_string())
                } else {
                    "Drop MIDI file".to_string()
                }
            }
        }
    }

    fn on_slot_visibility_toggled(&mut self, slot: ReferenceSlot, cx: &mut Context<Self>) {
        let index = Self::reference_slot_index(slot);
        self.slot_visibility[index] = !self.slot_visibility[index];
        cx.notify();
    }

    fn slot_is_visible(&self, slot: ReferenceSlot) -> bool {
        self.slot_visibility[Self::reference_slot_index(slot)]
    }

    fn on_slot_source_toggled(&mut self, slot: ReferenceSlot, cx: &mut Context<Self>) {
        let current = self.source_for_slot(slot);
        let next = match current {
            ReferenceSource::File => ReferenceSource::Live,
            ReferenceSource::Live => ReferenceSource::File,
        };
        self.on_reference_source_selected(slot, next, cx);
    }

    fn settings_tab_button_id(tab: SettingsTab) -> &'static str {
        match tab {
            SettingsTab::ApiKeys => "settings-tab-api-keys",
            SettingsTab::MidiSettings => "settings-tab-midi-settings",
            SettingsTab::General => "settings-tab-general",
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

    #[allow(dead_code)]
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

    fn on_select_midi_file_clicked(
        &mut self,
        slot: ReferenceSlot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.source_for_slot(slot) != ReferenceSource::File {
            self.input_track_error = Some(format!(
                "{} is set to Live input. Switch source to File to load MIDI files.",
                Self::reference_slot_label(slot)
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
                        view.upsert_midi_slot_error(MidiSlotErrorState::non_retryable(
                            slot, message,
                        ));
                        cx.notify();
                    });
                }
            }
        });
    }

    fn on_midi_slot_drop(
        &mut self,
        slot: ReferenceSlot,
        paths: &ExternalPaths,
        cx: &mut Context<Self>,
    ) {
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

    fn on_clear_midi_slot_clicked(&mut self, slot: ReferenceSlot, cx: &mut Context<Self>) {
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
                        .child(Label::new("Settings"))
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
                        .child(Label::new("MIDI Settings")),
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
        let status_label = self.generation_status.label();
        let status_color = self.generation_status.color(colors);
        let generating = self.generation_status.is_submitting_or_running();
        let generation_references = self.collect_generation_references();
        let mode_requirement = mode_reference_requirement(self.selected_generation_mode);
        let mode_requirement_satisfied = mode_reference_requirement_satisfied(
            self.selected_generation_mode,
            &generation_references,
        );

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
                                    .bg(colors.primary)
                                    .shadow(vec![gpui::BoxShadow {
                                        color: colors.glow_primary,
                                        offset: gpui::point(px(0.0), px(0.0)),
                                        blur_radius: px(8.0),
                                        spread_radius: px(0.0),
                                    }])
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .text_color(gpui::white())
                                    .font_weight(gpui::FontWeight::BOLD)
                                    .child("S"),
                            )
                            .child(Label::new("Sonant"))
                            .child(
                                div()
                                    .px_2()
                                    .py(px(2.0))
                                    .rounded(px(999.0))
                                    .bg(colors.input_background)
                                    .text_color(colors.muted_foreground)
                                    .text_size(px(10.0))
                                    .child(concat!("v", env!("CARGO_PKG_VERSION"))),
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
                                    .flex()
                                    .items_center()
                                    .gap(px(6.0))
                                    .px_3()
                                    .py(px(4.0))
                                    .rounded(px(999.0))
                                    .border_1()
                                    .border_color(colors.panel_border)
                                    .bg(colors.surface_background)
                                    .text_color(provider_status_color)
                                    .child(
                                        div()
                                            .w(px(8.0))
                                            .h(px(8.0))
                                            .rounded(px(999.0))
                                            .bg(provider_status_color),
                                    )
                                    .child(provider_status_label),
                            )
                            .child(
                                div()
                                    .id("settings-button")
                                    .px_2()
                                    .py_1()
                                    .rounded(radius.control)
                                    .text_size(px(20.0))
                                    .text_color(colors.muted_foreground)
                                    .cursor_pointer()
                                    .hover(|style| {
                                        style
                                            .text_color(colors.surface_foreground)
                                            .bg(colors.input_background)
                                    })
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.on_open_settings_clicked(window, cx)
                                    }))
                                    .child("⚙"),
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
                            .p(spacing.panel_padding)
                            .gap(spacing.panel_padding)
                            .overflow_y_scrollbar()
                            .child(
                                div()
                                    .id("prompt-section")
                                    .w_full()
                                    .flex()
                                    .flex_col()
                                    .gap_2()
                                    .child(Self::section_label_with_info("Prompt", colors))
                                    .child(
                                        div()
                                            .w_full()
                                            .min_h(px(96.0))
                                            .flex()
                                            .flex_col()
                                            .child(Input::new(&self.prompt_input).h_full()),
                                    )
                                    .children(self.validation_error.iter().map(|message| {
                                        div()
                                            .text_color(colors.error_foreground)
                                            .child(format!("Validation: {message}"))
                                    })),
                            )
                            .child(
                                div()
                                    .id("generation-mode-section")
                                    .w_full()
                                    .flex()
                                    .flex_col()
                                    .gap_2()
                                    .pt(spacing.panel_padding)
                                    .border_t_1()
                                    .border_color(colors.panel_border)
                                    .child(Self::section_label("Generation Mode", colors))
                                    .child(
                                        div().w_full().h(px(36.0)).child(
                                            Select::new(&self.generation_mode_dropdown)
                                                .placeholder("Select generation mode"),
                                        ),
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
                                    ),
                            )
                            .child(
                                div()
                                    .id("ai-model-section")
                                    .w_full()
                                    .flex()
                                    .flex_col()
                                    .gap_2()
                                    .pt(spacing.panel_padding)
                                    .border_t_1()
                                    .border_color(colors.panel_border)
                                    .child(Self::section_label("AI Model", colors))
                                    .child(
                                        div().w_full().h(px(36.0)).child(
                                            Select::new(&self.ai_model_dropdown)
                                                .placeholder("Select AI model"),
                                        ),
                                    ),
                            )
                            .child(
                                div()
                                    .id("input-tracks-section")
                                    .w_full()
                                    .flex()
                                    .flex_col()
                                    .gap_2()
                                    .pt(spacing.panel_padding)
                                    .border_t_1()
                                    .border_color(colors.panel_border)
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .justify_between()
                                            .child(Self::section_label("Input Tracks", colors))
                                            .child(
                                                div()
                                                    .id("add-track-btn-header")
                                                    .px_1()
                                                    .py(px(2.0))
                                                    .rounded(radius.control)
                                                    .text_size(px(11.0))
                                                    .text_color(colors.muted_foreground)
                                                    .cursor_pointer()
                                                    .hover(|s| s.text_color(colors.primary).bg(colors.input_background))
                                                    .child("+ Add"),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .id("input-tracks-list")
                                            .rounded(radius.control)
                                            .border_1()
                                            .border_color(colors.panel_border)
                                            .bg(colors.input_background)
                                            .overflow_hidden()
                                            .child(
                                                div()
                                                    .id("input-tracks-column-header")
                                                    .flex()
                                                    .items_center()
                                                    .justify_between()
                                                    .px_3()
                                                    .py(px(6.0))
                                                    .border_b_1()
                                                    .border_color(colors.panel_border)
                                                    .bg(colors.panel_background)
                                                    .child(
                                                        div()
                                                            .text_size(px(10.0))
                                                            .text_color(colors.muted_foreground)
                                                            .font_weight(gpui::FontWeight::BOLD)
                                                            .child("Source"),
                                                    )
                                                    .child(
                                                        div()
                                                            .pr(px(24.0))
                                                            .text_size(px(10.0))
                                                            .text_color(colors.muted_foreground)
                                                            .font_weight(gpui::FontWeight::BOLD)
                                                            .child("Type"),
                                                    ),
                                            )
                                            .children(Self::reference_slots().iter().copied().map(|slot| {
                                                let slot_color = colors.slot_color(slot);
                                                let source_label = self.slot_source_display_label(slot);
                                                let short_label = Self::slot_short_label(slot);
                                                let is_live = self.source_for_slot(slot) == ReferenceSource::Live;
                                                let live_ch = self.channel_mapping_for_slot(slot).unwrap_or(1);
                                                let monitoring_on = is_live && self.recording_enabled_for_channel(live_ch);
                                                let is_visible = self.slot_is_visible(slot);
                                                let slot_error = self.midi_slot_error_for_slot(slot).cloned();
                                                let slot_has_file = {
                                                    let refs = self.load_midi_use_case.snapshot_references();
                                                    refs.iter().any(|r| r.slot == slot)
                                                };

                                                div()
                                                    .id(("track-row", Self::reference_slot_index(slot)))
                                                    .flex()
                                                    .items_center()
                                                    .h(px(40.0))
                                                    .bg(colors.panel_background)
                                                    .hover(|s| s.bg(colors.input_background))
                                                    .can_drop(move |value, _, _| {
                                                        !is_live
                                                            && value
                                                                .downcast_ref::<ExternalPaths>()
                                                                .is_some_and(|paths| !paths.paths().is_empty())
                                                    })
                                                    .drag_over::<ExternalPaths>(move |style, paths, _, _| {
                                                        if is_live {
                                                            style
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
                                                        move |this, paths: &ExternalPaths, _window, cx| {
                                                            this.on_midi_slot_drop(slot, paths, cx)
                                                        },
                                                    ))
                                                    // Color stripe
                                                    .child(
                                                        div()
                                                            .w(px(6.0))
                                                            .h_full()
                                                            .flex_none()
                                                            .bg(slot_color),
                                                    )
                                                    // Source label (clickable to toggle source)
                                                    .child(
                                                        div()
                                                            .flex_1()
                                                            .flex()
                                                            .items_center()
                                                            .gap_2()
                                                            .px_2()
                                                            .min_w(px(0.0))
                                                            .child(
                                                                div()
                                                                    .id(("slot-source-label", Self::reference_slot_index(slot)))
                                                                    .flex_1()
                                                                    .min_w(px(0.0))
                                                                    .overflow_hidden()
                                                                    .text_size(px(11.0))
                                                                    .text_color(colors.surface_foreground)
                                                                    .cursor_pointer()
                                                                    .hover(|s| s.text_color(colors.primary))
                                                                    .on_click(cx.listener(move |this, _, window, cx| {
                                                                        this.on_select_midi_file_clicked(slot, window, cx);
                                                                    }))
                                                                    .child(source_label),
                                                            )
                                                            // Type badge
                                                            .child(
                                                                div()
                                                                    .flex_none()
                                                                    .px(px(6.0))
                                                                    .py(px(2.0))
                                                                    .rounded(px(4.0))
                                                                    .text_size(px(9.0))
                                                                    .text_color(slot_color)
                                                                    .font_weight(gpui::FontWeight::BOLD)
                                                                    .border_1()
                                                                    .border_color(slot_color)
                                                                    .child(short_label),
                                                            ),
                                                    )
                                                    // Action buttons
                                                    .child(
                                                        div()
                                                            .flex()
                                                            .items_center()
                                                            .gap_1()
                                                            .pr_2()
                                                            .pl_2()
                                                            .h(px(24.0))
                                                            .border_l_1()
                                                            .border_color(colors.panel_border)
                                                            // Source toggle button
                                                            .child(
                                                                div()
                                                                    .id(("slot-source-toggle", Self::reference_slot_index(slot)))
                                                                    .px(px(4.0))
                                                                    .py(px(2.0))
                                                                    .rounded(px(3.0))
                                                                    .text_size(px(9.0))
                                                                    .text_color(if is_live { colors.primary } else { colors.muted_foreground })
                                                                    .font_weight(gpui::FontWeight::BOLD)
                                                                    .cursor_pointer()
                                                                    .hover(|s| s.text_color(colors.surface_foreground).bg(colors.input_background))
                                                                    .on_click(cx.listener(move |this, _, _window, cx| {
                                                                        this.on_slot_source_toggled(slot, cx);
                                                                    }))
                                                                    .child(if is_live { "LIVE" } else { "FILE" }),
                                                            )
                                                            // Monitoring button
                                                            .child(
                                                                div()
                                                                    .id(("slot-monitor", Self::reference_slot_index(slot)))
                                                                    .w(px(20.0))
                                                                    .h(px(20.0))
                                                                    .flex()
                                                                    .items_center()
                                                                    .justify_center()
                                                                    .rounded(px(999.0))
                                                                    .text_size(px(12.0))
                                                                    .text_color(if monitoring_on { colors.error_foreground } else { colors.muted_foreground })
                                                                    .cursor_pointer()
                                                                    .hover(|s| s.text_color(colors.surface_foreground))
                                                                    .on_click(cx.listener(move |this, _, _window, cx| {
                                                                        if is_live {
                                                                            this.on_recording_channel_toggled(live_ch, cx);
                                                                        }
                                                                    }))
                                                                    .child("●"),
                                                            )
                                                            // Visibility toggle
                                                            .child(
                                                                div()
                                                                    .id(("slot-visibility", Self::reference_slot_index(slot)))
                                                                    .w(px(20.0))
                                                                    .h(px(20.0))
                                                                    .flex()
                                                                    .items_center()
                                                                    .justify_center()
                                                                    .rounded(px(999.0))
                                                                    .text_size(px(12.0))
                                                                    .text_color(if is_visible { colors.muted_foreground } else { colors.panel_border })
                                                                    .cursor_pointer()
                                                                    .hover(|s| s.text_color(colors.surface_foreground))
                                                                    .on_click(cx.listener(move |this, _, _window, cx| {
                                                                        this.on_slot_visibility_toggled(slot, cx);
                                                                    }))
                                                                    .child(if is_visible { "O" } else { "-" }),
                                                            )
                                                            // Clear file button (only for file source with a file loaded)
                                                            .when(!is_live && slot_has_file, |el| {
                                                                el.child(
                                                                    div()
                                                                        .id(("slot-clear", Self::reference_slot_index(slot)))
                                                                        .w(px(20.0))
                                                                        .h(px(20.0))
                                                                        .flex()
                                                                        .items_center()
                                                                        .justify_center()
                                                                        .rounded(px(999.0))
                                                                        .text_size(px(12.0))
                                                                        .text_color(colors.muted_foreground)
                                                                        .cursor_pointer()
                                                                        .hover(|s| s.text_color(colors.error_foreground))
                                                                        .on_click(cx.listener(move |this, _, _window, cx| {
                                                                            this.on_clear_midi_slot_clicked(slot, cx);
                                                                        }))
                                                                        .child("x"),
                                                                )
                                                            }),
                                                    )
                                                    // Error indicator
                                                    .children(slot_error.into_iter().map(|error| {
                                                        let retry_slot = error.slot;
                                                        let can_retry = error.can_retry();
                                                        div()
                                                            .id(("slot-error", Self::reference_slot_index(retry_slot)))
                                                            .absolute()
                                                            .bottom(px(0.0))
                                                            .left(px(6.0))
                                                            .right(px(0.0))
                                                            .text_size(px(9.0))
                                                            .text_color(colors.error_foreground)
                                                            .overflow_hidden()
                                                            .child(format!("Error: {}", error.message))
                                                            .when(can_retry, |el| {
                                                                el.cursor_pointer()
                                                                    .on_click(cx.listener(move |this, _, _window, cx| {
                                                                        this.on_retry_midi_slot_clicked(retry_slot, cx);
                                                                    }))
                                                            })
                                                    }))
                                            }))
                                            .child(
                                                div()
                                                    .id("add-track-footer-btn")
                                                    .w_full()
                                                    .py(px(6.0))
                                                    .flex()
                                                    .items_center()
                                                    .justify_center()
                                                    .gap_1()
                                                    .border_t_1()
                                                    .border_color(colors.panel_border)
                                                    .text_size(px(10.0))
                                                    .text_color(colors.muted_foreground)
                                                    .font_weight(gpui::FontWeight::BOLD)
                                                    .cursor_pointer()
                                                    .hover(|s| s.text_color(colors.primary).bg(colors.panel_active_background))
                                                    .child("+ Add Track"),
                                            ),
                                    )
                                    .children(self.input_track_error.iter().map(|message| {
                                        div()
                                            .text_color(colors.error_foreground)
                                            .text_size(px(11.0))
                                            .child(format!("Input Tracks: {message}"))
                                    })),
                            )
                            .child(
                                div()
                                    .id("generated-patterns-section")
                                    .flex()
                                    .flex_col()
                                    .gap_2()
                                    .pt(spacing.panel_padding)
                                    .border_t_1()
                                    .border_color(colors.panel_border)
                                    .child(Self::section_label("Generated Patterns", colors))
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
                                    ),
                            )
                            .child(
                                div()
                                    .id("parameter-sliders-section")
                                    .flex()
                                    .flex_col()
                                    .gap_2()
                                    .pt(spacing.panel_padding)
                                    .border_t_1()
                                    .border_color(colors.panel_border)
                                    .child(Self::section_label("Parameter Sliders", colors))
                                    .child(
                                        div()
                                            .text_color(colors.muted_foreground)
                                            .child("Placeholder: Complexity and Note Density sliders will be added in follow-up issues."),
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
