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
};
use sonant::{
    app::{
        GenerationJobManager, GenerationJobState, GenerationJobUpdate, LoadMidiCommand,
        LoadMidiUseCase,
    },
    domain::{GenerationMode, LlmError, ReferenceSlot, has_supported_midi_extension},
};

use super::backend::{
    GenerationBackend, build_generation_backend, build_generation_backend_from_api_key,
};
use super::request::PromptSubmissionModel;
use super::state::{HelperGenerationStatus, MidiSlotErrorState};
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

pub(super) struct SonantMainWindow {
    prompt_input: Entity<InputState>,
    _prompt_input_subscription: Subscription,
    api_key_input: Entity<InputState>,
    _api_key_input_subscription: Subscription,
    load_midi_use_case: Arc<LoadMidiUseCase>,
    generation_job_manager: Arc<GenerationJobManager>,
    submission_model: PromptSubmissionModel,
    selected_generation_mode: GenerationMode,
    generation_status: HelperGenerationStatus,
    validation_error: Option<String>,
    api_key_error: Option<String>,
    midi_slot_error: Option<MidiSlotErrorState>,
    active_test_api_key: Option<String>,
    startup_notice: Option<String>,
    _update_poll_task: Task<()>,
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

        Self {
            prompt_input,
            _prompt_input_subscription: prompt_input_subscription,
            api_key_input,
            _api_key_input_subscription: api_key_input_subscription,
            load_midi_use_case: Arc::new(LoadMidiUseCase::new()),
            generation_job_manager: Arc::clone(&backend.job_manager),
            submission_model: PromptSubmissionModel::new(backend.default_model),
            selected_generation_mode: GenerationMode::Melody,
            generation_status: HelperGenerationStatus::Idle,
            validation_error: None,
            api_key_error: None,
            midi_slot_error: None,
            active_test_api_key: None,
            startup_notice: backend.startup_notice,
            _update_poll_task: Task::ready(()),
            _midi_file_picker_task: Task::ready(()),
        }
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

        let prompt = self.prompt_input.read(cx).value().to_string();
        let request = match self.submission_model.prepare_request(
            self.selected_generation_mode,
            prompt,
            self.load_midi_use_case.snapshot_references(),
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
            self.midi_slot_error = Some(MidiSlotErrorState::non_retryable(error.user_message()));
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
                                view.midi_slot_error = Some(MidiSlotErrorState::non_retryable(
                                    MIDI_SLOT_UNSUPPORTED_FILE_MESSAGE,
                                ));
                                cx.notify();
                                return;
                            }

