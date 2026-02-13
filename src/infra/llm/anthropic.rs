use std::time::{Duration, Instant};

use reqwest::StatusCode;
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};

use crate::domain::{
    GenerationMetadata, GenerationRequest, GenerationResult, GenerationUsage, LlmError,
};

use super::LlmProvider;
use super::response_parsing::{extract_json_payload, truncate_message};
use super::schema_validator::{GENERATION_RESULT_JSON_SCHEMA, LlmResponseSchemaValidator};

const PROVIDER_ID: &str = "anthropic";
const API_VERSION: &str = "2023-06-01";
const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(8);
const DEFAULT_MAX_TOKENS: u16 = 1024;
const ENV_API_KEY: &str = "SONANT_ANTHROPIC_API_KEY";
const ENV_API_KEY_FALLBACK: &str = "ANTHROPIC_API_KEY";
const ENV_BASE_URL: &str = "SONANT_ANTHROPIC_BASE_URL";
const ENV_TIMEOUT_SECS: &str = "SONANT_ANTHROPIC_TIMEOUT_SECS";
const ENV_GLOBAL_TIMEOUT_SECS: &str = "SONANT_LLM_TIMEOUT_SECS";

pub struct AnthropicProvider {
    api_key: String,
    api_base_url: String,
    client: Client,
    schema_validator: LlmResponseSchemaValidator,
}

impl AnthropicProvider {
    pub fn from_api_key(api_key: impl Into<String>) -> Result<Self, LlmError> {
        Self::with_config(api_key, DEFAULT_BASE_URL, DEFAULT_TIMEOUT)
    }

    pub fn from_env() -> Result<Self, LlmError> {
        let api_key = read_env_var(ENV_API_KEY)?
            .or(read_env_var(ENV_API_KEY_FALLBACK)?)
            .ok_or_else(|| {
                LlmError::validation(
                    "Anthropic API key is missing (set SONANT_ANTHROPIC_API_KEY or ANTHROPIC_API_KEY)",
                )
            })?;
        let api_base_url = read_env_var(ENV_BASE_URL)?.unwrap_or_else(|| DEFAULT_BASE_URL.into());
        let timeout = read_timeout_from_env(ENV_TIMEOUT_SECS)?
            .or(read_timeout_from_env(ENV_GLOBAL_TIMEOUT_SECS)?)
            .unwrap_or(DEFAULT_TIMEOUT);
        Self::with_config(api_key, api_base_url, timeout)
    }

    pub fn with_config(
        api_key: impl Into<String>,
        api_base_url: impl Into<String>,
        timeout: Duration,
    ) -> Result<Self, LlmError> {
        let api_key = api_key.into();
        if api_key.trim().is_empty() {
            return Err(LlmError::validation("Anthropic API key must not be empty"));
        }

        let api_base_url = api_base_url.into();
        if api_base_url.trim().is_empty() {
            return Err(LlmError::validation(
                "Anthropic API base URL must not be empty",
            ));
        }

        let client = Client::builder().timeout(timeout).build().map_err(|err| {
            LlmError::internal(format!("failed to create Anthropic HTTP client: {err}"))
        })?;
        let schema_validator = LlmResponseSchemaValidator::new()?;

        Ok(Self {
            api_key,
            api_base_url,
            client,
            schema_validator,
        })
    }

    fn endpoint_url(&self) -> String {
        format!("{}/v1/messages", self.api_base_url.trim_end_matches('/'))
    }

