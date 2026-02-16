use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use mockito::{Matcher, Server};
use serde_json::json;
use sonant::app::GenerationService;
use sonant::domain::{
    FileReferenceInput, GeneratedNote, GenerationCandidate, GenerationMetadata, GenerationMode,
    GenerationParams, GenerationRequest, GenerationResult, LlmError, MidiReferenceEvent,
    MidiReferenceSummary, ModelRef, ReferenceSlot, ReferenceSource,
};
use sonant::infra::llm::{
    AnthropicProvider, LlmProvider, OpenAiCompatibleProvider, ProviderRegistry,
};

fn valid_request(provider: &str, model: &str, mode: GenerationMode) -> GenerationRequest {
    GenerationRequest {
        request_id: "req-fr05-1".to_string(),
        model: ModelRef {
            provider: provider.to_string(),
            model: model.to_string(),
        },
        mode,
        prompt: "keep energy and groove".to_string(),
        params: GenerationParams {
            bpm: 124,
            key: "D".to_string(),
            scale: "minor".to_string(),
            density: 3,
            complexity: 3,
            temperature: Some(0.7),
            top_p: Some(0.9),
            max_tokens: Some(512),
        },
        references: Vec::new(),
        variation_count: 1,
    }
}

fn sample_reference(slot: ReferenceSlot) -> MidiReferenceSummary {
    MidiReferenceSummary {
        slot,
        source: ReferenceSource::File,
        file: Some(FileReferenceInput {
            path: "refs/reference.mid".to_string(),
        }),
        bars: 4,
        note_count: 8,
        density_hint: 0.4,
        min_pitch: 48,
        max_pitch: 72,
        events: vec![MidiReferenceEvent {
            track: 0,
            absolute_tick: 0,
            delta_tick: 0,
            event: "NoteOn channel=0 key=60 vel=90".to_string(),
        }],
    }
}

fn generation_result_json(provider: &str, model: &str, request_id: &str) -> String {
    json!({
        "request_id": request_id,
        "model": {
            "provider": provider,
            "model": model
        },
        "candidates": [
            {
                "id": "cand-1",
                "bars": 4,
                "notes": [
                    {
                        "pitch": 60,
                        "start_tick": 0,
                        "duration_tick": 240,
                        "velocity": 96,
                        "channel": 1
                    }
                ]
            }
        ]
    })
    .to_string()
}

fn valid_result(request: &GenerationRequest) -> GenerationResult {
    GenerationResult {
        request_id: request.request_id.clone(),
        model: request.model.clone(),
        candidates: vec![GenerationCandidate {
            id: "cand-1".to_string(),
            bars: 4,
            notes: vec![GeneratedNote {
                pitch: 60,
                start_tick: 0,
                duration_tick: 240,
                velocity: 96,
                channel: 1,
            }],
            score_hint: None,
        }],
        metadata: GenerationMetadata::default(),
    }
}

struct CallCountingProvider {
    calls: Arc<AtomicUsize>,
}

impl LlmProvider for CallCountingProvider {
    fn provider_id(&self) -> &str {
        "anthropic"
    }

    fn supports_model(&self, model_id: &str) -> bool {
        model_id == "claude-3-5-sonnet"
    }

    fn generate(&self, request: &GenerationRequest) -> Result<GenerationResult, LlmError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(valid_result(request))
    }
}

#[test]
fn anthropic_path_reflects_selected_mode_in_provider_payload() {
    let mut server = Server::new();
    let response_body = json!({
        "id": "msg_01",
        "stop_reason": "end_turn",
        "usage": {
            "input_tokens": 8,
            "output_tokens": 6
        },
        "content": [
            {
                "type": "text",
                "text": generation_result_json("anthropic", "claude-3-5-sonnet", "req-fr05-1")
            }
        ]
    })
    .to_string();

    let mock = server
        .mock("POST", "/v1/messages")
        .match_header("x-api-key", "test-key")
        .match_body(Matcher::Regex("Generation mode: harmony".to_string()))
        .match_body(Matcher::Regex("Create a harmony line".to_string()))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(response_body)
        .create();

    let provider = AnthropicProvider::with_config("test-key", server.url(), Duration::from_secs(2))
        .expect("provider should build");
    let mut registry = ProviderRegistry::new();
    registry
        .register(provider)
        .expect("provider registration should succeed");
    let service = GenerationService::new(registry);

    let mut request = valid_request("anthropic", "claude-3-5-sonnet", GenerationMode::Harmony);
    request.references = vec![sample_reference(ReferenceSlot::Melody)];

    let result = service
        .generate(request)
        .expect("generation should pass through anthropic provider");

    mock.assert();
    assert_eq!(result.request_id, "req-fr05-1");
}

#[test]
fn openai_compatible_path_reflects_selected_mode_in_provider_payload() {
    let mut server = Server::new();
    let response_body = json!({
        "id": "chatcmpl_01",
        "choices": [
            {
                "finish_reason": "stop",
                "message": {
                    "content": generation_result_json("openai_compatible", "gpt-5.2", "req-fr05-1")
                }
            }
        ],
        "usage": {
            "prompt_tokens": 20,
            "completion_tokens": 8
        }
    })
    .to_string();

    let mock = server
        .mock("POST", "/v1/chat/completions")
        .match_header("authorization", "Bearer test-key")
        .match_body(Matcher::Regex("Generation mode: continuation".to_string()))
        .match_body(Matcher::Regex("Continue the musical idea".to_string()))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(response_body)
        .create();

    let provider = OpenAiCompatibleProvider::with_config(
        "openai_compatible",
        "test-key",
        server.url(),
        Duration::from_secs(2),
        vec!["gpt-5.2".to_string()],
    )
    .expect("provider should build");
    let mut registry = ProviderRegistry::new();
    registry
        .register(provider)
        .expect("provider registration should succeed");
    let service = GenerationService::new(registry);

    let mut request = valid_request("openai_compatible", "gpt-5.2", GenerationMode::Continuation);
    request.references = vec![sample_reference(ReferenceSlot::ContinuationSeed)];

    let result = service
        .generate(request)
        .expect("generation should pass through openai-compatible provider");

    mock.assert();
    assert_eq!(result.request_id, "req-fr05-1");
}

#[test]
fn continuation_without_references_fails_before_provider_call() {
    let calls = Arc::new(AtomicUsize::new(0));
    let provider = CallCountingProvider {
        calls: Arc::clone(&calls),
    };

    let mut registry = ProviderRegistry::new();
    registry
        .register(provider)
        .expect("provider registration should succeed");
    let service = GenerationService::new(registry);

    let request = valid_request(
        "anthropic",
        "claude-3-5-sonnet",
        GenerationMode::Continuation,
    );

    let error = service
        .generate(request)
        .expect_err("continuation without references should fail validation");

    assert!(matches!(
        error,
        LlmError::Validation { message }
        if message == "continuation mode requires at least one MIDI reference"
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}