                            let path = path.to_string_lossy().to_string();
                            view.set_midi_slot_file(path, cx);
                        });
                    }
                }
                Ok(None) => {}
                Err(error) => {
                    let message = format!("Could not open the file dialog: {error}");
                    let _ = view.update_in(window, |view, _window, cx| {
                        view.midi_slot_error = Some(MidiSlotErrorState::non_retryable(message));
                        cx.notify();
                    });
                }
            }
        });
    }

    fn on_midi_slot_drop(&mut self, paths: &ExternalPaths, cx: &mut Context<Self>) {
        let Some(path) = dropped_path_to_load(paths) else {
            self.midi_slot_error = Some(MidiSlotErrorState::non_retryable(
                MIDI_SLOT_DROP_ERROR_MESSAGE,
            ));
            cx.notify();
            return;
        };

        self.set_midi_slot_file(path, cx);
    }

    fn set_midi_slot_file(&mut self, path: String, cx: &mut Context<Self>) {
        self.midi_slot_error = None;
        match self.load_midi_use_case.execute(LoadMidiCommand::SetFile {
            slot: ReferenceSlot::Melody,
            path: path.clone(),
        }) {
            Ok(_) => cx.notify(),
            Err(error) => {
                self.midi_slot_error = Some(MidiSlotErrorState::from_load_error(&path, &error));
                cx.notify();
            }
        }
    }

    fn on_retry_midi_slot_clicked(&mut self, cx: &mut Context<Self>) {
        let retry_path = self
            .midi_slot_error
            .as_ref()
            .and_then(|error| error.retry_path.clone());
        if let Some(path) = retry_path {
            self.set_midi_slot_file(path, cx);
        }
    }

    fn on_clear_midi_slot_clicked(&mut self, cx: &mut Context<Self>) {
        self.midi_slot_error = None;
        match self.load_midi_use_case.execute(LoadMidiCommand::ClearSlot {
            slot: ReferenceSlot::Melody,
        }) {
            Ok(_) => cx.notify(),
            Err(error) => {
                self.midi_slot_error =
                    Some(MidiSlotErrorState::non_retryable(error.user_message()));
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

impl Render for SonantMainWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let status_label = self.generation_status.label();
        let status_color = self.generation_status.color();
        let generating = self.generation_status.is_submitting_or_running();
        let selected_mode_label = Self::generation_mode_label(self.selected_generation_mode);
        let melody_reference = self
            .load_midi_use_case
            .slot_reference(ReferenceSlot::Melody);
        let melody_file_path = melody_reference
            .as_ref()
            .and_then(|reference| reference.file.as_ref())
            .map(|file| file.path.clone());
        let melody_file_label = melody_file_path
            .as_deref()
            .map(display_file_name_from_path)
            .unwrap_or_else(|| MIDI_SLOT_EMPTY_LABEL.to_string());
        let melody_slot_stats = melody_reference
            .as_ref()
            .map(|reference| format!("Bars: {} / Notes: {}", reference.bars, reference.note_count));
        let melody_slot_set = melody_reference.is_some();
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

        div()
            .size_full()
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .bg(rgb(0x111827))
            .text_color(rgb(0xf9fafb))
            .child(Label::new("Sonant GPUI Helper"))
            .child(Label::new(
                "FR-02/03a: Prompt input and reference MIDI file slot wired to app use cases.",
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
            .child(Label::new("Reference MIDI (Melody Slot)"))
            .child(
                div()
                    .id("midi-slot-melody")
                    .flex()
                    .flex_col()
                    .gap_2()
                    .p_3()
                    .border_1()
                    .border_color(rgb(0x334155))
                    .bg(rgb(0x0f172a))
                    .can_drop(|value, _, _| {
                        value
                            .downcast_ref::<ExternalPaths>()
                            .is_some_and(|paths| !paths.paths().is_empty())
                    })
                    .drag_over::<ExternalPaths>(|style, paths, _, _| {
                        if choose_dropped_midi_path(paths.paths()).is_some() {
                            style.border_color(rgb(0x67e8f9)).bg(rgb(0x082f49))
                        } else {
                            style.border_color(rgb(0xfda4af)).bg(rgb(0x3f1d2e))
                        }
                    })
                    .on_drop(cx.listener(|this, paths: &ExternalPaths, _window, cx| {
                        this.on_midi_slot_drop(paths, cx)
                    }))
                    .child(div().child(MIDI_SLOT_DROP_HINT))
                    .child(div().child(format!("File: {melody_file_label}")))
                    .children(
                        melody_slot_stats
                            .iter()
                            .map(|stats| div().text_color(rgb(0x93c5fd)).child(stats.clone())),
                    )
                    .children(
                        melody_file_path
                            .iter()
                            .map(|path| div().text_color(rgb(0x94a3b8)).child(path.clone())),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                Button::new("midi-slot-select-button")
                                    .label("Select MIDI File")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.on_select_midi_file_clicked(window, cx)
                                    })),
                            )
                            .child(
                                Button::new("midi-slot-clear-button")
                                    .label("Clear")
                                    .disabled(!melody_slot_set)
                                    .on_click(cx.listener(|this, _, _window, cx| {
                                        this.on_clear_midi_slot_clicked(cx)
                                    })),
                            ),
                    ),
            )
            .children(self.midi_slot_error.iter().map(|error| {
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_color(rgb(0xfca5a5))
                            .child(format!("Reference MIDI: {}", error.message)),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                Button::new("midi-slot-retry-button")
                                    .label("Retry")
                                    .disabled(!error.can_retry())
                                    .on_click(cx.listener(|this, _, _window, cx| {
                                        this.on_retry_midi_slot_clicked(cx)
                                    })),
                            )
                            .child(
                                Button::new("midi-slot-reselect-button")
                                    .label("Choose Another File")
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
                            .disabled(generating)
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
