use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    App, Application, Bounds, Context, Entity, ExternalPaths, IntoElement, PathPromptOptions,
    Render, Subscription, Task, Timer, Window, WindowBounds, WindowOptions, div, prelude::*, px,
    rgb, size,
};
use gpui_component::{
    Disableable, Root,
    button::{Button, ButtonVariants as _},
    input::{Input, InputEvent, InputState},
    label::Label,
};
use sonant::{
    app::{
        GenerationJobManager, GenerationJobState, GenerationJobUpdate, GenerationService,
        LoadMidiCommand, LoadMidiUseCase,
    },
    domain::{
        GenerationMode, GenerationParams, GenerationRequest, GenerationResult, LlmError,
        MidiReferenceSummary, ModelRef, ReferenceSlot, has_supported_midi_extension,
    },
    infra::llm::{AnthropicProvider, LlmProvider, OpenAiCompatibleProvider, ProviderRegistry},
};

#[cfg(target_os = "macos")]
use cocoa::{
    appkit::{
        NSApplication, NSApplicationActivationPolicy::NSApplicationActivationPolicyAccessory,
    },
    base::nil,
};

const HELPER_WINDOW_WIDTH: f32 = 700.0;
const HELPER_WINDOW_HEIGHT: f32 = 640.0;
const PROMPT_EDITOR_HEIGHT_PX: f32 = 220.0;
const PROMPT_EDITOR_ROWS: usize = 8;
const JOB_UPDATE_POLL_INTERVAL_MS: u64 = 50;

const DEFAULT_BPM: u16 = 120;
const DEFAULT_DENSITY: u8 = 3;
const DEFAULT_COMPLEXITY: u8 = 3;
const DEFAULT_TEMPERATURE: f32 = 0.7;
const DEFAULT_TOP_P: f32 = 0.9;
const DEFAULT_MAX_TOKENS: u16 = 512;
const DEFAULT_VARIATION_COUNT: u8 = 1;

const DEFAULT_ANTHROPIC_MODEL: &str = "claude-3-5-sonnet";
const DEFAULT_OPENAI_COMPAT_MODEL: &str = "gpt-5.2";
const GPUI_HELPER_REQUEST_ID_PREFIX: &str = "gpui-helper-req";

const STUB_PROVIDER_ID: &str = "helper_stub";
const STUB_MODEL_ID: &str = "helper-unconfigured";

const PROMPT_PLACEHOLDER: &str =
    "Describe what to generate, for example: Bright pop melody in C major with syncopation.";
const PROMPT_VALIDATION_MESSAGE: &str = "Prompt must not be empty.";
const STUB_PROVIDER_NOTICE: &str = "No LLM provider is configured. Set SONANT_ANTHROPIC_API_KEY or SONANT_OPENAI_COMPAT_API_KEY to enable real generation requests.";
const TEST_API_KEY_BACKEND_NOTICE: &str = "Using API key from helper input for Anthropic backend.";
const API_KEY_PLACEHOLDER: &str = "Anthropic API key (testing only)";
const MIDI_SLOT_FILE_PICKER_PROMPT: &str = "Select MIDI File (.mid/.midi)";
const MIDI_SLOT_DROP_HINT: &str = "Drop a .mid/.midi file here or choose one from the dialog.";
const MIDI_SLOT_EMPTY_LABEL: &str = "Not set";
const MIDI_SLOT_DROP_ERROR_MESSAGE: &str = "Drop at least one file to set the MIDI reference.";
const MIDI_SLOT_UNSUPPORTED_FILE_MESSAGE: &str = "Only .mid or .midi files are supported.";
const DEBUG_PROMPT_LOG_ENV: &str = "SONANT_HELPER_DEBUG_PROMPT_LOG";
const DEBUG_PROMPT_PREVIEW_CHARS: usize = 120;

fn main() {
    let is_helper = std::env::args().any(|arg| arg == "--gpui-helper");

    if is_helper {
        run_gpui_helper();
        return;
    }

    eprintln!("Sonant helper binary. Run with --gpui-helper.");
}

fn run_gpui_helper() {
    Application::new().run(|cx: &mut App| {
        set_plugin_helper_activation_policy();
        gpui_component::init(cx);

        let bounds = Bounds::centered(
            None,
            size(px(HELPER_WINDOW_WIDTH), px(HELPER_WINDOW_HEIGHT)),
            cx,
        );
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            ..Default::default()
        };

        if cx
            .open_window(options, |window, cx| {
                let view = cx.new(|cx| SonantMainWindow::new(window, cx));
                cx.new(|cx| Root::new(view, window, cx))
            })
            .is_err()
        {
            cx.quit();
            return;
        }

        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();

        cx.activate(true);
        set_plugin_helper_activation_policy();
    });
}