    fn build_request_payload(
        &self,
        request: &GenerationRequest,
    ) -> Result<AnthropicMessagesRequest, LlmError> {
        let request_json = serde_json::to_string_pretty(request).map_err(|err| {
            LlmError::internal(format!("failed to serialize request prompt: {err}"))
        })?;
        let user_content = format!(
            "Input GenerationRequest (JSON):\n{request_json}\n\nReturn only a JSON object that matches the schema below.\nDo not include markdown fences.\nSchema:\n{GENERATION_RESULT_JSON_SCHEMA}"
        );

        Ok(AnthropicMessagesRequest {
            model: request.model.model.clone(),
            max_tokens: request.params.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS),
            temperature: request.params.temperature,
            top_p: request.params.top_p,
            system: "You are Sonant's generation backend. Output must be strict JSON only."
                .to_string(),
            messages: vec![AnthropicMessage {
                role: "user".to_string(),
                content: user_content,
            }],
        })
    }

    fn map_success_response(
        &self,
        request: &GenerationRequest,
        response_body: &str,
        latency_ms: u64,
        header_request_id: Option<String>,
    ) -> Result<GenerationResult, LlmError> {
        let response: AnthropicMessagesResponse =
            serde_json::from_str(response_body).map_err(|err| {
                LlmError::invalid_response(format!("Anthropic response decode failed: {err}"))
            })?;

        let joined_text = response
            .content
            .iter()
            .filter_map(AnthropicContentBlock::as_text)
            .collect::<Vec<_>>()
            .join("");
        if joined_text.trim().is_empty() {
            return Err(LlmError::invalid_response(
                "Anthropic response did not include a text content block",
            ));
        }

        let json_payload = extract_json_payload(&joined_text).ok_or_else(|| {
            LlmError::invalid_response("Anthropic text block did not include a JSON object")
        })?;
        let mut result = self.schema_validator.validate_response_json(json_payload)?;

        if result.request_id != request.request_id {
            return Err(LlmError::invalid_response(format!(
                "response request_id mismatch: expected '{}', got '{}'",
                request.request_id, result.request_id
            )));
        }
        if result.model.provider != request.model.provider {
            return Err(LlmError::invalid_response(format!(
                "response model.provider mismatch: expected '{}', got '{}'",
                request.model.provider, result.model.provider
            )));
        }
        if result.model.model != request.model.model {
            return Err(LlmError::invalid_response(format!(
                "response model.model mismatch: expected '{}', got '{}'",
                request.model.model, result.model.model
            )));
        }

        let usage = response.usage.and_then(map_usage);
        let provider_request_id = header_request_id.or_else(|| {
            response
                .id
                .and_then(|id| if id.trim().is_empty() { None } else { Some(id) })
        });
        let stop_reason = response.stop_reason.and_then(|reason| {
            if reason.trim().is_empty() {
                None
            } else {
                Some(reason)
            }
        });

        result.metadata = GenerationMetadata {
            latency_ms: Some(latency_ms),
            provider_request_id,
            stop_reason,
            usage,
        };

        Ok(result)
    }
}

impl LlmProvider for AnthropicProvider {
    fn provider_id(&self) -> &str {
        PROVIDER_ID
    }

    fn supports_model(&self, model_id: &str) -> bool {
        let model_id = model_id.trim();
        !model_id.is_empty() && model_id.starts_with("claude-")
    }

    fn generate(&self, request: &GenerationRequest) -> Result<GenerationResult, LlmError> {
        let payload = self.build_request_payload(request)?;
        let started = Instant::now();

        let response = self
            .client
            .post(self.endpoint_url())
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", API_VERSION)
            .header("content-type", "application/json")
            .json(&payload)
            .send()
            .map_err(map_transport_error)?;

        let status = response.status();
        let header_request_id = response
            .headers()
            .get("request-id")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);

        let response_body = response.text().map_err(map_transport_error)?;
        if !status.is_success() {
            return Err(map_http_error(status, &response_body));
        }

        let elapsed_ms = started.elapsed().as_millis();
        let latency_ms = u64::try_from(elapsed_ms).unwrap_or(u64::MAX);
        self.map_success_response(request, &response_body, latency_ms, header_request_id)
    }
}

#[derive(Debug, Serialize)]
struct AnthropicMessagesRequest {
    model: String,
    max_tokens: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f32>,
    system: String,
    messages: Vec<AnthropicMessage>,
}

