use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use reqwest::StatusCode;
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::domain::{
    GenerationMetadata, GenerationRequest, GenerationResult, GenerationUsage, LlmError,
};

use super::env::{read_env_var, read_timeout_from_env, resolve_timeout_with_global_fallback};
use super::response_parsing::{extract_json_payload, truncate_message};
use super::schema_validator::LlmResponseSchemaValidator;
use super::{LlmProvider, PromptBuilder};

const DEFAULT_PROVIDER_ID: &str = "openai_compatible";
const DEFAULT_BASE_URL: &str = "https://api.openai.com";
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(8);

const ENV_API_KEY: &str = "SONANT_OPENAI_COMPAT_API_KEY";
const ENV_BASE_URL: &str = "SONANT_OPENAI_COMPAT_BASE_URL";
const ENV_PROVIDER_ID: &str = "SONANT_OPENAI_COMPAT_PROVIDER_ID";
const ENV_MODELS: &str = "SONANT_OPENAI_COMPAT_MODELS";
const ENV_FETCH_MODELS: &str = "SONANT_OPENAI_COMPAT_FETCH_MODELS";
const ENV_TIMEOUT_SECS: &str = "SONANT_OPENAI_COMPAT_TIMEOUT_SECS";
const ENV_GLOBAL_TIMEOUT_SECS: &str = "SONANT_LLM_TIMEOUT_SECS";

const DEFAULT_SUPPORTED_MODELS: &[&str] = &["gpt-5.2"];

pub struct OpenAiCompatibleProvider {
    provider_id: String,
    api_key: String,
    api_base_url: String,
    client: Client,
    schema_validator: LlmResponseSchemaValidator,
    supported_models: BTreeSet<String>,
}

impl OpenAiCompatibleProvider {
    pub fn from_api_key(api_key: impl Into<String>) -> Result<Self, LlmError> {
        Self::with_config(
            DEFAULT_PROVIDER_ID,
            api_key,
            DEFAULT_BASE_URL,
            DEFAULT_TIMEOUT,
            default_supported_models(),
        )
    }

    pub fn from_env() -> Result<Self, LlmError> {
        let api_key = match read_env_var(ENV_API_KEY)? {
            Some(key) => key,
            None => read_env_var("OPENAI_API_KEY")?.ok_or_else(|| {
                LlmError::validation(
                    "OpenAI-compatible API key is missing (set SONANT_OPENAI_COMPAT_API_KEY or OPENAI_API_KEY)",
                )
            })?,
        };

        let api_base_url =
            read_env_var(ENV_BASE_URL)?.unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
        let provider_id =
            read_env_var(ENV_PROVIDER_ID)?.unwrap_or_else(|| DEFAULT_PROVIDER_ID.to_string());

        let supported_models = match read_env_var(ENV_MODELS)? {
            Some(value) => parse_supported_models(&value)?,
            None => default_supported_models(),
        };
        let provider_timeout = read_timeout_from_env(ENV_TIMEOUT_SECS)?;
        let timeout = resolve_timeout_with_global_fallback(
            provider_timeout,
            || read_timeout_from_env(ENV_GLOBAL_TIMEOUT_SECS),
            DEFAULT_TIMEOUT,
        )?;

        let mut provider = Self::with_config(
            provider_id,
            api_key,
            api_base_url,
            timeout,
            supported_models,
        )?;

        if read_bool_env(ENV_FETCH_MODELS)? {
            provider.refresh_models()?;
        }

        Ok(provider)
    }

    pub fn with_config(
        provider_id: impl Into<String>,
        api_key: impl Into<String>,
        api_base_url: impl Into<String>,
        timeout: Duration,
        supported_models: Vec<String>,
    ) -> Result<Self, LlmError> {
        let provider_id = provider_id.into();
        let provider_id = provider_id.trim();
        if provider_id.is_empty() {
            return Err(LlmError::validation(
                "OpenAI-compatible provider_id must not be empty",
            ));
        }

        let api_key = api_key.into();
        if api_key.trim().is_empty() {
            return Err(LlmError::validation(
                "OpenAI-compatible API key must not be empty",
            ));
        }

        let api_base_url = api_base_url.into();
        if api_base_url.trim().is_empty() {
            return Err(LlmError::validation(
                "OpenAI-compatible API base URL must not be empty",
            ));
        }

        let supported_models = normalize_supported_models(supported_models)?;

        let client = Client::builder().timeout(timeout).build().map_err(|err| {
            LlmError::internal(format!(
                "failed to create OpenAI-compatible HTTP client: {err}"
            ))
        })?;
        let schema_validator = LlmResponseSchemaValidator::new()?;

        Ok(Self {
            provider_id: provider_id.to_string(),
            api_key,
            api_base_url,
            client,
            schema_validator,
            supported_models,
        })
    }

