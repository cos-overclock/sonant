use std::collections::HashMap;
use std::sync::Arc;

use crate::domain::LlmError;

use super::LlmProvider;

#[derive(Default, Clone)]
pub struct ProviderRegistry {
    providers: HashMap<String, Arc<dyn LlmProvider>>,
}

impl ProviderRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register<P>(&mut self, provider: P) -> Result<(), LlmError>
    where
        P: LlmProvider + 'static,
    {
        self.register_shared(Arc::new(provider))
    }

    pub fn register_shared(&mut self, provider: Arc<dyn LlmProvider>) -> Result<(), LlmError> {
        let provider_id = provider.provider_id().trim();
        if provider_id.is_empty() {
            return Err(LlmError::validation("provider_id must not be empty"));
        }
        if self.providers.contains_key(provider_id) {
            return Err(LlmError::validation(format!(
                "provider '{provider_id}' is already registered"
            )));
        }

        self.providers.insert(provider_id.to_string(), provider);
        Ok(())
    }

    pub fn resolve(
        &self,
        provider_id: &str,
        model_id: &str,
    ) -> Result<Arc<dyn LlmProvider>, LlmError> {
        let provider_id = provider_id.trim();
        if provider_id.is_empty() {
            return Err(LlmError::validation("provider_id must not be empty"));
        }

        let model_id = model_id.trim();
        if model_id.is_empty() {
            return Err(LlmError::validation("model_id must not be empty"));
        }

        let provider = self.providers.get(provider_id).ok_or_else(|| {
            LlmError::validation(format!("provider '{provider_id}' is not registered"))
        })?;

        if !provider.supports_model(model_id) {
            return Err(LlmError::validation(format!(
                "model '{model_id}' is not supported by provider '{provider_id}'"
            )));
        }

        Ok(Arc::clone(provider))
    }

    pub fn len(&self) -> usize {
        self.providers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.providers.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::ProviderRegistry;
    use crate::domain::{
        GeneratedNote, GenerationCandidate, GenerationMode, GenerationParams, GenerationRequest,
        GenerationResult, LlmError, ModelRef,
    };
    use crate::infra::llm::LlmProvider;

    struct FakeProvider {
        provider_id: &'static str,
        supported_models: &'static [&'static str],
    }

    impl LlmProvider for FakeProvider {
        fn provider_id(&self) -> &str {
            self.provider_id
        }

        fn supports_model(&self, model_id: &str) -> bool {
            self.supported_models.contains(&model_id)
        }

        fn generate(&self, request: &GenerationRequest) -> Result<GenerationResult, LlmError> {
            Ok(GenerationResult {
                request_id: request.request_id.clone(),
                model: request.model.clone(),
                candidates: vec![GenerationCandidate {
                    id: "cand-1".to_string(),
                    bars: 4,
                    notes: vec![GeneratedNote {
                        pitch: 60,
                        start_tick: 0,
                        duration_tick: 240,
                        velocity: 100,
                        channel: 1,
                    }],
                    score_hint: Some(0.9),
                }],
            })
        }
    }

    fn request(provider_id: &str, model_id: &str) -> GenerationRequest {
        GenerationRequest {
            request_id: "req-1".to_string(),
            model: ModelRef {
                provider: provider_id.to_string(),
                model: model_id.to_string(),
            },
            mode: GenerationMode::Melody,
            prompt: "lofi groove".to_string(),
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

    #[test]
    fn register_and_resolve_provider_for_model() {
        let mut registry = ProviderRegistry::new();
        registry
            .register(FakeProvider {
                provider_id: "anthropic",
                supported_models: &["claude-3-5-sonnet"],
            })
            .expect("provider registration should succeed");

        let provider = registry
            .resolve("anthropic", "claude-3-5-sonnet")
            .expect("provider should resolve");
        let result = provider
            .generate(&request("anthropic", "claude-3-5-sonnet"))
            .expect("provider should generate");

        assert_eq!(result.request_id, "req-1");
        assert_eq!(result.model.provider, "anthropic");
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn resolve_rejects_unknown_provider() {
        let registry = ProviderRegistry::new();
        let error = registry
            .resolve("openai", "gpt-4.1")
            .err()
            .expect("unknown provider should fail");

        assert!(matches!(
            error,
            LlmError::Validation { message } if message == "provider 'openai' is not registered"
        ));
    }

    #[test]
    fn resolve_rejects_unsupported_model() {
        let mut registry = ProviderRegistry::new();
        registry
            .register(FakeProvider {
                provider_id: "anthropic",
                supported_models: &["claude-3-5-sonnet"],
            })
            .expect("provider registration should succeed");

        let error = registry
            .resolve("anthropic", "claude-3-opus")
            .err()
            .expect("unsupported model should fail");

        assert!(matches!(
            error,
            LlmError::Validation { message }
            if message == "model 'claude-3-opus' is not supported by provider 'anthropic'"
        ));
    }

    #[test]
    fn register_rejects_duplicate_provider() {
        let mut registry = ProviderRegistry::new();
        registry
            .register(FakeProvider {
                provider_id: "anthropic",
                supported_models: &["claude-3-5-sonnet"],
            })
            .expect("first registration should succeed");

        let error = registry
            .register(FakeProvider {
                provider_id: "anthropic",
                supported_models: &["claude-3-opus"],
            })
            .expect_err("duplicate registration should fail");

        assert!(matches!(
            error,
            LlmError::Validation { message }
            if message == "provider 'anthropic' is already registered"
        ));
    }
}