#[derive(Debug, Serialize)]
struct AnthropicMessage {
    role: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct AnthropicMessagesResponse {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    stop_reason: Option<String>,
    #[serde(default)]
    usage: Option<AnthropicUsage>,
    #[serde(default)]
    content: Vec<AnthropicContentBlock>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AnthropicContentBlock {
    Text {
        text: String,
    },
    #[serde(other)]
    Other,
}

impl AnthropicContentBlock {
    fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text { text } => Some(text),
            Self::Other => None,
        }
    }
}

#[derive(Debug, Deserialize)]
struct AnthropicUsage {
    #[serde(default)]
    input_tokens: Option<u32>,
    #[serde(default)]
    output_tokens: Option<u32>,
    #[serde(default)]
    cache_creation_input_tokens: Option<u32>,
    #[serde(default)]
    cache_read_input_tokens: Option<u32>,
}

fn map_usage(usage: AnthropicUsage) -> Option<GenerationUsage> {
    let total_tokens = match (usage.input_tokens, usage.output_tokens) {
        (Some(input), Some(output)) => input.checked_add(output),
        _ => None,
    };

    let mapped = GenerationUsage {
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        total_tokens,
        cache_creation_input_tokens: usage.cache_creation_input_tokens,
        cache_read_input_tokens: usage.cache_read_input_tokens,
    };

    if mapped.input_tokens.is_some()
        || mapped.output_tokens.is_some()
        || mapped.total_tokens.is_some()
        || mapped.cache_creation_input_tokens.is_some()
        || mapped.cache_read_input_tokens.is_some()
    {
        Some(mapped)
    } else {
        None
    }
}

fn map_http_error(status: StatusCode, body: &str) -> LlmError {
    let parsed_error = serde_json::from_str::<AnthropicErrorEnvelope>(body).ok();
    let error_type = parsed_error
        .as_ref()
        .and_then(|envelope| envelope.error.as_ref())
        .map(|detail| detail.error_type.as_str());

    if matches!(
        error_type,
        Some("authentication_error" | "invalid_api_key_error")
    ) || status == StatusCode::UNAUTHORIZED
        || status == StatusCode::FORBIDDEN
    {
        return LlmError::Auth;
    }
    if matches!(error_type, Some("rate_limit_error")) || status == StatusCode::TOO_MANY_REQUESTS {
        return LlmError::RateLimited;
    }
    if matches!(error_type, Some("timeout_error"))
        || status == StatusCode::REQUEST_TIMEOUT
        || status == StatusCode::GATEWAY_TIMEOUT
    {
        return LlmError::Timeout;
    }

    let message = parsed_error
        .as_ref()
        .and_then(|envelope| envelope.error.as_ref())
        .map(|detail| detail.message.clone())
        .unwrap_or_else(|| truncate_message(body));
    LlmError::Transport {
        message: format!("Anthropic API returned HTTP {status}: {message}"),
    }
}

fn map_transport_error(error: reqwest::Error) -> LlmError {
    if error.is_timeout() {
        return LlmError::Timeout;
    }
    LlmError::Transport {
        message: format!("Anthropic transport error: {error}"),
    }
}

#[derive(Debug, Deserialize)]
struct AnthropicErrorEnvelope {
    #[serde(default)]
    error: Option<AnthropicErrorDetail>,
}

#[derive(Debug, Deserialize)]
struct AnthropicErrorDetail {
    #[serde(rename = "type")]
    error_type: String,
    message: String,
}