    pub fn refresh_models(&mut self) -> Result<(), LlmError> {
        self.supported_models = self.fetch_supported_models()?;
        Ok(())
    }

    pub fn supported_models(&self) -> Vec<String> {
        self.supported_models.iter().cloned().collect()
    }

    fn endpoint_url(&self) -> String {
        build_v1_url(&self.api_base_url, "chat/completions")
    }

    fn models_endpoint_url(&self) -> String {
        build_v1_url(&self.api_base_url, "models")
    }

    fn fetch_supported_models(&self) -> Result<BTreeSet<String>, LlmError> {
        let response = self
            .client
            .get(self.models_endpoint_url())
            .bearer_auth(&self.api_key)
            .header("content-type", "application/json")
            .send()
            .map_err(map_transport_error)?;

        let status = response.status();
        let response_body = response.text().map_err(map_transport_error)?;
        if !status.is_success() {
            return Err(map_http_error(status, &response_body));
        }

        let decoded: OpenAiModelsResponse =
            serde_json::from_str(&response_body).map_err(|err| {
                LlmError::invalid_response(format!(
                    "OpenAI-compatible models response decode failed: {err}"
                ))
            })?;

        let models = decoded
            .data
            .into_iter()
            .map(|model| model.id)
            .collect::<Vec<_>>();

        normalize_supported_models_from_response(models)
    }

    fn build_request_payload(
        &self,
        request: &GenerationRequest,
    ) -> Result<OpenAiChatCompletionsRequest, LlmError> {
        let prompt = PromptBuilder::build(request);

        Ok(OpenAiChatCompletionsRequest {
            model: request.model.model.clone(),
            messages: vec![
                OpenAiChatMessageRequest {
                    role: "system".to_string(),
                    content: prompt.system,
                },
                OpenAiChatMessageRequest {
                    role: "user".to_string(),
                    content: prompt.user,
                },
            ],
            temperature: request.params.temperature,
            top_p: request.params.top_p,
            max_tokens: request.params.max_tokens,
        })
    }

