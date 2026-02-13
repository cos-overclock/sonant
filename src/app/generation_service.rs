use crate::domain::{GenerationRequest, GenerationResult, LlmError};
use crate::infra::llm::ProviderRegistry;

#[derive(Clone)]
pub struct GenerationService {
    registry: ProviderRegistry,
}

impl GenerationService {
    pub fn new(registry: ProviderRegistry) -> Self {
        Self { registry }
    }

    pub fn generate(&self, mut request: GenerationRequest) -> Result<GenerationResult, LlmError> {
        // Canonicalize provider/model IDs so resolution and provider execution use the same values.
        request.model.provider = request.model.provider.trim().to_string();
        request.model.model = request.model.model.trim().to_string();

        request.validate()?;

        let provider = self
            .registry
            .resolve(&request.model.provider, &request.model.model)?;
        let result = provider.generate(&request)?;

        result.validate()?;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::GenerationService;
    use crate::domain::{
        GeneratedNote, GenerationCandidate, GenerationMetadata, GenerationMode, GenerationParams,
        GenerationRequest, GenerationResult, LlmError, ModelRef,
    };
    use crate::infra::llm::{LlmProvider, ProviderRegistry};

    struct CountingProvider {
        calls: Arc<AtomicUsize>,
        last_ids: Arc<Mutex<Option<(String, String)>>>,
    }

    impl LlmProvider for CountingProvider {
        fn provider_id(&self) -> &str {
            "anthropic"
        }

        fn supports_model(&self, model_id: &str) -> bool {
            model_id == "claude-3-5-sonnet"
        }

        fn generate(&self, request: &GenerationRequest) -> Result<GenerationResult, LlmError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            *self.last_ids.lock().expect("mutex poisoned") =
                Some((request.model.provider.clone(), request.model.model.clone()));

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
                    score_hint: Some(0.8),
                }],
                metadata: GenerationMetadata::default(),
            })
        }
    }

    fn valid_request() -> GenerationRequest {
        GenerationRequest {
            request_id: "req-1".to_string(),
            model: ModelRef {
                provider: "anthropic".to_string(),
                model: "claude-3-5-sonnet".to_string(),
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

    #[test]
    fn generate_routes_request_to_registry_resolved_provider() {
        let calls = Arc::new(AtomicUsize::new(0));
        let last_ids = Arc::new(Mutex::new(None));
        let provider = Arc::new(CountingProvider {
            calls: Arc::clone(&calls),
            last_ids: Arc::clone(&last_ids),
        });

        let mut registry = ProviderRegistry::new();
        registry
            .register_shared(provider)
            .expect("provider registration should succeed");

        let service = GenerationService::new(registry);
        let result = service
            .generate(valid_request())
            .expect("generation should succeed");

        assert_eq!(result.request_id, "req-1");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            *last_ids.lock().expect("mutex poisoned"),
            Some(("anthropic".to_string(), "claude-3-5-sonnet".to_string()))
        );
    }

    #[test]
    fn generate_trims_model_identifiers_before_provider_call() {
        let calls = Arc::new(AtomicUsize::new(0));
        let last_ids = Arc::new(Mutex::new(None));
        let provider = Arc::new(CountingProvider {
            calls: Arc::clone(&calls),
            last_ids: Arc::clone(&last_ids),
        });

        let mut registry = ProviderRegistry::new();
        registry
            .register_shared(provider)
            .expect("provider registration should succeed");

        let service = GenerationService::new(registry);
        let mut request = valid_request();
        request.model.provider = " anthropic ".to_string();
        request.model.model = " claude-3-5-sonnet ".to_string();

        let result = service
            .generate(request)
            .expect("generation should succeed");

        assert_eq!(result.model.provider, "anthropic");
        assert_eq!(result.model.model, "claude-3-5-sonnet");
        assert_eq!(
            *last_ids.lock().expect("mutex poisoned"),
            Some(("anthropic".to_string(), "claude-3-5-sonnet".to_string()))
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn generate_returns_error_when_provider_is_missing() {
        let service = GenerationService::new(ProviderRegistry::new());
        let error = service
            .generate(valid_request())
            .expect_err("unregistered provider should fail");

        assert!(matches!(
            error,
            LlmError::Validation { message } if message == "provider 'anthropic' is not registered"
        ));
    }

    #[test]
    fn generate_validates_request_before_provider_call() {
        let calls = Arc::new(AtomicUsize::new(0));
        let last_ids = Arc::new(Mutex::new(None));
        let provider = Arc::new(CountingProvider {
            calls: Arc::clone(&calls),
            last_ids: Arc::clone(&last_ids),
        });

        let mut registry = ProviderRegistry::new();
        registry
            .register_shared(provider)
            .expect("provider registration should succeed");

        let service = GenerationService::new(registry);
        let mut invalid_request = valid_request();
        invalid_request.prompt = " ".to_string();

        let error = service
            .generate(invalid_request)
            .expect_err("invalid request should fail");

        assert!(matches!(
            error,
            LlmError::Validation { message } if message == "prompt must not be empty"
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    /// Test-only provider that always returns an invalid `GenerationResult`.
    /// This is used to exercise the `result.validate()` error path in `GenerationService::generate`.
    struct InvalidResultProvider;

    impl LlmProvider for InvalidResultProvider {
        fn provider_id(&self) -> &str {
            "anthropic"
        }

        fn supports_model(&self, model_id: &str) -> bool {
            model_id == "claude-3-5-sonnet"
        }

        fn generate(&self, _request: &GenerationRequest) -> Result<GenerationResult, LlmError> {
            Ok(GenerationResult {
                request_id: String::new(),
                model: ModelRef {
                    provider: "anthropic".to_string(),
                    model: "claude-3-5-sonnet".to_string(),
                },
                candidates: Vec::new(),
                metadata: GenerationMetadata::default(),
            })
        }
    }

    #[test]
    fn generate_returns_error_when_result_is_invalid() {
        let provider = Arc::new(InvalidResultProvider);

        let mut registry = ProviderRegistry::new();
        registry
            .register_shared(provider)
            .expect("provider registration should succeed");

        let service = GenerationService::new(registry);

        let error = service
            .generate(valid_request())
            .expect_err("invalid result should fail validation");

        assert!(matches!(error, LlmError::Validation { .. }));
    }
}