fn read_env_var(name: &str) -> Result<Option<String>, LlmError> {
    match std::env::var(name) {
        Ok(value) => Ok(Some(value)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(error) => Err(LlmError::validation(format!(
            "{name} could not be read: {error}"
        ))),
    }
}

fn parse_timeout_seconds(name: &str, value: &str) -> Result<Duration, LlmError> {
    let parsed = value.trim().parse::<u64>().map_err(|_| {
        LlmError::validation(format!("{name} must be a positive integer in seconds"))
    })?;
    if parsed == 0 {
        return Err(LlmError::validation(format!(
            "{name} must be greater than 0 seconds"
        )));
    }
    Ok(Duration::from_secs(parsed))
}

fn read_timeout_from_env(name: &str) -> Result<Option<Duration>, LlmError> {
    let Some(value) = read_env_var(name)? else {
        return Ok(None);
    };
    Ok(Some(parse_timeout_seconds(name, &value)?))
}

#[cfg(test)]
mod tests {
    use super::{AnthropicProvider, map_http_error, parse_timeout_seconds};
    use crate::domain::{
        GenerationMode, GenerationParams, GenerationRequest, LlmError, MidiReferenceSummary,
        ModelRef, ReferenceSource,
    };
    use reqwest::StatusCode;
    use std::time::Duration;

    fn provider() -> AnthropicProvider {
        AnthropicProvider::with_config(
            "test-key",
            "https://api.anthropic.com",
            Duration::from_secs(2),
        )
        .expect("provider should build")
    }

    fn request() -> GenerationRequest {
        GenerationRequest {
            request_id: "req-42".to_string(),
            model: ModelRef {
                provider: "anthropic".to_string(),
                model: "claude-3-5-sonnet".to_string(),
            },
            mode: GenerationMode::Melody,
            prompt: "warm synth melody".to_string(),
            params: GenerationParams {
                bpm: 122,
                key: "C".to_string(),
                scale: "major".to_string(),
                density: 3,
                complexity: 2,
                temperature: Some(0.5),
                top_p: Some(0.9),
                max_tokens: Some(512),
            },
            references: vec![MidiReferenceSummary {
                slot: "melody".to_string(),
                source: ReferenceSource::File,
                bars: 4,
                note_count: 24,
                density_hint: 0.42,
                min_pitch: 60,
                max_pitch: 74,
            }],
            variation_count: 2,
        }
    }

    #[test]
    fn build_request_payload_maps_generation_request() {
        let payload = provider()
            .build_request_payload(&request())
            .expect("payload should be built");

        assert_eq!(payload.model, "claude-3-5-sonnet");
        assert_eq!(payload.max_tokens, 512);
        assert_eq!(payload.temperature, Some(0.5));
        assert_eq!(payload.top_p, Some(0.9));
        assert_eq!(payload.messages.len(), 1);
        assert!(
            payload.messages[0]
                .content
                .contains("\"request_id\": \"req-42\"")
        );
        assert!(
            payload.messages[0]
                .content
                .contains("\"variation_count\": 2")
        );
    }

    #[test]
    fn map_success_response_extracts_result_and_metadata() {
        let response = r#"{
          "id": "msg_01",
          "stop_reason": "end_turn",
          "usage": {
            "input_tokens": 110,
            "output_tokens": 35,
            "cache_creation_input_tokens": 0,
            "cache_read_input_tokens": 10
          },
          "content": [
            {
              "type": "text",
              "text": "```json\n{\n  \"request_id\": \"req-42\",\n  \"model\": {\n    \"provider\": \"anthropic\",\n    \"model\": \"claude-3-5-sonnet\"\n  },\n  \"candidates\": [\n    {\n      \"id\": \"cand-1\",\n      \"bars\": 4,\n      \"notes\": [\n        {\n          \"pitch\": 60,\n          \"start_tick\": 0,\n          \"duration_tick\": 240,\n          \"velocity\": 96,\n          \"channel\": 1\n        }\n      ]\n    }\n  ]\n}\n```"
            }
          ]
        }"#;

        let result = provider()
            .map_success_response(&request(), response, 640, Some("req_hdr".to_string()))
            .expect("response mapping should succeed");

        assert_eq!(result.request_id, "req-42");
        assert_eq!(result.candidates.len(), 1);
        assert_eq!(result.metadata.latency_ms, Some(640));
        assert_eq!(
            result.metadata.provider_request_id.as_deref(),
            Some("req_hdr")
        );
        assert_eq!(result.metadata.stop_reason.as_deref(), Some("end_turn"));
        assert_eq!(
            result
                .metadata
                .usage
                .as_ref()
                .and_then(|usage| usage.total_tokens),
            Some(145)
        );
    }

    #[test]
    fn map_success_response_accepts_json_split_across_multiple_text_blocks() {
        let response = r#"{
          "id": "msg_01",
          "content": [
            {
              "type": "text",
              "text": "{\"request_id\":\"req-42\",\"model\":{\"provider\":\"anthropic\",\"model\":\"claude-3-5-sonnet\"},\"candidates\":["
            },
            {
              "type": "text",
              "text": "{\"id\":\"cand-1\",\"bars\":4,\"notes\":[{\"pitch\":60,\"start_tick\":0,\"duration_tick\":240,\"velocity\":96}]}"
            },
            {
              "type": "text",
              "text": "]}"
            }
          ]
        }"#;

        let result = provider()
            .map_success_response(&request(), response, 25, None)
            .expect("split text blocks should still parse");

        assert_eq!(result.request_id, "req-42");
        assert_eq!(result.candidates.len(), 1);
        assert_eq!(result.candidates[0].id, "cand-1");
        assert_eq!(result.metadata.latency_ms, Some(25));
    }

    #[test]
    fn map_success_response_rejects_request_id_mismatch() {
        let response = r#"{
          "id": "msg_01",
          "content": [
            {
              "type": "text",
              "text": "{\"request_id\":\"req-other\",\"model\":{\"provider\":\"anthropic\",\"model\":\"claude-3-5-sonnet\"},\"candidates\":[{\"id\":\"cand-1\",\"bars\":4,\"notes\":[{\"pitch\":60,\"start_tick\":0,\"duration_tick\":240,\"velocity\":96}]}]}"
            }
          ]
        }"#;

        let error = provider()
            .map_success_response(&request(), response, 10, None)
            .expect_err("request id mismatch should fail");

        assert!(matches!(
            error,
            LlmError::InvalidResponse { message }
            if message == "response request_id mismatch: expected 'req-42', got 'req-other'"
        ));
    }

    #[test]
    fn map_http_error_maps_status_and_error_type() {
        let auth = map_http_error(
            StatusCode::UNAUTHORIZED,
            r#"{"error":{"type":"authentication_error","message":"invalid key"}}"#,
        );
        let rate_limited = map_http_error(
            StatusCode::TOO_MANY_REQUESTS,
            r#"{"error":{"type":"rate_limit_error","message":"slow down"}}"#,
        );
        let timeout = map_http_error(
            StatusCode::GATEWAY_TIMEOUT,
            r#"{"error":{"type":"timeout_error","message":"timed out"}}"#,
        );

        assert!(matches!(auth, LlmError::Auth));
        assert!(matches!(rate_limited, LlmError::RateLimited));
        assert!(matches!(timeout, LlmError::Timeout));
    }

    #[test]
    fn parse_timeout_seconds_accepts_positive_integer_values() {
        let timeout = parse_timeout_seconds("TEST_TIMEOUT", "8")
            .expect("positive integer timeout should parse");
        assert_eq!(timeout, Duration::from_secs(8));
    }

    #[test]
    fn parse_timeout_seconds_rejects_invalid_values() {
        let zero = parse_timeout_seconds("TEST_TIMEOUT", "0")
            .expect_err("zero timeout should fail validation");
        assert!(matches!(
            zero,
            LlmError::Validation { message }
            if message == "TEST_TIMEOUT must be greater than 0 seconds"
        ));

        let invalid = parse_timeout_seconds("TEST_TIMEOUT", "abc")
            .expect_err("non-integer timeout should fail validation");
        assert!(matches!(
            invalid,
            LlmError::Validation { message }
            if message == "TEST_TIMEOUT must be a positive integer in seconds"
        ));
    }
}