    fn map_success_response(
        &self,
        request: &GenerationRequest,
        response_body: &str,
        latency_ms: u64,
        header_request_id: Option<String>,
    ) -> Result<GenerationResult, LlmError> {
        let response: OpenAiChatCompletionsResponse =
            serde_json::from_str(response_body).map_err(|err| {
                LlmError::invalid_response(format!(
                    "OpenAI-compatible response decode failed: {err}"
                ))
            })?;

        let mut response_text = None;
        let mut stop_reason = None;
        for choice in &response.choices {
            if let Some(text) = choice.extract_text() {
                response_text = Some(text);
                stop_reason = choice.finish_reason.as_deref().and_then(non_empty_owned);
                break;
            }
        }

        let response_text = response_text.ok_or_else(|| {
            LlmError::invalid_response("OpenAI-compatible response did not include text content")
        })?;

        let json_payload = extract_json_payload(&response_text).ok_or_else(|| {
            LlmError::invalid_response(
                "OpenAI-compatible text content did not include a JSON object",
            )
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
        let provider_request_id =
            header_request_id.or_else(|| response.id.as_deref().and_then(non_empty_owned));

        result.metadata = GenerationMetadata {
            latency_ms: Some(latency_ms),
            provider_request_id,
            stop_reason,
            usage,
        };

        Ok(result)
    }
}

impl LlmProvider for OpenAiCompatibleProvider {
    fn provider_id(&self) -> &str {
        &self.provider_id
    }

    fn supports_model(&self, model_id: &str) -> bool {
        let model_id = model_id.trim();
        !model_id.is_empty() && self.supported_models.contains(model_id)
    }

    fn generate(&self, request: &GenerationRequest) -> Result<GenerationResult, LlmError> {
        let payload = self.build_request_payload(request)?;
        let started = Instant::now();

        let response = self
            .client
            .post(self.endpoint_url())
            .bearer_auth(&self.api_key)
            .header("content-type", "application/json")
            .json(&payload)
            .send()
            .map_err(map_transport_error)?;

        let status = response.status();
        let header_request_id = response
            .headers()
            .get("x-request-id")
            .or_else(|| response.headers().get("request-id"))
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
struct OpenAiChatCompletionsRequest {
    model: String,
    messages: Vec<OpenAiChatMessageRequest>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u16>,
}

#[derive(Debug, Serialize)]
struct OpenAiChatMessageRequest {
    role: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct OpenAiChatCompletionsResponse {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    choices: Vec<OpenAiChoice>,
    #[serde(default)]
    usage: Option<OpenAiUsage>,
}

#[derive(Debug, Deserialize)]
struct OpenAiChoice {
    #[serde(default)]
    finish_reason: Option<String>,
    #[serde(default)]
    message: Option<OpenAiChoiceMessage>,
    #[serde(default)]
    text: Option<String>,
}

impl OpenAiChoice {
    fn extract_text(&self) -> Option<String> {
        if let Some(text) = self.text.as_deref().and_then(non_empty_owned) {
            return Some(text);
        }

        let content = self.message.as_ref()?.content.as_ref()?;
        extract_message_content(content)
    }
}

#[derive(Debug, Deserialize)]
struct OpenAiChoiceMessage {
    #[serde(default)]
    content: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct OpenAiUsage {
    #[serde(default)]
    prompt_tokens: Option<u32>,
    #[serde(default)]
    completion_tokens: Option<u32>,
    #[serde(default)]
    total_tokens: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct OpenAiModelsResponse {
    #[serde(default)]
    data: Vec<OpenAiModelInfo>,
}

#[derive(Debug, Deserialize)]
struct OpenAiModelInfo {
    id: String,
}

#[derive(Debug, Deserialize)]
struct OpenAiErrorEnvelope {
    #[serde(default)]
    error: Option<OpenAiErrorDetail>,
}

#[derive(Debug, Deserialize)]
struct OpenAiErrorDetail {
    #[serde(default)]
    message: String,
    #[serde(rename = "type", default)]
    error_type: Option<String>,
    #[serde(default)]
    code: Option<String>,
}

fn map_usage(usage: OpenAiUsage) -> Option<GenerationUsage> {
    let total_tokens = usage.total_tokens.or_else(|| {
        let (Some(prompt_tokens), Some(completion_tokens)) =
            (usage.prompt_tokens, usage.completion_tokens)
        else {
            return None;
        };
        prompt_tokens.checked_add(completion_tokens)
    });

    let mapped = GenerationUsage {
        input_tokens: usage.prompt_tokens,
        output_tokens: usage.completion_tokens,
        total_tokens,
        cache_creation_input_tokens: None,
        cache_read_input_tokens: None,
    };

    if mapped.input_tokens.is_some()
        || mapped.output_tokens.is_some()
        || mapped.total_tokens.is_some()
    {
        Some(mapped)
    } else {
        None
    }
}

fn extract_message_content(content: &Value) -> Option<String> {
    match content {
        Value::String(text) => non_empty_owned(text),
        Value::Array(parts) => {
            let joined = parts
                .iter()
                .filter_map(extract_content_part_text)
                .collect::<String>();
            non_empty_owned(&joined)
        }
        _ => None,
    }
}

fn extract_content_part_text(part: &Value) -> Option<String> {
    match part {
        Value::String(text) => Some(text.to_string()),
        Value::Object(map) => map
            .get("text")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        _ => None,
    }
}

fn map_http_error(status: StatusCode, body: &str) -> LlmError {
    let parsed_error = serde_json::from_str::<OpenAiErrorEnvelope>(body).ok();
    let error_type = parsed_error
        .as_ref()
        .and_then(|envelope| envelope.error.as_ref())
        .and_then(|detail| detail.error_type.as_deref());
    let error_code = parsed_error
        .as_ref()
        .and_then(|envelope| envelope.error.as_ref())
        .and_then(|detail| detail.code.as_deref());

    if status == StatusCode::UNAUTHORIZED
        || status == StatusCode::FORBIDDEN
        || matches!(error_type, Some("authentication_error"))
        || matches!(
            error_code,
            Some("invalid_api_key" | "invalid_authentication")
        )
    {
        return LlmError::Auth;
    }

    if status == StatusCode::TOO_MANY_REQUESTS
        || matches!(error_type, Some("rate_limit_error" | "insufficient_quota"))
        || matches!(
            error_code,
            Some("rate_limit_exceeded" | "insufficient_quota")
        )
    {
        return LlmError::RateLimited;
    }

    if status == StatusCode::REQUEST_TIMEOUT
        || status == StatusCode::GATEWAY_TIMEOUT
        || matches!(error_type, Some("timeout" | "server_timeout"))
        || matches!(error_code, Some("request_timeout"))
    {
        return LlmError::Timeout;
    }

    let message = parsed_error
        .as_ref()
        .and_then(|envelope| envelope.error.as_ref())
        .map(|detail| detail.message.clone())
        .filter(|message| !message.trim().is_empty())
        .unwrap_or_else(|| truncate_message(body));

    LlmError::Transport {
        message: format!("OpenAI-compatible API returned HTTP {status}: {message}"),
    }
}

fn map_transport_error(error: reqwest::Error) -> LlmError {
    if error.is_timeout() {
        return LlmError::Timeout;
    }

    LlmError::Transport {
        message: format!("OpenAI-compatible transport error: {error}"),
    }
}

fn non_empty_owned(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn default_supported_models() -> Vec<String> {
    DEFAULT_SUPPORTED_MODELS
        .iter()
        .map(|model| (*model).to_string())
        .collect()
}

fn normalize_supported_models(models: Vec<String>) -> Result<BTreeSet<String>, LlmError> {
    let normalized = models
        .into_iter()
        .filter_map(|model| non_empty_owned(&model))
        .collect::<BTreeSet<_>>();

    if normalized.is_empty() {
        return Err(LlmError::validation(
            "OpenAI-compatible supported models must not be empty",
        ));
    }

    Ok(normalized)
}

fn normalize_supported_models_from_response(
    models: Vec<String>,
) -> Result<BTreeSet<String>, LlmError> {
    let normalized = models
        .into_iter()
        .filter_map(|model| non_empty_owned(&model))
        .collect::<BTreeSet<_>>();

    if normalized.is_empty() {
        return Err(LlmError::invalid_response(
            "OpenAI-compatible models response did not include any model IDs",
        ));
    }

    Ok(normalized)
}

fn parse_supported_models(value: &str) -> Result<Vec<String>, LlmError> {
    let models = value
        .split(',')
        .filter_map(non_empty_owned)
        .collect::<Vec<_>>();

    if models.is_empty() {
        return Err(LlmError::validation(
            "SONANT_OPENAI_COMPAT_MODELS must include at least one model ID",
        ));
    }

    Ok(models)
}

fn read_bool_env(name: &str) -> Result<bool, LlmError> {
    let Some(value) = read_env_var(name)? else {
        return Ok(false);
    };

    parse_bool(&value).ok_or_else(|| {
        LlmError::validation(format!(
            "{name} must be one of: true,false,1,0,yes,no,on,off"
        ))
    })
}

fn parse_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

fn build_v1_url(api_base_url: &str, endpoint_path: &str) -> String {
    let base = api_base_url.trim_end_matches('/');
    let endpoint_path = endpoint_path.trim_start_matches('/');

    if base.ends_with("/v1") {
        format!("{base}/{endpoint_path}")
    } else {
        format!("{base}/v1/{endpoint_path}")
    }
}

#[cfg(test)]
mod tests {
    use super::{OpenAiCompatibleProvider, build_v1_url, map_http_error, parse_bool};
    use crate::domain::{
        FileReferenceInput, GenerationMode, GenerationParams, GenerationRequest, LlmError,
        MidiReferenceSummary, ModelRef, ReferenceSlot, ReferenceSource,
    };
    use crate::infra::llm::{LlmProvider, PromptBuilder};
    use reqwest::StatusCode;
    use std::time::Duration;

    fn provider() -> OpenAiCompatibleProvider {
        OpenAiCompatibleProvider::with_config(
            "openai_compatible",
            "test-key",
            "https://api.openai.com",
            Duration::from_secs(2),
            vec!["gpt-5.2".to_string()],
        )
        .expect("provider should build")
    }

    fn request() -> GenerationRequest {
        GenerationRequest {
            request_id: "req-42".to_string(),
            model: ModelRef {
                provider: "openai_compatible".to_string(),
                model: "gpt-5.2".to_string(),
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
                slot: ReferenceSlot::Melody,
                source: ReferenceSource::File,
                file: Some(FileReferenceInput {
                    path: "references/melody.mid".to_string(),
                }),
                bars: 4,
                note_count: 24,
                density_hint: 0.42,
                min_pitch: 60,
                max_pitch: 74,
                events: vec![crate::domain::MidiReferenceEvent {
                    track: 0,
                    absolute_tick: 0,
                    delta_tick: 0,
                    event: "NoteOn channel=0 key=60 vel=100".to_string(),
                }],
            }],
            variation_count: 2,
        }
    }

    #[test]
    fn build_request_payload_maps_generation_request() {
        let payload = provider()
            .build_request_payload(&request())
            .expect("payload should be built");

        assert_eq!(payload.model, "gpt-5.2");
        assert_eq!(payload.max_tokens, Some(512));
        assert_eq!(payload.temperature, Some(0.5));
        assert_eq!(payload.top_p, Some(0.9));
        assert_eq!(payload.messages.len(), 2);
        assert_eq!(payload.messages[0].role, "system");
        assert_eq!(payload.messages[1].role, "user");
        assert!(
            payload.messages[1]
                .content
                .contains("request_id must equal \"req-42\"")
        );
        assert!(
            payload.messages[1]
                .content
                .contains("candidates must contain exactly 2 items")
        );
    }

    #[test]
    fn build_request_payload_uses_prompt_builder_output() {
        let request = request();
        let prompt = PromptBuilder::build(&request);

        let payload = provider()
            .build_request_payload(&request)
            .expect("payload should be built");

        assert_eq!(payload.messages.len(), 2);
        assert_eq!(payload.messages[0].role, "system");
        assert_eq!(payload.messages[0].content, prompt.system);
        assert_eq!(payload.messages[1].role, "user");
        assert_eq!(payload.messages[1].content, prompt.user);
    }

    #[test]
    fn build_request_payload_reflects_mode_in_prompt_content() {
        let mut request = request();
        request.mode = GenerationMode::Continuation;

        let payload = provider()
            .build_request_payload(&request)
            .expect("payload should be built");

        assert!(
            payload.messages[1]
                .content
                .contains("Generation mode: continuation")
        );
        assert!(
            payload.messages[1]
                .content
                .contains("Continue the musical idea")
        );
    }

    #[test]
    fn map_success_response_extracts_result_and_metadata() {
        let response = r#"{
          "id": "chatcmpl_01",
          "choices": [
            {
              "index": 0,
              "finish_reason": "stop",
              "message": {
                "role": "assistant",
                "content": "```json\n{\n  \"request_id\": \"req-42\",\n  \"model\": {\n    \"provider\": \"openai_compatible\",\n    \"model\": \"gpt-5.2\"\n  },\n  \"candidates\": [\n    {\n      \"id\": \"cand-1\",\n      \"bars\": 4,\n      \"notes\": [\n        {\n          \"pitch\": 60,\n          \"start_tick\": 0,\n          \"duration_tick\": 240,\n          \"velocity\": 96,\n          \"channel\": 1\n        }\n      ]\n    }\n  ]\n}\n```"
              }
            }
          ],
          "usage": {
            "prompt_tokens": 120,
            "completion_tokens": 36,
            "total_tokens": 156
          }
        }"#;

        let result = provider()
            .map_success_response(&request(), response, 410, Some("req_hdr".to_string()))
            .expect("response mapping should succeed");

        assert_eq!(result.request_id, "req-42");
        assert_eq!(result.candidates.len(), 1);
        assert_eq!(result.metadata.latency_ms, Some(410));
        assert_eq!(
            result.metadata.provider_request_id.as_deref(),
            Some("req_hdr")
        );
        assert_eq!(result.metadata.stop_reason.as_deref(), Some("stop"));
        assert_eq!(
            result
                .metadata
                .usage
                .as_ref()
                .and_then(|usage| usage.total_tokens),
            Some(156)
        );
    }

    #[test]
    fn map_success_response_accepts_content_array_parts() {
        let response = r#"{
          "id": "chatcmpl_01",
          "choices": [
            {
              "finish_reason": "stop",
              "message": {
                "content": [
                  {
                    "type": "text",
                    "text": "{\"request_id\":\"req-42\",\"model\":{\"provider\":\"openai_compatible\",\"model\":\"gpt-5.2\"},\"candidates\":["
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
              }
            }
          ]
        }"#;

        let result = provider()
            .map_success_response(&request(), response, 33, None)
            .expect("array content parts should still parse");

        assert_eq!(result.request_id, "req-42");
        assert_eq!(result.candidates.len(), 1);
        assert_eq!(result.candidates[0].id, "cand-1");
        assert_eq!(result.metadata.latency_ms, Some(33));
    }

    #[test]
    fn map_success_response_rejects_request_id_mismatch() {
        let response = r#"{
          "id": "chatcmpl_01",
          "choices": [
            {
              "message": {
                "content": "{\"request_id\":\"req-other\",\"model\":{\"provider\":\"openai_compatible\",\"model\":\"gpt-5.2\"},\"candidates\":[{\"id\":\"cand-1\",\"bars\":4,\"notes\":[{\"pitch\":60,\"start_tick\":0,\"duration_tick\":240,\"velocity\":96}]}]}"
              }
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
            r#"{"error":{"type":"authentication_error","code":"invalid_api_key","message":"invalid key"}}"#,
        );
        let rate_limited = map_http_error(
            StatusCode::TOO_MANY_REQUESTS,
            r#"{"error":{"type":"rate_limit_error","code":"rate_limit_exceeded","message":"slow down"}}"#,
        );
        let timeout = map_http_error(
            StatusCode::GATEWAY_TIMEOUT,
            r#"{"error":{"type":"server_timeout","code":"request_timeout","message":"timed out"}}"#,
        );

        assert!(matches!(auth, LlmError::Auth));
        assert!(matches!(rate_limited, LlmError::RateLimited));
        assert!(matches!(timeout, LlmError::Timeout));
    }

    #[test]
    fn supports_model_uses_static_catalog() {
        let provider = provider();

        assert!(provider.supports_model("gpt-5.2"));
    }

    #[test]
    fn with_config_rejects_empty_model_catalog() {
        let error = match OpenAiCompatibleProvider::with_config(
            "openai_compatible",
            "test-key",
            "https://api.openai.com",
            Duration::from_secs(2),
            Vec::new(),
        ) {
            Ok(_) => panic!("empty model catalog should fail"),
            Err(error) => error,
        };

        assert!(matches!(
            error,
            LlmError::Validation { message }
            if message == "OpenAI-compatible supported models must not be empty"
        ));
    }

    #[test]
    fn parse_bool_accepts_expected_variants() {
        assert_eq!(parse_bool("true"), Some(true));
        assert_eq!(parse_bool("1"), Some(true));
        assert_eq!(parse_bool("YES"), Some(true));
        assert_eq!(parse_bool("off"), Some(false));
        assert_eq!(parse_bool("0"), Some(false));
        assert_eq!(parse_bool("maybe"), None);
    }

    #[test]
    fn build_v1_url_appends_v1_when_base_has_no_version_segment() {
        let url = build_v1_url("https://api.openai.com", "chat/completions");
        assert_eq!(url, "https://api.openai.com/v1/chat/completions");

        let url = build_v1_url("https://api.openai.com/", "/models");
        assert_eq!(url, "https://api.openai.com/v1/models");
    }

    #[test]
    fn build_v1_url_avoids_duplicate_v1_when_base_already_has_v1() {
        let url = build_v1_url("https://example.com/v1", "chat/completions");
        assert_eq!(url, "https://example.com/v1/chat/completions");

        let url = build_v1_url("https://example.com/v1/", "models");
        assert_eq!(url, "https://example.com/v1/models");
    }
}
