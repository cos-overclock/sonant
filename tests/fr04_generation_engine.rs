use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

use mockito::{Matcher, Server};
use serde_json::json;
use sonant::app::{GenerationRetryConfig, GenerationService};
use sonant::domain::{
    GeneratedNote, GenerationCandidate, GenerationMetadata, GenerationMode, GenerationParams,
    GenerationRequest, GenerationResult, GenerationUsage, LlmError, ModelRef,
};
use sonant::infra::llm::schema_validator::LlmResponseSchemaValidator;
use sonant::infra::llm::{
    AnthropicProvider, LlmProvider, OpenAiCompatibleProvider, ProviderRegistry,
};

fn valid_request(provider: &str, model: &str) -> GenerationRequest {
    GenerationRequest {
        request_id: "req-1".to_string(),
        model: ModelRef {
            provider: provider.to_string(),
            model: model.to_string(),
        },
        mode: GenerationMode::Melody,
        prompt: "warm synth melody".to_string(),
        params: GenerationParams {
            bpm: 120,
            key: "C".to_string(),
            scale: "major".to_string(),
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
            score_hint: Some(0.8),
        }],
        metadata: GenerationMetadata::default(),
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

struct DummyProvider {
    provider_id: &'static str,
    model_id: &'static str,
}

impl LlmProvider for DummyProvider {
    fn provider_id(&self) -> &str {
        self.provider_id
    }

    fn supports_model(&self, model_id: &str) -> bool {
        model_id == self.model_id
    }

    fn generate(&self, request: &GenerationRequest) -> Result<GenerationResult, LlmError> {
        Ok(valid_result(request))
    }
}

struct FlakyProvider {
    calls: Arc<AtomicUsize>,
    failures_before_success: usize,
}

impl LlmProvider for FlakyProvider {
    fn provider_id(&self) -> &str {
        "anthropic"
    }

    fn supports_model(&self, model_id: &str) -> bool {
        model_id == "claude-3-5-sonnet"
    }

    fn generate(&self, request: &GenerationRequest) -> Result<GenerationResult, LlmError> {
        let attempt = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        if attempt <= self.failures_before_success {
            return Err(LlmError::Timeout);
        }
        Ok(valid_result(request))
    }
}

struct AlwaysTimeoutProvider {
    calls: Arc<AtomicUsize>,
}

impl LlmProvider for AlwaysTimeoutProvider {
    fn provider_id(&self) -> &str {
        "anthropic"
    }

    fn supports_model(&self, model_id: &str) -> bool {
        model_id == "claude-3-5-sonnet"
    }

    fn generate(&self, _request: &GenerationRequest) -> Result<GenerationResult, LlmError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(LlmError::Timeout)
    }
}

#[test]
fn schema_contract_accepts_valid_generation_result_payload() {
    let validator = LlmResponseSchemaValidator::new().expect("schema should compile");

    let result = validator
        .validate_response_json(&generation_result_json(
            "anthropic",
            "claude-3-5-sonnet",
            "req-1",
        ))
        .expect("valid payload should satisfy schema contract");

    assert_eq!(result.request_id, "req-1");
    assert_eq!(result.candidates.len(), 1);
}

#[test]
fn schema_contract_rejects_unknown_top_level_property() {
    let validator = LlmResponseSchemaValidator::new().expect("schema should compile");
    let payload = json!({
        "request_id": "req-1",
        "model": {
            "provider": "anthropic",
            "model": "claude-3-5-sonnet"
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
                        "velocity": 96
                    }
                ]
            }
        ],
        "unexpected": true
    })
    .to_string();

    let error = validator
        .validate_response_json(&payload)
        .expect_err("additionalProperties=false should reject unknown fields");

    assert!(matches!(error, LlmError::InvalidResponse { .. }));
}

#[test]
fn provider_registry_resolves_registered_provider_for_model() {
    let request = valid_request("anthropic", "claude-3-5-sonnet");
    let mut registry = ProviderRegistry::new();
    registry
        .register(DummyProvider {
            provider_id: "anthropic",
            model_id: "claude-3-5-sonnet",
        })
        .expect("provider registration should succeed");

    let provider = registry
        .resolve("anthropic", "claude-3-5-sonnet")
        .expect("provider should resolve");
    let result = provider
        .generate(&request)
        .expect("resolved provider should generate");

    assert_eq!(result.request_id, "req-1");
}

#[test]
fn anthropic_generate_succeeds_through_http_mock() {
    let mut server = Server::new();
    let response_body = json!({
        "id": "msg_01",
        "stop_reason": "end_turn",
        "usage": {
            "input_tokens": 12,
            "output_tokens": 8
        },
        "content": [
            {
                "type": "text",
                "text": generation_result_json("anthropic", "claude-3-5-sonnet", "req-1")
            }
        ]
    })
    .to_string();

    let mock = server
        .mock("POST", "/v1/messages")
        .match_header("x-api-key", "test-key")
        .match_header("anthropic-version", "2023-06-01")
        .match_header(
            "content-type",
            Matcher::Regex("application/json.*".to_string()),
        )
        .match_body(Matcher::Regex(
            "\"model\"\\s*:\\s*\"claude-3-5-sonnet\"".to_string(),
        ))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_header("request-id", "anthropic-req-1")
        .with_body(response_body)
        .create();

    let provider = AnthropicProvider::with_config("test-key", server.url(), Duration::from_secs(2))
        .expect("provider should build");
    let request = valid_request("anthropic", "claude-3-5-sonnet");

    let result = provider
        .generate(&request)
        .expect("mocked anthropic response should parse");

    mock.assert();
    assert_eq!(result.request_id, "req-1");
    assert_eq!(result.metadata.stop_reason.as_deref(), Some("end_turn"));
    assert_eq!(
        result.metadata.provider_request_id.as_deref(),
        Some("anthropic-req-1")
    );
    assert_eq!(
        result.metadata.usage,
        Some(GenerationUsage {
            input_tokens: Some(12),
            output_tokens: Some(8),
            total_tokens: Some(20),
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
        })
    );
}

