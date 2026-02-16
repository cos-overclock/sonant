use std::sync::Arc;
use std::time::Duration;

use gpui::{
    App, Context, Entity, ExternalPaths, IntoElement, PathPromptOptions, Render, Subscription,
    Task, Timer, Window, div, prelude::*, px, rgb,
};
use gpui_component::{
    Disableable,
    button::{Button, ButtonVariants as _},
    input::{Input, InputEvent, InputState},
    label::Label,
    scroll::ScrollableElement,
};
use sonant::{
    app::{
        ChannelMapping, GenerationJobManager, GenerationJobState, GenerationJobUpdate,
        InputTrackModel, LiveInputEvent, LiveInputEventSource, LiveMidiCapture, LoadMidiCommand,
        LoadMidiUseCase, MidiInputRouter,
    },
    domain::{
        GenerationMode, LlmError, ReferenceSlot, ReferenceSource, has_supported_midi_extension,
    },
};

use super::backend::{
    GenerationBackend, build_generation_backend, build_generation_backend_from_api_key,
};
use super::request::PromptSubmissionModel;
use super::state::{
    HelperGenerationStatus, MidiSlotErrorState, mode_reference_requirement,
    mode_reference_requirement_satisfied,
};
use super::utils::{
    choose_dropped_midi_path, display_file_name_from_path, dropped_path_to_load,
    log_generation_request_submission, normalize_api_key_input,
};
use super::{
    API_KEY_PLACEHOLDER, JOB_UPDATE_POLL_INTERVAL_MS, MIDI_SLOT_DROP_ERROR_MESSAGE,
    MIDI_SLOT_DROP_HINT, MIDI_SLOT_EMPTY_LABEL, MIDI_SLOT_FILE_PICKER_PROMPT,
    MIDI_SLOT_UNSUPPORTED_FILE_MESSAGE, PROMPT_EDITOR_HEIGHT_PX, PROMPT_EDITOR_ROWS,
    PROMPT_PLACEHOLDER, PROMPT_VALIDATION_MESSAGE,
};

const MIDI_CHANNEL_MIN: u8 = 1;
const MIDI_CHANNEL_MAX: u8 = 16;
const LIVE_CAPTURE_POLL_INTERVAL_MS: u64 = 30;
const LIVE_CAPTURE_MAX_EVENTS_PER_POLL: usize = 512;

