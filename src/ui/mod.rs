use gpui::{App, AppContext, Application, Bounds, WindowBounds, WindowOptions, px, size};
use gpui_component::Root;

#[cfg(target_os = "macos")]
use cocoa::{
    appkit::{
        NSApplication, NSApplicationActivationPolicy::NSApplicationActivationPolicyAccessory,
    },
    base::nil,
};

mod backend;
mod request;
mod state;
mod theme;
mod utils;
mod window;

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
const SETTINGS_ANTHROPIC_API_KEY_PLACEHOLDER: &str = "Anthropic API key";
const SETTINGS_OPENAI_API_KEY_PLACEHOLDER: &str = "OpenAI-compatible API key";
const SETTINGS_CUSTOM_BASE_URL_PLACEHOLDER: &str = "Custom base URL (optional)";
const SETTINGS_DEFAULT_MODEL_PLACEHOLDER: &str = "Default model ID";
const SETTINGS_CONTEXT_WINDOW_PLACEHOLDER: &str = "Context window tokens";
const MIDI_SLOT_FILE_PICKER_PROMPT: &str = "Select MIDI File (.mid/.midi)";
const MIDI_SLOT_DROP_HINT: &str = "Drop a .mid/.midi file here or choose one from the dialog.";
const MIDI_SLOT_EMPTY_LABEL: &str = "Not set";
const MIDI_SLOT_DROP_ERROR_MESSAGE: &str = "Drop at least one file to set the MIDI reference.";
const MIDI_SLOT_UNSUPPORTED_FILE_MESSAGE: &str = "Only .mid or .midi files are supported.";
const DEBUG_PROMPT_LOG_ENV: &str = "SONANT_HELPER_DEBUG_PROMPT_LOG";
const DEBUG_PROMPT_PREVIEW_CHARS: usize = 120;