#[test]
fn anthropic_generate_maps_rate_limit_http_error() {
    let mut server = Server::new();
    let mock = server
        .mock("POST", "/v1/messages")
        .with_status(429)
        .with_header("content-type", "application/json")
        .with_body(r#"{"error":{"type":"rate_limit_error","message":"slow down"}}"#)
        .create();

    let provider = AnthropicProvider::with_config("test-key", server.url(), Duration::from_secs(2))
        .expect("provider should build");
    let request = valid_request("anthropic", "claude-3-5-sonnet");

    let error = provider
        .generate(&request)
        .expect_err("429 should map to rate-limited error");

    mock.assert();
    assert!(matches!(error, LlmError::RateLimited));
}

#[test]
fn openai_compatible_generate_succeeds_through_http_mock() {
    let mut server = Server::new();
    let response_body = json!({
        "id": "chatcmpl_01",
        "choices": [
            {
                "finish_reason": "stop",
                "message": {
                    "content": generation_result_json("openai_compatible", "gpt-5.2", "req-1")
                }
            }
        ],
        "usage": {
            "prompt_tokens": 30,
            "completion_tokens": 11
        }
    })
    .to_string();

    let mock = server
        .mock("POST", "/v1/chat/completions")
        .match_header("authorization", "Bearer test-key")
        .match_header(
            "content-type",
            Matcher::Regex("application/json.*".to_string()),
        )
        .match_body(Matcher::Regex("\"model\"\\s*:\\s*\"gpt-5.2\"".to_string()))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_header("x-request-id", "openai-req-1")
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
    let request = valid_request("openai_compatible", "gpt-5.2");

    let result = provider
        .generate(&request)
        .expect("mocked openai-compatible response should parse");

    mock.assert();
    assert_eq!(result.request_id, "req-1");
    assert_eq!(result.metadata.stop_reason.as_deref(), Some("stop"));
    assert_eq!(
        result.metadata.provider_request_id.as_deref(),
        Some("openai-req-1")
    );
    assert_eq!(
        result.metadata.usage,
        Some(GenerationUsage {
            input_tokens: Some(30),
            output_tokens: Some(11),
            total_tokens: Some(41),
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
        })
    );
}

#[test]
fn openai_compatible_generate_maps_timeout_http_error() {
    let mut server = Server::new();
    let mock = server
        .mock("POST", "/v1/chat/completions")
        .with_status(408)
        .with_header("content-type", "application/json")
        .with_body(
            r#"{"error":{"type":"server_timeout","code":"request_timeout","message":"timed out"}}"#,
        )
        .create();

    let provider = OpenAiCompatibleProvider::with_config(
        "openai_compatible",
        "test-key",
        server.url(),
        Duration::from_secs(2),
        vec!["gpt-5.2".to_string()],
    )
    .expect("provider should build");
    let request = valid_request("openai_compatible", "gpt-5.2");

    let error = provider
        .generate(&request)
        .expect_err("timeout status should map to timeout error");

    mock.assert();
    assert!(matches!(error, LlmError::Timeout));
}

#[test]
fn generation_service_retries_retryable_errors_until_success() {
    let calls = Arc::new(AtomicUsize::new(0));
    let provider = Arc::new(FlakyProvider {
        calls: Arc::clone(&calls),
        failures_before_success: 2,
    });

    let mut registry = ProviderRegistry::new();
    registry
        .register_shared(provider)
        .expect("provider registration should succeed");

    let service = GenerationService::with_retry_config(
        registry,
        GenerationRetryConfig {
            max_attempts: 3,
            initial_backoff: Duration::from_millis(1),
            max_backoff: Duration::from_millis(2),
        },
    )
    .expect("retry config should be valid");

    let result = service
        .generate(valid_request("anthropic", "claude-3-5-sonnet"))
        .expect("generation should succeed after retries");

    assert_eq!(result.request_id, "req-1");
    assert_eq!(calls.load(Ordering::SeqCst), 3);
}

#[test]
fn generation_service_cancels_during_retry_backoff() {
    let calls = Arc::new(AtomicUsize::new(0));
    let provider = Arc::new(AlwaysTimeoutProvider {
        calls: Arc::clone(&calls),
    });

    let mut registry = ProviderRegistry::new();
    registry
        .register_shared(provider)
        .expect("provider registration should succeed");

    let service = GenerationService::with_retry_config(
        registry,
        GenerationRetryConfig {
            max_attempts: 5,
            initial_backoff: Duration::from_millis(200),
            max_backoff: Duration::from_millis(200),
        },
    )
    .expect("retry config should be valid");

    let cancelled = Arc::new(AtomicBool::new(false));
    let cancelled_for_thread = Arc::clone(&cancelled);
    let control_thread = thread::spawn(move || {
        thread::sleep(Duration::from_millis(20));
        cancelled_for_thread.store(true, Ordering::SeqCst);
    });

    let error = service
        .generate_with_cancel(valid_request("anthropic", "claude-3-5-sonnet"), || {
            cancelled.load(Ordering::SeqCst)
        })
        .expect_err("cancel flag should stop retry backoff");

    control_thread
        .join()
        .expect("control thread should join cleanly");
    assert!(matches!(
        error,
        LlmError::Internal { message } if message == "generation cancelled"
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}
