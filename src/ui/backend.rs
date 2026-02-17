use std::sync::Arc;

use sonant::{
    app::{GenerationJobManager, GenerationService},
    domain::{GenerationRequest, GenerationResult, LlmError, ModelRef},
    infra::llm::{AnthropicProvider, LlmProvider, OpenAiCompatibleProvider, ProviderRegistry},
};

use super::{
    DEFAULT_ANTHROPIC_MODEL, DEFAULT_OPENAI_COMPAT_MODEL, STUB_MODEL_ID, STUB_PROVIDER_ID,
    STUB_PROVIDER_NOTICE,
};

pub(super) struct GenerationBackend {
    pub(super) job_manager: Arc<GenerationJobManager>,
    pub(super) default_model: ModelRef,
    pub(super) startup_notice: Option<String>,
}

pub(super) fn build_generation_backend() -> GenerationBackend {
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