pub(crate) fn run_gpui_helper() {
    Application::new().run(|cx: &mut App| {
        set_plugin_helper_activation_policy();
        gpui_component::init(cx);
        theme::apply_default_theme(cx);

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
                let view = cx.new(|cx| window::SonantMainWindow::new(window, cx));
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

#[cfg(test)]
mod tests {
    use super::request::{
        PromptSubmissionModel, build_generation_request_with_prompt_validation,
        validate_prompt_input,
    };
    use super::state::{
        MidiSlotErrorState, can_retry_midi_load_error, mode_reference_requirement,
        mode_reference_requirement_satisfied,
    };
    use super::utils::{
        choose_dropped_midi_path, display_file_name_from_path, normalize_api_key_input,
        parse_truthy_flag, prompt_preview,
    };
    use sonant::app::LoadMidiError;
    use sonant::domain::{
        FileReferenceInput, GenerationMode, LlmError, MidiReferenceEvent, MidiReferenceSummary,
        ModelRef, ReferenceSlot, ReferenceSource, has_supported_midi_extension,
    };
    use sonant::infra::midi::MidiLoadError;
    use std::path::{Path, PathBuf};

    fn test_model() -> ModelRef {
        ModelRef {
            provider: "anthropic".to_string(),
            model: "claude-3-5-sonnet".to_string(),
        }
    }

    fn test_reference_with_slot(path: &str, slot: ReferenceSlot) -> MidiReferenceSummary {
        MidiReferenceSummary {
            slot,
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

    fn test_reference(path: &str) -> MidiReferenceSummary {
        test_reference_with_slot(path, ReferenceSlot::Melody)
    }

    fn test_live_reference_with_slot(slot: ReferenceSlot) -> MidiReferenceSummary {
        MidiReferenceSummary {
            slot,
            source: ReferenceSource::Live,
            file: None,
            bars: 2,
            note_count: 8,
            density_hint: 0.25,
            min_pitch: 55,
            max_pitch: 67,
            events: vec![MidiReferenceEvent {
                track: 1,
                absolute_tick: 120,
                delta_tick: 120,
                event: "LiveMidi channel=2 status=0x91 data1=55 data2=100 port=1 time=120"
                    .to_string(),
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
            .prepare_request(
                GenerationMode::Melody,
                "first prompt".to_string(),
                Vec::new(),
            )
            .expect("first prompt should be accepted");
        let second = model
            .prepare_request(
                GenerationMode::Bassline,
                "second prompt".to_string(),
                Vec::new(),
            )
            .expect("second prompt should be accepted");

        assert_eq!(first.request_id, "gpui-helper-req-1");
        assert_eq!(second.request_id, "gpui-helper-req-2");
        assert_eq!(first.prompt, "first prompt");
        assert_eq!(second.prompt, "second prompt");
        assert_eq!(first.mode, GenerationMode::Melody);
        assert_eq!(second.mode, GenerationMode::Bassline);
    }

    #[test]
    fn submission_model_preserves_all_generation_modes() {
        let mut model = PromptSubmissionModel::new(test_model());
        let modes = [
            GenerationMode::Melody,
            GenerationMode::ChordProgression,
            GenerationMode::DrumPattern,
            GenerationMode::Bassline,
            GenerationMode::CounterMelody,
            GenerationMode::Harmony,
            GenerationMode::Continuation,
        ];

        for (index, mode) in modes.into_iter().enumerate() {
            let request = model
                .prepare_request(mode, format!("prompt-{index}"), Vec::new())
                .expect("prompt should be accepted");

            assert_eq!(request.mode, mode);
        }
    }

    #[test]
    fn submission_model_injects_references_into_request() {
        let mut model = PromptSubmissionModel::new(test_model());
        let references = vec![test_reference("/tmp/reference.mid")];

        let request = model
            .prepare_request(
                GenerationMode::Continuation,
                "continue this".to_string(),
                references.clone(),
            )
            .expect("request should be prepared");

        assert_eq!(request.mode, GenerationMode::Continuation);
        assert_eq!(request.references, references);
    }

    #[test]
    fn submission_model_preserves_multiple_reference_slots_in_request() {
        let mut model = PromptSubmissionModel::new(test_model());
        let references = vec![
            test_reference_with_slot("/tmp/melody.mid", ReferenceSlot::Melody),
            test_reference_with_slot("/tmp/chords.mid", ReferenceSlot::ChordProgression),
            test_reference_with_slot("/tmp/drums.mid", ReferenceSlot::DrumPattern),
        ];

        let request = model
            .prepare_request(
                GenerationMode::Continuation,
                "continue from multiple references".to_string(),
                references.clone(),
            )
            .expect("request should preserve all reference slots");

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
    fn mode_reference_requirement_labels_cover_all_modes() {
        let cases = [
            (GenerationMode::Melody, "Reference MIDI: Optional.", None),
            (
                GenerationMode::ChordProgression,
                "Reference MIDI: Optional.",
                None,
            ),
            (
                GenerationMode::DrumPattern,
                "Reference MIDI: Optional.",
                None,
            ),
            (GenerationMode::Bassline, "Reference MIDI: Optional.", None),
            (
                GenerationMode::CounterMelody,
                "Reference MIDI required: Melody.",
                Some("Counter Melody mode requires a Melody reference MIDI before generating."),
            ),
            (
                GenerationMode::Harmony,
                "Reference MIDI required: Melody.",
                Some("Harmony mode requires a Melody reference MIDI before generating."),
            ),
            (
                GenerationMode::Continuation,
                "Reference MIDI required: At least one slot.",
                Some("Continuation mode requires at least one reference MIDI before generating."),
            ),
        ];

        for (mode, expected_description, expected_unmet_message) in cases {
            let requirement = mode_reference_requirement(mode);
            assert_eq!(
                requirement.description, expected_description,
                "unexpected requirement description for {mode:?}"
            );
            assert_eq!(
                requirement.unmet_message, expected_unmet_message,
                "unexpected requirement unmet message for {mode:?}"
            );
        }
    }

    #[test]
    fn mode_reference_requirement_satisfied_covers_mode_pass_and_fail_matrix() {
        let no_references = Vec::<MidiReferenceSummary>::new();
        let melody_reference = vec![test_reference("/tmp/melody.mid")];
        let chord_reference = vec![test_reference_with_slot(
            "/tmp/chords.mid",
            ReferenceSlot::ChordProgression,
        )];
        let melody_live_reference = vec![test_live_reference_with_slot(ReferenceSlot::Melody)];
        let chord_live_reference = vec![test_live_reference_with_slot(
            ReferenceSlot::ChordProgression,
        )];
        let mixed_references = vec![
            test_reference_with_slot("/tmp/chords.mid", ReferenceSlot::ChordProgression),
            test_live_reference_with_slot(ReferenceSlot::Melody),
        ];

        let cases = [
            (GenerationMode::Melody, &no_references, true),
            (GenerationMode::ChordProgression, &no_references, true),
            (GenerationMode::DrumPattern, &no_references, true),
            (GenerationMode::Bassline, &no_references, true),
            (GenerationMode::CounterMelody, &no_references, false),
            (GenerationMode::Harmony, &no_references, false),
            (GenerationMode::Continuation, &no_references, false),
            (GenerationMode::CounterMelody, &chord_reference, false),
            (GenerationMode::Harmony, &chord_reference, false),
            (GenerationMode::CounterMelody, &melody_reference, true),
            (GenerationMode::Harmony, &melody_reference, true),
            (GenerationMode::Continuation, &melody_reference, true),
            (GenerationMode::Continuation, &chord_reference, true),
            (GenerationMode::Bassline, &melody_reference, true),
            (GenerationMode::Bassline, &chord_reference, true),
            (GenerationMode::CounterMelody, &melody_live_reference, true),
            (GenerationMode::Harmony, &melody_live_reference, true),
            (GenerationMode::Continuation, &melody_live_reference, true),
            (GenerationMode::CounterMelody, &chord_live_reference, false),
            (GenerationMode::Harmony, &chord_live_reference, false),
            (GenerationMode::Continuation, &chord_live_reference, true),
            (GenerationMode::CounterMelody, &mixed_references, true),
            (GenerationMode::Harmony, &mixed_references, true),
            (GenerationMode::Continuation, &mixed_references, true),
        ];

        for (mode, references, expected) in cases {
            let actual = mode_reference_requirement_satisfied(mode, references);
            assert_eq!(
                actual, expected,
                "unexpected requirement result for {mode:?} with references {references:?}"
            );
        }
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
    fn can_retry_midi_load_error_for_io_failure() {
        let error = LoadMidiError::LoadFailed {
            source: MidiLoadError::Io {
                message: "permission denied".to_string(),
            },
        };

        assert!(can_retry_midi_load_error(&error));
    }

    #[test]
    fn can_retry_midi_load_error_is_false_for_unsupported_extension() {
        let error = LoadMidiError::LoadFailed {
            source: MidiLoadError::UnsupportedExtension {
                path: "/tmp/not-midi.wav".to_string(),
            },
        };

        assert!(!can_retry_midi_load_error(&error));
    }

    #[test]
    fn midi_slot_error_state_uses_retry_path_only_when_retryable() {
        let io_error = LoadMidiError::LoadFailed {
            source: MidiLoadError::Io {
                message: "file locked".to_string(),
            },
        };
        let io_state =
            MidiSlotErrorState::from_load_error(ReferenceSlot::Melody, "/tmp/retry.mid", &io_error);
        assert!(io_state.can_retry());
        assert_eq!(io_state.slot, ReferenceSlot::Melody);
        assert_eq!(io_state.retry_path.as_deref(), Some("/tmp/retry.mid"));

        let parse_error = LoadMidiError::LoadFailed {
            source: MidiLoadError::Parse {
                message: "invalid chunk".to_string(),
            },
        };
        let parse_state = MidiSlotErrorState::from_load_error(
            ReferenceSlot::CounterMelody,
            "/tmp/broken.mid",
            &parse_error,
        );
        assert!(parse_state.can_retry());
        assert_eq!(parse_state.slot, ReferenceSlot::CounterMelody);

        let extension_error = LoadMidiError::LoadFailed {
            source: MidiLoadError::UnsupportedExtension {
                path: "/tmp/invalid.wav".to_string(),
            },
        };
        let extension_state = MidiSlotErrorState::from_load_error(
            ReferenceSlot::Harmony,
            "/tmp/invalid.wav",
            &extension_error,
        );
        assert!(!extension_state.can_retry());
        assert_eq!(extension_state.slot, ReferenceSlot::Harmony);
        assert_eq!(extension_state.retry_path, None);
    }

    #[test]
    fn display_file_name_falls_back_when_no_name_exists() {
        assert_eq!(display_file_name_from_path("/tmp/melody.mid"), "melody.mid");
        assert_eq!(display_file_name_from_path("melody.mid"), "melody.mid");
        assert_eq!(display_file_name_from_path("/tmp/"), "tmp");
    }
}