pub(super) struct SonantMainWindow {
    prompt_input: Entity<InputState>,
    _prompt_input_subscription: Subscription,
    api_key_input: Entity<InputState>,
    _api_key_input_subscription: Subscription,
    load_midi_use_case: Arc<LoadMidiUseCase>,
    live_midi_capture: LiveMidiCapture,
    midi_input_router: MidiInputRouter,
    generation_job_manager: Arc<GenerationJobManager>,
    submission_model: PromptSubmissionModel,
    input_track_model: InputTrackModel,
    recording_channel_enabled: [bool; 16],
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
        let api_key_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(API_KEY_PLACEHOLDER)
                .masked(true)
        });
        let api_key_input_subscription =
            cx.subscribe_in(&api_key_input, window, Self::on_api_key_input_event);

        let backend = build_generation_backend();
        let input_track_model = InputTrackModel::new();
        let recording_channel_enabled = [false; 16];
        let live_midi_capture = LiveMidiCapture::new(Arc::new(NoopLiveInputSource));
        let midi_input_router = MidiInputRouter::new();

        let mut this = Self {
            prompt_input,
            _prompt_input_subscription: prompt_input_subscription,
            api_key_input,
            _api_key_input_subscription: api_key_input_subscription,
            load_midi_use_case: Arc::new(LoadMidiUseCase::new()),
            live_midi_capture,
            midi_input_router,
            generation_job_manager: Arc::clone(&backend.job_manager),
            submission_model: PromptSubmissionModel::new(backend.default_model),
            input_track_model,
            recording_channel_enabled,
            live_capture_playhead_ppq: 0.0,
            selected_generation_mode: GenerationMode::Melody,
            selected_reference_slot: ReferenceSlot::Melody,
            generation_status: HelperGenerationStatus::Idle,
            validation_error: None,
            api_key_error: None,
            input_track_error: None,
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

        let references = self.load_midi_use_case.snapshot_references();
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

    fn reference_slot_button_id(slot: ReferenceSlot) -> &'static str {
        match slot {
            ReferenceSlot::Melody => "reference-slot-melody",
            ReferenceSlot::ChordProgression => "reference-slot-chord-progression",
            ReferenceSlot::DrumPattern => "reference-slot-drum-pattern",
            ReferenceSlot::Bassline => "reference-slot-bassline",
            ReferenceSlot::CounterMelody => "reference-slot-counter-melody",
            ReferenceSlot::Harmony => "reference-slot-harmony",
            ReferenceSlot::ContinuationSeed => "reference-slot-continuation-seed",
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

    fn reference_source_index(source: ReferenceSource) -> usize {
        match source {
            ReferenceSource::File => 0,
            ReferenceSource::Live => 1,
        }
    }

    fn input_track_row_id(slot: ReferenceSlot) -> (&'static str, usize) {
        ("input-track-row", Self::reference_slot_index(slot))
    }

    fn input_track_source_button_id(
        slot: ReferenceSlot,
        source: ReferenceSource,
    ) -> (&'static str, usize) {
        (
            "input-track-source",
            Self::reference_slot_index(slot) * 2 + Self::reference_source_index(source),
        )
    }

    fn input_track_channel_button_id(slot: ReferenceSlot, channel: u8) -> (&'static str, usize) {
        (
            "input-track-channel",
            Self::reference_slot_index(slot) * 100 + usize::from(channel),
        )
    }

    fn input_track_slot_select_button_id(slot: ReferenceSlot) -> (&'static str, usize) {
        ("input-track-select", Self::reference_slot_index(slot))
    }

    fn recording_channel_button_id(channel: u8) -> (&'static str, usize) {
        ("recording-channel", usize::from(channel))
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
        if !(MIDI_CHANNEL_MIN..=MIDI_CHANNEL_MAX).contains(&channel) {
            return false;
        }
        let index = usize::from(channel - MIDI_CHANNEL_MIN);
        self.recording_channel_enabled[index]
    }

    fn live_channel_used_by_other_slots(&self, slot: ReferenceSlot, channel: u8) -> bool {
        live_channel_used_by_other_slots(&self.input_track_model, slot, channel)
    }

    fn first_available_live_channel_for_slot(&self, slot: ReferenceSlot) -> Option<u8> {
        first_available_live_channel_for_slot(&self.input_track_model, slot)
    }

    fn ensure_live_channel_mapping_for_slot(&mut self, slot: ReferenceSlot) -> Result<(), String> {
        let target_channel = preferred_live_channel_for_slot(&self.input_track_model, slot)
            .or_else(|| self.first_available_live_channel_for_slot(slot))
            .ok_or_else(|| {
                format!(
                    "No free MIDI channel is available for {}.",
                    Self::reference_slot_label(slot)
                )
            })?;

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

        self.midi_input_router
            .update_transport_state(true, self.live_capture_playhead_ppq);
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
        self.midi_input_router
            .update_transport_state(true, self.live_capture_playhead_ppq);

        for event in events {
            let Some(channel) = midi_channel_from_status(event.data[0]) else {
                continue;
            };
            self.midi_input_router.push_live_event(channel, event);
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

fn live_channel_used_by_other_slots(
    model: &InputTrackModel,
    slot: ReferenceSlot,
    channel: u8,
) -> bool {
    model
        .live_channel_mappings()
        .iter()
        .any(|mapping| mapping.slot != slot && mapping.channel == channel)
}

fn first_available_live_channel_for_slot(
    model: &InputTrackModel,
    slot: ReferenceSlot,
) -> Option<u8> {
    (MIDI_CHANNEL_MIN..=MIDI_CHANNEL_MAX)
        .find(|channel| !live_channel_used_by_other_slots(model, slot, *channel))
}

fn preferred_live_channel_for_slot(model: &InputTrackModel, slot: ReferenceSlot) -> Option<u8> {
    model
        .channel_mappings()
        .iter()
        .find(|mapping| mapping.slot == slot)
        .map(|mapping| mapping.channel)
        .filter(|channel| !live_channel_used_by_other_slots(model, slot, *channel))
}

impl Render for SonantMainWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let status_label = self.generation_status.label();
        let status_color = self.generation_status.color();
        let generating = self.generation_status.is_submitting_or_running();
        let selected_mode_label = Self::generation_mode_label(self.selected_generation_mode);
        let selected_reference_slot_label =
            Self::reference_slot_label(self.selected_reference_slot);
        let selected_reference_source = self.source_for_slot(self.selected_reference_slot);
        let selected_reference_source_label =
            Self::reference_source_label(selected_reference_source);
        let selected_live_channel = self.channel_mapping_for_slot(self.selected_reference_slot);
        let selected_slot_accepts_file_drop = selected_reference_source == ReferenceSource::File;
        let selected_live_recording_summary =
            self.live_recording_summary_for_slot(self.selected_reference_slot);
        let references = self.load_midi_use_case.snapshot_references();
        let mode_requirement = mode_reference_requirement(self.selected_generation_mode);
        let mode_requirement_satisfied =
            mode_reference_requirement_satisfied(self.selected_generation_mode, &references);
        let selected_slot_references: Vec<&_> = references
            .iter()
            .filter(|reference| reference.slot == self.selected_reference_slot)
            .collect();
        let selected_slot_reference_count = selected_slot_references.len();
        let selected_slot_set = selected_slot_reference_count > 0;
        let selected_slot_error = self
            .midi_slot_error_for_slot(self.selected_reference_slot)
            .cloned();
        let mode_button = |id: &'static str, mode: GenerationMode| {
            let button = Button::new(id)
                .label(Self::generation_mode_label(mode))
                .on_click(cx.listener(move |this, _, _window, cx| {
                    this.on_generation_mode_selected(mode, cx)
                }));
            if self.selected_generation_mode == mode {
                button.primary()
            } else {
                button
            }
        };
        let slot_button = |slot: ReferenceSlot| {
            let button = Button::new(Self::reference_slot_button_id(slot))
                .label(Self::reference_slot_label(slot))
                .on_click(cx.listener(move |this, _, _window, cx| {
                    this.on_reference_slot_selected(slot, cx)
                }));
            if self.selected_reference_slot == slot {
                button.primary()
            } else {
                button
            }
        };

        div()
            .size_full()
            .overflow_y_scrollbar()
            .overflow_x_hidden()
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .bg(rgb(0x111827))
            .text_color(rgb(0xf9fafb))
            .child(Label::new("Sonant GPUI Helper"))
            .child(Label::new(
                "FR-05: Prompt input and multi-slot reference MIDI editing are wired to app use cases.",
            ))
            .child(Label::new("API Key (testing)"))
            .child(Input::new(&self.api_key_input).mask_toggle())
            .children(self.api_key_error.iter().map(|message| {
                div()
                    .text_color(rgb(0xfca5a5))
                    .child(format!("API Key: {message}"))
            }))
            .child(Label::new("Generation Mode"))
            .child(
                div()
                    .id("generation-mode-selector")
                    .flex()
                    .flex_col()
                    .gap_2()
                    .p_3()
                    .border_1()
                    .border_color(rgb(0x334155))
                    .bg(rgb(0x0f172a))
                    .child(
                        div()
                            .text_color(rgb(0x93c5fd))
                            .child(format!("Selected: {selected_mode_label}")),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div().text_color(rgb(0x94a3b8)).child(format!(
                                    "Requirement: {}",
                                    mode_requirement.description
                                )),
                            )
                            .children(
                                std::iter::once(mode_requirement_satisfied)
                                    .filter(|ready| *ready)
                                    .map(|_| {
                                        div()
                                            .text_color(rgb(0x86efac))
                                            .child("Reference requirement satisfied.")
                                    }),
                            )
                            .children(
                                mode_requirement
                                    .unmet_message
                                    .iter()
                                    .filter(|_| !mode_requirement_satisfied)
                                    .map(|message| div().text_color(rgb(0xfca5a5)).child(*message)),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(mode_button(
                                "generation-mode-melody",
                                GenerationMode::Melody,
                            ))
                            .child(mode_button(
                                "generation-mode-chord-progression",
                                GenerationMode::ChordProgression,
                            ))
                            .child(mode_button(
                                "generation-mode-drum-pattern",
                                GenerationMode::DrumPattern,
                            ))
                            .child(mode_button(
                                "generation-mode-bassline",
                                GenerationMode::Bassline,
                            )),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(mode_button(
                                "generation-mode-counter-melody",
                                GenerationMode::CounterMelody,
                            ))
                            .child(mode_button(
                                "generation-mode-harmony",
                                GenerationMode::Harmony,
                            ))
                            .child(mode_button(
                                "generation-mode-continuation",
                                GenerationMode::Continuation,
                            )),
                    ),
            )
            .child(Label::new("Reference Slot"))
            .child(
                div()
                    .id("reference-slot-selector")
                    .flex()
                    .flex_col()
                    .gap_2()
                    .p_3()
                    .border_1()
                    .border_color(rgb(0x334155))
                    .bg(rgb(0x0f172a))
                    .child(
                        div()
                            .text_color(rgb(0x93c5fd))
                            .child(format!("Selected: {selected_reference_slot_label}")),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(slot_button(ReferenceSlot::Melody))
                            .child(slot_button(ReferenceSlot::ChordProgression))
                            .child(slot_button(ReferenceSlot::DrumPattern))
                            .child(slot_button(ReferenceSlot::Bassline)),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(slot_button(ReferenceSlot::CounterMelody))
                            .child(slot_button(ReferenceSlot::Harmony))
                            .child(slot_button(ReferenceSlot::ContinuationSeed)),
                    ),
            )
            .child(Label::new("Input Tracks"))
            .child(
                div()
                    .id("input-track-list")
                    .flex()
                    .flex_col()
                    .gap_2()
                    .children(Self::reference_slots().iter().copied().map(|slot| {
                        let slot_label = Self::reference_slot_label(slot);
                        let slot_source = self.source_for_slot(slot);
                        let slot_channel = self.channel_mapping_for_slot(slot);
                        let slot_live_summary = self.live_recording_summary_for_slot(slot);
                        let slot_is_selected = self.selected_reference_slot == slot;
                        let select_button = Button::new(Self::input_track_slot_select_button_id(slot))
                            .label(if slot_is_selected { "Selected" } else { "Select Slot" })
                            .on_click(cx.listener(move |this, _, _window, cx| {
                                this.on_reference_slot_selected(slot, cx)
                            }));
                        let select_button = if slot_is_selected {
                            select_button.primary()
                        } else {
                            select_button
                        };
                        div()
                            .id(Self::input_track_row_id(slot))
                            .flex()
                            .flex_col()
                            .gap_2()
                            .p_3()
                            .border_1()
                            .border_color(if slot_is_selected {
                                rgb(0x67e8f9)
                            } else {
                                rgb(0x334155)
                            })
                            .bg(if slot_is_selected {
                                rgb(0x082f49)
                            } else {
                                rgb(0x0f172a)
                            })
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap_2()
                                    .child(div().text_color(rgb(0x93c5fd)).child(slot_label))
                                    .child(select_button),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        if slot_source == ReferenceSource::File {
                                            Button::new(Self::input_track_source_button_id(
                                                slot,
                                                ReferenceSource::File,
                                            ))
                                            .label("File")
                                            .primary()
                                            .on_click(cx.listener(move |this, _, _window, cx| {
                                                this.on_reference_source_selected(
                                                    slot,
                                                    ReferenceSource::File,
                                                    cx,
                                                )
                                            }))
                                        } else {
                                            Button::new(Self::input_track_source_button_id(
                                                slot,
                                                ReferenceSource::File,
                                            ))
                                            .label("File")
                                            .on_click(cx.listener(move |this, _, _window, cx| {
                                                this.on_reference_source_selected(
                                                    slot,
                                                    ReferenceSource::File,
                                                    cx,
                                                )
                                            }))
                                        },
                                    )
                                    .child(
                                        if slot_source == ReferenceSource::Live {
                                            Button::new(Self::input_track_source_button_id(
                                                slot,
                                                ReferenceSource::Live,
                                            ))
                                            .label("Live")
                                            .primary()
                                            .on_click(cx.listener(move |this, _, _window, cx| {
                                                this.on_reference_source_selected(
                                                    slot,
                                                    ReferenceSource::Live,
                                                    cx,
                                                )
                                            }))
                                        } else {
                                            Button::new(Self::input_track_source_button_id(
                                                slot,
                                                ReferenceSource::Live,
                                            ))
                                            .label("Live")
                                            .on_click(cx.listener(move |this, _, _window, cx| {
                                                this.on_reference_source_selected(
                                                    slot,
                                                    ReferenceSource::Live,
                                                    cx,
                                                )
                                            }))
                                        },
                                    )
                                    .child(
                                        div()
                                            .text_color(rgb(0x94a3b8))
                                            .child(format!("Source: {}", Self::reference_source_label(slot_source))),
                                    ),
                            )
                            .child(
                                div().text_color(rgb(0x94a3b8)).child(format!(
                                    "Assigned Live Channel: {}",
                                    slot_channel
                                        .map(|channel| channel.to_string())
                                        .unwrap_or_else(|| "Not set".to_string())
                                )),
                            )
                            .children(
                                std::iter::once(slot_source)
                                    .filter(|source| *source == ReferenceSource::Live)
                                    .map(|_| {
                                        let selected_channel = slot_channel;
                                        let live_summary = slot_live_summary;
                                        div()
                                            .flex()
                                            .flex_col()
                                            .gap_2()
                                            .child(
                                                div().text_color(rgb(0x93c5fd)).child(format!(
                                                    "Recorded Bars: {} / Notes: {} / Events: {}",
                                                    live_summary.bar_count,
                                                    live_summary.note_count,
                                                    live_summary.event_count
                                                )),
                                            )
                                            .child(
                                                div().text_color(rgb(0x94a3b8)).child(format!(
                                                    "Pitch Range: {}",
                                                    match (
                                                        live_summary.min_pitch,
                                                        live_summary.max_pitch,
                                                    ) {
                                                        (Some(min), Some(max)) =>
                                                            format!("{min}..{max}"),
                                                        _ => "N/A".to_string(),
                                                    }
                                                )),
                                            )
                                            .child(
                                                div().text_color(rgb(0x93c5fd)).child(format!(
                                                    "Live Channel: {}",
                                                    selected_channel
                                                        .map(|channel| channel.to_string())
                                                        .unwrap_or_else(|| "Not set".to_string())
                                                )),
                                            )
                                            .child(
                                                div()
                                                    .flex()
                                                    .items_center()
                                                    .gap_1()
                                                    .children((1..=8).map(|channel| {
                                                        let channel = channel as u8;
                                                        let disabled = self
                                                            .live_channel_used_by_other_slots(slot, channel);
                                                        let button = Button::new(
                                                            Self::input_track_channel_button_id(
                                                                slot, channel,
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
                                                                    slot, channel, cx,
                                                                )
                                                            },
                                                        ));
                                                        if selected_channel == Some(channel) {
                                                            button.primary()
                                                        } else {
                                                            button
                                                        }
                                                    })),
                                            )
                                            .child(
                                                div()
                                                    .flex()
                                                    .items_center()
                                                    .gap_1()
                                                    .children((9..=16).map(|channel| {
                                                        let channel = channel as u8;
                                                        let disabled = self
                                                            .live_channel_used_by_other_slots(slot, channel);
                                                        let button = Button::new(
                                                            Self::input_track_channel_button_id(
                                                                slot, channel,
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
                                                                    slot, channel, cx,
                                                                )
                                                            },
                                                        ));
                                                        if selected_channel == Some(channel) {
                                                            button.primary()
                                                        } else {
                                                            button
                                                        }
                                                    })),
                                            )
                                            .child(
                                                div()
                                                    .text_color(rgb(0x94a3b8))
                                                    .child("`*` means already used by another Live slot."),
                                            )
                                    }),
                            )
                    })),
            )
            .children(self.input_track_error.iter().map(|message| {
                div()
                    .text_color(rgb(0xfca5a5))
                    .child(format!("Input Tracks: {message}"))
            }))
            .child(Label::new("MIDI Channel Recording"))
            .child(
                div()
                    .id("recording-channel-panel")
                    .flex()
                    .flex_col()
                    .gap_2()
                    .p_3()
                    .border_1()
                    .border_color(rgb(0x334155))
                    .bg(rgb(0x0f172a))
                    .child(
                        div()
                            .text_color(rgb(0x94a3b8))
                            .child("Toggle recording capture per MIDI Channel."),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .children((1..=8).map(|channel| {
                                let channel = channel as u8;
                                let enabled = self.recording_enabled_for_channel(channel);
                                let button = Button::new(Self::recording_channel_button_id(channel))
                                    .label(if enabled {
                                        format!("Ch {channel} ON")
                                    } else {
                                        format!("Ch {channel} OFF")
                                    })
                                    .on_click(cx.listener(move |this, _, _window, cx| {
                                        this.on_recording_channel_toggled(channel, cx)
                                    }));
                                if enabled { button.primary() } else { button }
                            })),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .children((9..=16).map(|channel| {
                                let channel = channel as u8;
                                let enabled = self.recording_enabled_for_channel(channel);
                                let button = Button::new(Self::recording_channel_button_id(channel))
                                    .label(if enabled {
                                        format!("Ch {channel} ON")
                                    } else {
                                        format!("Ch {channel} OFF")
                                    })
                                    .on_click(cx.listener(move |this, _, _window, cx| {
                                        this.on_recording_channel_toggled(channel, cx)
                                    }));
                                if enabled { button.primary() } else { button }
                            })),
                    ),
            )
            .child(Label::new("Reference Slot Overview"))
            .child(
                div()
                    .id("reference-slot-overview")
                    .flex()
                    .flex_col()
                    .gap_2()
                    .children(Self::reference_slots().iter().copied().map(|slot| {
                        let slot_references: Vec<&_> = references
                            .iter()
                            .filter(|reference| reference.slot == slot)
                            .collect();
                        let slot_reference_count = slot_references.len();
                        let slot_source = self.source_for_slot(slot);
                        let slot_channel = self.channel_mapping_for_slot(slot);
                        let slot_live_summary = self.live_recording_summary_for_slot(slot);
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .p_2()
                            .border_1()
                            .border_color(if slot == self.selected_reference_slot {
                                rgb(0x67e8f9)
                            } else {
                                rgb(0x334155)
                            })
                            .bg(if slot == self.selected_reference_slot {
                                rgb(0x082f49)
                            } else {
                                rgb(0x0f172a)
                            })
                            .child(div().text_color(rgb(0x93c5fd)).child(Self::reference_slot_label(slot)))
                            .child(
                                div()
                                    .text_color(rgb(0x94a3b8))
                                    .child(format!("Source: {}", Self::reference_source_label(slot_source))),
                            )
                            .children(std::iter::once(slot_source).filter_map(|source| {
                                if source == ReferenceSource::Live {
                                    Some(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .gap_1()
                                            .text_color(rgb(0x94a3b8))
                                            .child(format!(
                                                "Live Channel: {}",
                                                slot_channel
                                                    .map(|channel| channel.to_string())
                                                    .unwrap_or_else(|| "Not set".to_string())
                                            ))
                                            .child(format!(
                                                "Recorded Bars: {} / Notes: {} / Events: {}",
                                                slot_live_summary.bar_count,
                                                slot_live_summary.note_count,
                                                slot_live_summary.event_count
                                            ))
                                            .child(format!(
                                                "Pitch Range: {}",
                                                match (
                                                    slot_live_summary.min_pitch,
                                                    slot_live_summary.max_pitch,
                                                ) {
                                                    (Some(min), Some(max)) => format!("{min}..{max}"),
                                                    _ => "N/A".to_string(),
                                                }
                                            )),
                                    )
                                } else {
                                    None
                                }
                            }))
                            .child(div().child(format!("Registered File MIDI: {slot_reference_count}")))
                            .children(
                                std::iter::once((slot_reference_count, slot_source))
                                    .filter(|(count, source)| {
                                        *count == 0 && *source == ReferenceSource::File
                                    })
                                    .map(|_| div().child(format!("File: {MIDI_SLOT_EMPTY_LABEL}"))),
                            )
                            .children(
                                slot_references
                                    .iter()
                                    .enumerate()
                                    .map(|(index, reference)| {
                                        let slot_file_path = reference
                                            .file
                                            .as_ref()
                                            .map(|file| file.path.clone())
                                            .unwrap_or_else(|| MIDI_SLOT_EMPTY_LABEL.to_string());
                                        let slot_file_label = display_file_name_from_path(&slot_file_path);
                                        let slot_stats = format!(
                                            "Bars: {} / Notes: {}",
                                            reference.bars, reference.note_count
                                        );
                                        div()
                                            .flex()
                                            .flex_col()
                                            .gap_1()
                                            .child(
                                                div().text_color(rgb(0x93c5fd)).child(format!(
                                                    "#{}: {slot_file_label}",
                                                    index + 1
                                                )),
                                            )
                                            .child(div().text_color(rgb(0x93c5fd)).child(slot_stats))
                                            .child(div().text_color(rgb(0x94a3b8)).child(slot_file_path))
                                    }),
                            )
                    })),
            )
            .child(Label::new(format!(
                "Reference MIDI ({selected_reference_slot_label} Slot / Source: {selected_reference_source_label})"
            )))
            .child(
                div()
                    .id("midi-slot-selected")
                    .flex()
                    .flex_col()
                    .gap_2()
                    .p_3()
                    .border_1()
                    .border_color(rgb(0x334155))
                    .bg(rgb(0x0f172a))
                    .can_drop(move |value, _, _| {
                        selected_slot_accepts_file_drop
                            && value
                                .downcast_ref::<ExternalPaths>()
                                .is_some_and(|paths| !paths.paths().is_empty())
                    })
                    .drag_over::<ExternalPaths>(move |style, paths, _, _| {
                        if !selected_slot_accepts_file_drop {
                            style.border_color(rgb(0x334155)).bg(rgb(0x0f172a))
                        } else if choose_dropped_midi_path(paths.paths()).is_some() {
                            style.border_color(rgb(0x67e8f9)).bg(rgb(0x082f49))
                        } else {
                            style.border_color(rgb(0xfda4af)).bg(rgb(0x3f1d2e))
                        }
                    })
                    .on_drop(cx.listener(|this, paths: &ExternalPaths, _window, cx| {
                        this.on_midi_slot_drop(paths, cx)
                    }))
                    .child(if selected_slot_accepts_file_drop {
                        div().child(format!(
                            "{MIDI_SLOT_DROP_HINT} Target: {selected_reference_slot_label} (appends to this slot)."
                        ))
                    } else {
                        div().child(format!(
                            "{selected_reference_slot_label} is using Live input. Switch source to File to load MIDI files."
                        ))
                    })
                    .children(std::iter::once(selected_live_channel).filter_map(|channel| {
                        if selected_reference_source == ReferenceSource::Live {
                            Some(
                                div().flex().flex_col().gap_1().child(
                                    div().text_color(rgb(0x93c5fd)).child(format!(
                                        "Live Channel: {}",
                                        channel
                                            .map(|value| value.to_string())
                                            .unwrap_or_else(|| "Not set".to_string())
                                    )),
                                )
                                .child(
                                    div().text_color(rgb(0x93c5fd)).child(format!(
                                        "Recorded Bars: {} / Notes: {} / Events: {}",
                                        selected_live_recording_summary.bar_count,
                                        selected_live_recording_summary.note_count,
                                        selected_live_recording_summary.event_count
                                    )),
                                )
                                .child(
                                    div().text_color(rgb(0x94a3b8)).child(format!(
                                        "Pitch Range: {}",
                                        match (
                                            selected_live_recording_summary.min_pitch,
                                            selected_live_recording_summary.max_pitch,
                                        ) {
                                            (Some(min), Some(max)) => format!("{min}..{max}"),
                                            _ => "N/A".to_string(),
                                        }
                                    )),
                                ),
                            )
                        } else {
                            None
                        }
                    }))
                    .child(div().child(format!(
                        "Registered File MIDI in slot: {selected_slot_reference_count}"
                    )))
                    .children(
                        std::iter::once((selected_slot_reference_count, selected_reference_source))
                            .filter(|(count, source)| {
                                *count == 0 && *source == ReferenceSource::File
                            })
                            .map(|_| div().child(format!("File: {MIDI_SLOT_EMPTY_LABEL}"))),
                    )
                    .children(
                        selected_slot_references
                            .iter()
                            .enumerate()
                            .map(|(index, reference)| {
                                let slot_file_path = reference
                                    .file
                                    .as_ref()
                                    .map(|file| file.path.clone())
                                    .unwrap_or_else(|| MIDI_SLOT_EMPTY_LABEL.to_string());
                                let slot_file_label = display_file_name_from_path(&slot_file_path);
                                let slot_stats =
                                    format!("Bars: {} / Notes: {}", reference.bars, reference.note_count);
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .p_2()
                                    .border_1()
                                    .border_color(rgb(0x334155))
                                    .bg(rgb(0x111827))
                                    .child(
                                        div()
                                            .text_color(rgb(0x93c5fd))
                                            .child(format!("#{}: {slot_file_label}", index + 1)),
                                    )
                                    .child(div().text_color(rgb(0x93c5fd)).child(slot_stats))
                                    .child(div().text_color(rgb(0x94a3b8)).child(slot_file_path))
                            }),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                Button::new("midi-slot-select-button")
                                    .label("Select MIDI File")
                                    .disabled(!selected_slot_accepts_file_drop)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.on_select_midi_file_clicked(window, cx)
                                    })),
                            )
                            .child(
                                Button::new("midi-slot-clear-button")
                                    .label("Clear Slot")
                                    .disabled(!selected_slot_set || !selected_slot_accepts_file_drop)
                                    .on_click(cx.listener(|this, _, _window, cx| {
                                        this.on_clear_midi_slot_clicked(cx)
                                    })),
                            ),
                    ),
            )
            .children(selected_slot_error.into_iter().map(|error| {
                let slot_label = Self::reference_slot_label(error.slot);
                let retry_slot = error.slot;
                let slot_is_file_source = self.source_for_slot(error.slot) == ReferenceSource::File;
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_color(rgb(0xfca5a5))
                            .child(format!("Reference MIDI ({slot_label}): {}", error.message)),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                Button::new("midi-slot-retry-button")
                                    .label("Retry")
                                    .disabled(!error.can_retry() || !slot_is_file_source)
                                    .on_click(cx.listener(move |this, _, _window, cx| {
                                        this.on_retry_midi_slot_clicked(retry_slot, cx)
                                    })),
                            )
                            .child(
                                Button::new("midi-slot-reselect-button")
                                    .label("Choose Another File")
                                    .disabled(!slot_is_file_source)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.on_select_midi_file_clicked(window, cx)
                                    })),
                            ),
                    )
            }))
            .child(Label::new("Prompt"))
            .child(Input::new(&self.prompt_input).h(px(PROMPT_EDITOR_HEIGHT_PX)))
            .children(self.validation_error.iter().map(|message| {
                div()
                    .text_color(rgb(0xfca5a5))
                    .child(format!("Validation: {message}"))
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_3()
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
                    )
                    .child(div().text_color(status_color).child(status_label)),
            )
            .children(self.startup_notice.iter().map(|notice| {
                div()
                    .text_color(rgb(0x93c5fd))
                    .child(format!("Backend: {notice}"))
            }))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        first_available_live_channel_for_slot, live_channel_used_by_other_slots,
        midi_channel_from_status, preferred_live_channel_for_slot, summarize_live_recording,
    };
    use sonant::app::{ChannelMapping, InputTrackModel, LiveInputEvent};
    use sonant::domain::{ReferenceSlot, ReferenceSource};

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
            first_available_live_channel_for_slot(&model, ReferenceSlot::CounterMelody),
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
            first_available_live_channel_for_slot(&model, ReferenceSlot::Bassline),
            Some(1)
        );
    }

    #[test]
    fn summarize_live_recording_counts_note_events_and_pitch_range() {
        let events = vec![
            LiveInputEvent {
                time: 0,
                port_index: 0,
                data: [0x90, 60, 96],
            },
            LiveInputEvent {
                time: 1,
                port_index: 0,
                data: [0x90, 72, 100],
            },
            LiveInputEvent {
                time: 2,
                port_index: 0,
                data: [0x80, 60, 0],
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
    fn midi_channel_from_status_maps_channel_voice_messages() {
        assert_eq!(midi_channel_from_status(0x90), Some(1));
        assert_eq!(midi_channel_from_status(0x9F), Some(16));
        assert_eq!(midi_channel_from_status(0x80), Some(1));
        assert_eq!(midi_channel_from_status(0xF8), None);
        assert_eq!(midi_channel_from_status(0x20), None);
    }
}