#[cfg(target_os = "macos")]
fn set_plugin_helper_activation_policy() {
    unsafe {
        let app = NSApplication::sharedApplication(nil);
        app.setActivationPolicy_(NSApplicationActivationPolicyAccessory);
    }
}

#[cfg(not(target_os = "macos"))]
fn set_plugin_helper_activation_policy() {}

struct SonantMainWindow {
    prompt_input: Entity<InputState>,
    _prompt_input_subscription: Subscription,
    api_key_input: Entity<InputState>,
    _api_key_input_subscription: Subscription,
    load_midi_use_case: Arc<LoadMidiUseCase>,
    generation_job_manager: Arc<GenerationJobManager>,
    submission_model: PromptSubmissionModel,
    generation_status: HelperGenerationStatus,
    validation_error: Option<String>,
    api_key_error: Option<String>,
    midi_slot_error: Option<String>,
    active_test_api_key: Option<String>,
    startup_notice: Option<String>,
    _update_poll_task: Task<()>,
    _midi_file_picker_task: Task<()>,
}

impl SonantMainWindow {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
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
        let request = match self
            .submission_model
            .prepare_request(prompt, self.load_midi_use_case.snapshot_references())
        {
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
            self.midi_slot_error = Some(error.user_message());
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
                                view.midi_slot_error =
                                    Some(MIDI_SLOT_UNSUPPORTED_FILE_MESSAGE.to_string());
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
                        view.midi_slot_error = Some(message);
                        cx.notify();
                    });
                }
            }
        });
    }

    fn on_midi_slot_drop(&mut self, paths: &ExternalPaths, cx: &mut Context<Self>) {
        let Some(path) = dropped_path_to_load(paths) else {
            self.midi_slot_error = Some(MIDI_SLOT_DROP_ERROR_MESSAGE.to_string());
            cx.notify();
            return;
        };

        self.set_midi_slot_file(path, cx);
    }

    fn set_midi_slot_file(&mut self, path: String, cx: &mut Context<Self>) {
        self.midi_slot_error = None;
        match self.load_midi_use_case.execute(LoadMidiCommand::SetFile {
            slot: ReferenceSlot::Melody,
            path,
        }) {
            Ok(_) => cx.notify(),
            Err(error) => {
                self.midi_slot_error = Some(error.user_message());
                cx.notify();
            }
        }
    }

    fn on_clear_midi_slot_clicked(&mut self, cx: &mut Context<Self>) {
        self.midi_slot_error = None;
        match self.load_midi_use_case.execute(LoadMidiCommand::ClearSlot {
            slot: ReferenceSlot::Melody,
        }) {
            Ok(_) => cx.notify(),
            Err(error) => {
                self.midi_slot_error = Some(error.user_message());
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
            .children(self.midi_slot_error.iter().map(|message| {
                div()
                    .text_color(rgb(0xfca5a5))
                    .child(format!("Reference MIDI: {message}"))
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

#[derive(Debug, Clone, PartialEq, Eq)]
enum HelperGenerationStatus {
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
    fn label(&self) -> String {
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

    fn color(&self) -> gpui::Hsla {
        match self {
            Self::Idle => rgb(0x93c5fd).into(),
            Self::Submitting { .. } | Self::Running { .. } => rgb(0xfbbf24).into(),
            Self::Succeeded { .. } => rgb(0x86efac).into(),
            Self::Failed { .. } => rgb(0xfca5a5).into(),
            Self::Cancelled { .. } => rgb(0xfcd34d).into(),
        }
    }

    fn is_submitting_or_running(&self) -> bool {
        matches!(self, Self::Submitting { .. } | Self::Running { .. })
    }
}

struct GenerationBackend {
    job_manager: Arc<GenerationJobManager>,
    default_model: ModelRef,
    startup_notice: Option<String>,
}

fn build_generation_backend() -> GenerationBackend {
    let mut registry = ProviderRegistry::new();
    let mut default_model = None;
    let mut notices = Vec::new();

    register_anthropic_provider(&mut registry, &mut default_model, &mut notices);
    register_openai_compatible_provider(&mut registry, &mut default_model, &mut notices);

    if registry.is_empty() {
        return build_stub_backend(notices);
    }

    let service = GenerationService::new(registry);
    let manager = match GenerationJobManager::new(service) {
        Ok(manager) => manager,
        Err(error) => {
            notices.push(format!(
                "Failed to start generation worker, switched to stub provider: {}",
                error.user_message()
            ));
            return build_stub_backend(notices);
        }
    };

    GenerationBackend {
        job_manager: Arc::new(manager),
        default_model: default_model
            .expect("default model must be configured when at least one provider exists"),
        startup_notice: (!notices.is_empty()).then(|| notices.join(" ")),
    }
}

fn register_anthropic_provider(
    registry: &mut ProviderRegistry,
    default_model: &mut Option<ModelRef>,
    notices: &mut Vec<String>,
) {
    match AnthropicProvider::from_env() {
        Ok(provider) => {
            if let Err(error) = registry.register(provider) {
                notices.push(format!(
                    "Anthropic provider could not be registered: {}",
                    error.user_message()
                ));
                return;
            }

            if default_model.is_none() {
                *default_model = Some(ModelRef {
                    provider: "anthropic".to_string(),
                    model: DEFAULT_ANTHROPIC_MODEL.to_string(),
                });
            }
        }
        Err(error) if !is_missing_credentials_error(&error) => {
            notices.push(format!(
                "Anthropic provider is unavailable: {}",
                error.user_message()
            ));
        }
        Err(_) => {}
    }
}

fn register_openai_compatible_provider(
    registry: &mut ProviderRegistry,
    default_model: &mut Option<ModelRef>,
    notices: &mut Vec<String>,
) {
    match OpenAiCompatibleProvider::from_env() {
        Ok(provider) => {
            let provider_id = provider.provider_id().to_string();
            let default_model_id = provider
                .supported_models()
                .into_iter()
                .next()
                .unwrap_or_else(|| DEFAULT_OPENAI_COMPAT_MODEL.to_string());

            if let Err(error) = registry.register(provider) {
                notices.push(format!(
                    "OpenAI-compatible provider could not be registered: {}",
                    error.user_message()
                ));
                return;
            }

            if default_model.is_none() {
                *default_model = Some(ModelRef {
                    provider: provider_id,
                    model: default_model_id,
                });
            }
        }
        Err(error) if !is_missing_credentials_error(&error) => {
            notices.push(format!(
                "OpenAI-compatible provider is unavailable: {}",
                error.user_message()
            ));
        }
        Err(_) => {}
    }
}

fn build_stub_backend(mut notices: Vec<String>) -> GenerationBackend {
    let mut registry = ProviderRegistry::new();
    registry
        .register(HelperUnconfiguredProvider)
        .expect("stub provider registration should succeed");

    let service = GenerationService::new(registry);
    let manager = GenerationJobManager::new(service)
        .expect("stub generation worker should start for helper fallback");

    notices.push(STUB_PROVIDER_NOTICE.to_string());

    GenerationBackend {
        job_manager: Arc::new(manager),
        default_model: ModelRef {
            provider: STUB_PROVIDER_ID.to_string(),
            model: STUB_MODEL_ID.to_string(),
        },
        startup_notice: Some(notices.join(" ")),
    }
}

fn build_generation_backend_from_api_key(api_key: &str) -> Result<GenerationBackend, LlmError> {
    let mut registry = ProviderRegistry::new();
    let provider = AnthropicProvider::from_api_key(api_key.to_string())?;
    registry.register(provider)?;

    let service = GenerationService::new(registry);
    let manager = GenerationJobManager::new(service)?;

    Ok(GenerationBackend {
        job_manager: Arc::new(manager),
        default_model: ModelRef {
            provider: "anthropic".to_string(),
            model: DEFAULT_ANTHROPIC_MODEL.to_string(),
        },
        startup_notice: Some(TEST_API_KEY_BACKEND_NOTICE.to_string()),
    })
}

fn is_missing_credentials_error(error: &LlmError) -> bool {
    matches!(
        error,
        LlmError::Validation { message } if message.contains("API key is missing")
    )
}

struct HelperUnconfiguredProvider;

impl LlmProvider for HelperUnconfiguredProvider {
    fn provider_id(&self) -> &str {
        STUB_PROVIDER_ID
    }

    fn supports_model(&self, model_id: &str) -> bool {
        model_id.trim() == STUB_MODEL_ID
    }

    fn generate(&self, _request: &GenerationRequest) -> Result<GenerationResult, LlmError> {
        Err(LlmError::validation(STUB_PROVIDER_NOTICE))
    }
}

#[derive(Debug, Clone)]
struct PromptSubmissionModel {
    next_request_number: u64,
    model: ModelRef,
}

impl PromptSubmissionModel {
    fn new(model: ModelRef) -> Self {
        Self {
            next_request_number: 1,
            model,
        }
    }

    fn prepare_request(
        &mut self,
        prompt: String,
        references: Vec<MidiReferenceSummary>,
    ) -> Result<GenerationRequest, LlmError> {
        let request_id = format!(
            "{GPUI_HELPER_REQUEST_ID_PREFIX}-{}",
            self.next_request_number
        );
        self.next_request_number = self.next_request_number.saturating_add(1);
        build_generation_request_with_prompt_validation(
            request_id,
            self.model.clone(),
            GenerationMode::Melody,
            prompt,
            references,
        )
    }

    fn set_model(&mut self, model: ModelRef) {
        self.model = model;
    }
}

/// Builds a request after validating only prompt text.
/// Callers must run `GenerationRequest::validate()` before submission.
fn build_generation_request_with_prompt_validation(
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
            key: "C".to_string(),
            scale: "major".to_string(),
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

fn validate_prompt_input(prompt: &str) -> Result<(), LlmError> {
    if prompt.trim().is_empty() {
        return Err(LlmError::validation("prompt must not be empty"));
    }
    Ok(())
}

fn log_generation_request_submission(request: &GenerationRequest) {
    let prompt_chars = request.prompt.chars().count();
    if helper_debug_prompt_log_enabled() {
        let preview = prompt_preview(&request.prompt, DEBUG_PROMPT_PREVIEW_CHARS);
        eprintln!(
            "sonant-helper: submitting request_id={} prompt_chars={} prompt_preview={:?}",
            request.request_id, prompt_chars, preview
        );
    } else {
        eprintln!(
            "sonant-helper: submitting request_id={} prompt_chars={}",
            request.request_id, prompt_chars
        );
    }
}

fn helper_debug_prompt_log_enabled() -> bool {
    std::env::var(DEBUG_PROMPT_LOG_ENV)
        .ok()
        .as_deref()
        .is_some_and(parse_truthy_flag)
}

fn parse_truthy_flag(raw: &str) -> bool {
    raw.eq_ignore_ascii_case("1")
        || raw.eq_ignore_ascii_case("true")
        || raw.eq_ignore_ascii_case("yes")
        || raw.eq_ignore_ascii_case("on")
}

fn prompt_preview(prompt: &str, max_chars: usize) -> String {
    let mut chars = prompt.chars();
    let mut preview: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        preview.push_str("...");
    }
    preview
}

fn dropped_path_to_load(paths: &ExternalPaths) -> Option<String> {
    choose_dropped_midi_path(paths.paths()).map(|path| path.to_string_lossy().to_string())
}

fn choose_dropped_midi_path(paths: &[PathBuf]) -> Option<PathBuf> {
    paths
        .iter()
        .find(|path| has_supported_midi_extension(path))
        .cloned()
        .or_else(|| paths.first().cloned())
}

fn display_file_name_from_path(path: &str) -> String {
    Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or(path)
        .to_string()
}

fn normalize_api_key_input(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        PromptSubmissionModel, build_generation_request_with_prompt_validation,
        choose_dropped_midi_path, display_file_name_from_path, has_supported_midi_extension,
        normalize_api_key_input, parse_truthy_flag, prompt_preview, validate_prompt_input,
    };
    use sonant::domain::{
        FileReferenceInput, GenerationMode, LlmError, MidiReferenceEvent, MidiReferenceSummary,
        ModelRef, ReferenceSlot, ReferenceSource,
    };
    use std::path::{Path, PathBuf};

    fn test_model() -> ModelRef {
        ModelRef {
            provider: "anthropic".to_string(),
            model: "claude-3-5-sonnet".to_string(),
        }
    }

    fn test_reference(path: &str) -> MidiReferenceSummary {
        MidiReferenceSummary {
            slot: ReferenceSlot::Melody,
            source: ReferenceSource::File,
            file: Some(FileReferenceInput {
                path: path.to_string(),
            }),
            bars: 4,
            note_count: 16,
            density_hint: 0.5,
            min_pitch: 60,
            max_pitch: 72,
            events: vec![MidiReferenceEvent {
                track: 0,
                absolute_tick: 0,
                delta_tick: 0,
                event: "NoteOn channel=0 key=60 vel=100".to_string(),
            }],
        }
    }

    #[test]
    fn validate_prompt_input_rejects_empty_input() {
        assert!(validate_prompt_input("").is_err());
    }

    #[test]
    fn validate_prompt_input_rejects_whitespace_only_input() {
        assert!(validate_prompt_input(" \n\t   ").is_err());
    }

    #[test]
    fn build_generation_request_reflects_prompt_text() {
        let prompt = "  warm synth melody with syncopation  ".to_string();
        let request = build_generation_request_with_prompt_validation(
            "req-1".to_string(),
            test_model(),
            GenerationMode::Melody,
            prompt.clone(),
            Vec::new(),
        )
        .expect("request should be built for non-empty prompt");

        assert_eq!(request.prompt, prompt);
    }

    #[test]
    fn submission_model_prepares_requests_with_incrementing_ids() {
        let mut model = PromptSubmissionModel::new(test_model());

        let first = model
            .prepare_request("first prompt".to_string(), Vec::new())
            .expect("first prompt should be accepted");
        let second = model
            .prepare_request("second prompt".to_string(), Vec::new())
            .expect("second prompt should be accepted");

        assert_eq!(first.request_id, "gpui-helper-req-1");
        assert_eq!(second.request_id, "gpui-helper-req-2");
        assert_eq!(first.prompt, "first prompt");
        assert_eq!(second.prompt, "second prompt");
    }

    #[test]
    fn submission_model_injects_references_into_request() {
        let mut model = PromptSubmissionModel::new(test_model());
        let references = vec![test_reference("/tmp/reference.mid")];

        let request = model
            .prepare_request("continue this".to_string(), references.clone())
            .expect("request should be prepared");

        assert_eq!(request.references, references);
    }

    #[test]
    fn continuation_validation_requires_reference_after_conversion() {
        let request = build_generation_request_with_prompt_validation(
            "req-cont".to_string(),
            test_model(),
            GenerationMode::Continuation,
            "continue this phrase".to_string(),
            Vec::new(),
        )
        .expect("request construction should succeed before full validation");

        assert!(matches!(
            request.validate(),
            Err(LlmError::Validation { message })
            if message == "continuation mode requires at least one MIDI reference"
        ));
    }

    #[test]
    fn continuation_validation_accepts_reference_after_conversion() {
        let request = build_generation_request_with_prompt_validation(
            "req-cont".to_string(),
            test_model(),
            GenerationMode::Continuation,
            "continue this phrase".to_string(),
            vec![test_reference("/tmp/continuation_seed.mid")],
        )
        .expect("request construction should succeed");

        assert!(request.validate().is_ok());
    }

    #[test]
    fn normalize_api_key_input_trims_and_rejects_empty() {
        assert_eq!(
            normalize_api_key_input("  sk-test-key  "),
            Some("sk-test-key".to_string())
        );
        assert_eq!(normalize_api_key_input(" \n\t "), None);
    }

    #[test]
    fn parse_truthy_flag_accepts_expected_values() {
        assert!(parse_truthy_flag("1"));
        assert!(parse_truthy_flag("true"));
        assert!(parse_truthy_flag("YES"));
        assert!(parse_truthy_flag("On"));
        assert!(!parse_truthy_flag("0"));
        assert!(!parse_truthy_flag("false"));
    }

    #[test]
    fn prompt_preview_truncates_long_prompts() {
        assert_eq!(prompt_preview("abcdef", 4), "abcd...");
        assert_eq!(prompt_preview("abc", 4), "abc");
    }

    #[test]
    fn supported_midi_extension_is_case_insensitive() {
        assert!(has_supported_midi_extension(Path::new("/tmp/input.mid")));
        assert!(has_supported_midi_extension(Path::new("/tmp/input.MIDI")));
        assert!(!has_supported_midi_extension(Path::new("/tmp/input.wav")));
    }

    #[test]
    fn dropped_path_selection_prefers_supported_midi_file() {
        let selected = choose_dropped_midi_path(&[
            PathBuf::from("/tmp/ignore.txt"),
            PathBuf::from("/tmp/melody.mid"),
        ])
        .expect("a candidate path should be selected");
        assert_eq!(selected, PathBuf::from("/tmp/melody.mid"));
    }

    #[test]
    fn dropped_path_selection_falls_back_to_first_when_no_midi_found() {
        let selected = choose_dropped_midi_path(&[
            PathBuf::from("/tmp/data.txt"),
            PathBuf::from("/tmp/other.wav"),
        ])
        .expect("a candidate path should be selected");
        assert_eq!(selected, PathBuf::from("/tmp/data.txt"));
    }

    #[test]
    fn dropped_path_selection_returns_none_for_empty_input() {
        let selected = choose_dropped_midi_path(&[]);
        assert!(selected.is_none());
    }

    #[test]
    fn display_file_name_falls_back_when_no_name_exists() {
        assert_eq!(display_file_name_from_path("/tmp/melody.mid"), "melody.mid");
        assert_eq!(display_file_name_from_path("melody.mid"), "melody.mid");
        assert_eq!(display_file_name_from_path("/tmp/"), "tmp");
    }
}
