use std::thread;
use std::time::{Duration, Instant};

use crate::domain::{GenerationRequest, GenerationResult, LlmError};
use crate::infra::llm::ProviderRegistry;

const DEFAULT_RETRY_MAX_ATTEMPTS: u8 = 3;
const DEFAULT_RETRY_INITIAL_BACKOFF_MS: u64 = 200;
const DEFAULT_RETRY_MAX_BACKOFF_MS: u64 = 2_000;
const BACKOFF_CANCEL_POLL_INTERVAL_MS: u64 = 10;
const CANCELLATION_ERROR_MESSAGE: &str = "generation cancelled";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GenerationRetryConfig {
    pub max_attempts: u8,
    pub initial_backoff: Duration,
    pub max_backoff: Duration,
}

impl Default for GenerationRetryConfig {
    fn default() -> Self {
        Self {
            max_attempts: DEFAULT_RETRY_MAX_ATTEMPTS,
            initial_backoff: Duration::from_millis(DEFAULT_RETRY_INITIAL_BACKOFF_MS),
            max_backoff: Duration::from_millis(DEFAULT_RETRY_MAX_BACKOFF_MS),
        }
    }
}

impl GenerationRetryConfig {
    pub fn validate(&self) -> Result<(), LlmError> {
        if self.max_attempts == 0 {
            return Err(LlmError::validation(
                "retry max_attempts must be greater than 0",
            ));
        }
        if self.initial_backoff > self.max_backoff {
            return Err(LlmError::validation(
                "retry initial_backoff must be less than or equal to max_backoff",
            ));
        }
        Ok(())
    }

    fn backoff_for_retry(&self, retry_index: u8) -> Duration {
        let capped_retry_index = retry_index.saturating_sub(1).min(30);
        let multiplier = 1_u32 << u32::from(capped_retry_index);
        let backoff = self.initial_backoff.saturating_mul(multiplier);
        backoff.min(self.max_backoff)
    }
}

#[derive(Clone)]
pub struct GenerationService {
    registry: ProviderRegistry,
    retry_config: GenerationRetryConfig,
}

impl GenerationService {
    pub fn new(registry: ProviderRegistry) -> Self {
        Self {
            registry,
            retry_config: GenerationRetryConfig::default(),
        }
    }

    pub fn with_retry_config(
        registry: ProviderRegistry,
        retry_config: GenerationRetryConfig,
    ) -> Result<Self, LlmError> {
        retry_config.validate()?;
        Ok(Self {
            registry,
            retry_config,
        })
    }

    pub fn generate(&self, request: GenerationRequest) -> Result<GenerationResult, LlmError> {
        self.generate_with_cancel(request, || false)
    }

    pub fn generate_with_cancel<F>(
        &self,
        mut request: GenerationRequest,
        is_cancelled: F,
    ) -> Result<GenerationResult, LlmError>
    where
        F: Fn() -> bool,
    {
        // Canonicalize provider/model IDs so resolution and provider execution use the same values.
        request.model.provider = request.model.provider.trim().to_string();
        request.model.model = request.model.model.trim().to_string();

        request.validate()?;

        let provider = self
            .registry
            .resolve(&request.model.provider, &request.model.model)?;
        let mut attempt = 1_u8;

        loop {
            if is_cancelled() {
                return Err(LlmError::internal(CANCELLATION_ERROR_MESSAGE));
            }

            match provider.generate(&request) {
                Ok(result) => {
                    result.validate()?;
                    return Ok(result);
                }
                Err(error) => {
                    if attempt >= self.retry_config.max_attempts || !error.is_retryable() {
                        return Err(error);
                    }

                    if is_cancelled() {
                        return Err(LlmError::internal(CANCELLATION_ERROR_MESSAGE));
                    }

                    let backoff = self.retry_config.backoff_for_retry(attempt);
                    if sleep_with_cancellation(backoff, &is_cancelled) {
                        return Err(LlmError::internal(CANCELLATION_ERROR_MESSAGE));
                    }
                    attempt = attempt.saturating_add(1);
                }
            }
        }
    }
}

fn sleep_with_cancellation<F>(duration: Duration, is_cancelled: &F) -> bool
where
    F: Fn() -> bool,
{
    if duration.is_zero() {
        return is_cancelled();
    }

    let deadline = Instant::now() + duration;
    let poll_interval = Duration::from_millis(BACKOFF_CANCEL_POLL_INTERVAL_MS);

    loop {
        if is_cancelled() {
            return true;
        }

        let now = Instant::now();
        if now >= deadline {
            return false;
        }

        let remaining = deadline.saturating_duration_since(now);
        thread::sleep(remaining.min(poll_interval));
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::thread;

    use super::{GenerationRetryConfig, GenerationService};
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

            Ok(valid_result(request))
        }
    }

    struct RoutedCountingProvider {
        provider_id: &'static str,
        model_id: &'static str,
        calls: Arc<AtomicUsize>,
    }

    impl LlmProvider for RoutedCountingProvider {
        fn provider_id(&self) -> &str {
            self.provider_id
        }

        fn supports_model(&self, model_id: &str) -> bool {
            model_id == self.model_id
        }

        fn generate(&self, request: &GenerationRequest) -> Result<GenerationResult, LlmError> {
            self.calls.fetch_add(1, Ordering::SeqCst);

            Ok(valid_result(request))
        }
    }

    struct RetryControlledProvider {
        calls: Arc<AtomicUsize>,
        failures_before_success: usize,
        failure_error: LlmError,
    }

    impl LlmProvider for RetryControlledProvider {
        fn provider_id(&self) -> &str {
            "anthropic"
        }

        fn supports_model(&self, model_id: &str) -> bool {
            model_id == "claude-3-5-sonnet"
        }

        fn generate(&self, request: &GenerationRequest) -> Result<GenerationResult, LlmError> {
            let attempt = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
            if attempt <= self.failures_before_success {
                return Err(self.failure_error.clone());
            }

            Ok(valid_result(request))
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
                    velocity: 100,
                    channel: 1,
                }],
                score_hint: Some(0.8),
            }],
            metadata: GenerationMetadata::default(),
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
    fn generate_routes_to_provider_selected_by_request_model_ref() {
        let anthropic_calls = Arc::new(AtomicUsize::new(0));
        let openai_calls = Arc::new(AtomicUsize::new(0));

        let anthropic_provider = Arc::new(RoutedCountingProvider {
            provider_id: "anthropic",
            model_id: "claude-3-5-sonnet",
            calls: Arc::clone(&anthropic_calls),
        });
        let openai_provider = Arc::new(RoutedCountingProvider {
            provider_id: "openai_compatible",
            model_id: "gpt-4.1",
            calls: Arc::clone(&openai_calls),
        });

        let mut registry = ProviderRegistry::new();
        registry
            .register_shared(anthropic_provider)
            .expect("anthropic provider registration should succeed");
        registry
            .register_shared(openai_provider)
            .expect("openai-compatible provider registration should succeed");

        let service = GenerationService::new(registry);
        let mut request = valid_request();
        request.model.provider = "openai_compatible".to_string();
        request.model.model = "gpt-4.1".to_string();

        let result = service
            .generate(request)
            .expect("generation should route to openai-compatible provider");

        assert_eq!(result.model.provider, "openai_compatible");
        assert_eq!(result.model.model, "gpt-4.1");
        assert_eq!(anthropic_calls.load(Ordering::SeqCst), 0);
        assert_eq!(openai_calls.load(Ordering::SeqCst), 1);
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

    #[test]
    fn retry_config_backoff_grows_exponentially_and_caps() {
        let config = GenerationRetryConfig {
            max_attempts: 4,
            initial_backoff: Duration::from_millis(10),
            max_backoff: Duration::from_millis(25),
        };

        assert_eq!(config.backoff_for_retry(1), Duration::from_millis(10));
        assert_eq!(config.backoff_for_retry(2), Duration::from_millis(20));
        assert_eq!(config.backoff_for_retry(3), Duration::from_millis(25));
    }

    #[test]
    fn retry_config_validation_rejects_invalid_ranges() {
        let invalid_attempts = GenerationRetryConfig {
            max_attempts: 0,
            ..GenerationRetryConfig::default()
        };
        assert!(matches!(
            invalid_attempts.validate(),
            Err(LlmError::Validation { message })
            if message == "retry max_attempts must be greater than 0"
        ));

        let invalid_backoff = GenerationRetryConfig {
            max_attempts: 3,
            initial_backoff: Duration::from_millis(30),
            max_backoff: Duration::from_millis(20),
        };
        assert!(matches!(
            invalid_backoff.validate(),
            Err(LlmError::Validation { message })
            if message == "retry initial_backoff must be less than or equal to max_backoff"
        ));
    }

    #[test]
    fn generate_retries_retryable_errors_until_success() {
        let calls = Arc::new(AtomicUsize::new(0));
        let provider = Arc::new(RetryControlledProvider {
            calls: Arc::clone(&calls),
            failures_before_success: 2,
            failure_error: LlmError::Timeout,
        });

        let mut registry = ProviderRegistry::new();
        registry
            .register_shared(provider)
            .expect("provider registration should succeed");

        let retry_config = GenerationRetryConfig {
            max_attempts: 3,
            initial_backoff: Duration::from_millis(20),
            max_backoff: Duration::from_millis(80),
        };
        let service = GenerationService::with_retry_config(registry, retry_config)
            .expect("retry config should be valid");

        let started = Instant::now();
        let result = service
            .generate(valid_request())
            .expect("third attempt should succeed");

        assert_eq!(result.request_id, "req-1");
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        assert!(
            started.elapsed() >= Duration::from_millis(50),
            "expected exponential backoff delays before final success"
        );
    }

    #[test]
    fn generate_does_not_retry_non_retryable_errors() {
        let calls = Arc::new(AtomicUsize::new(0));
        let provider = Arc::new(RetryControlledProvider {
            calls: Arc::clone(&calls),
            failures_before_success: usize::MAX,
            failure_error: LlmError::Auth,
        });

        let mut registry = ProviderRegistry::new();
        registry
            .register_shared(provider)
            .expect("provider registration should succeed");

        let service = GenerationService::new(registry);
        let error = service
            .generate(valid_request())
            .expect_err("non-retryable error should fail fast");

        assert!(matches!(error, LlmError::Auth));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn generate_stops_after_max_retry_attempts() {
        let calls = Arc::new(AtomicUsize::new(0));
        let provider = Arc::new(RetryControlledProvider {
            calls: Arc::clone(&calls),
            failures_before_success: usize::MAX,
            failure_error: LlmError::RateLimited,
        });

        let mut registry = ProviderRegistry::new();
        registry
            .register_shared(provider)
            .expect("provider registration should succeed");

        let retry_config = GenerationRetryConfig {
            max_attempts: 3,
            initial_backoff: Duration::from_millis(0),
            max_backoff: Duration::from_millis(0),
        };
        let service = GenerationService::with_retry_config(registry, retry_config)
            .expect("retry config should be valid");

        let error = service
            .generate(valid_request())
            .expect_err("retryable error should bubble up after max attempts");

        assert!(matches!(error, LlmError::RateLimited));
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn generate_with_cancel_aborts_before_first_attempt() {
        let calls = Arc::new(AtomicUsize::new(0));
        let provider = Arc::new(RetryControlledProvider {
            calls: Arc::clone(&calls),
            failures_before_success: usize::MAX,
            failure_error: LlmError::Timeout,
        });

        let mut registry = ProviderRegistry::new();
        registry
            .register_shared(provider)
            .expect("provider registration should succeed");

        let service = GenerationService::new(registry);
        let cancelled = Arc::new(AtomicBool::new(true));
        let error = service
            .generate_with_cancel(valid_request(), || cancelled.load(Ordering::SeqCst))
            .expect_err("cancelled request should fail before calling provider");

        assert!(matches!(
            error,
            LlmError::Internal { message } if message == "generation cancelled"
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn generate_with_cancel_interrupts_retry_backoff_sleep() {
        let calls = Arc::new(AtomicUsize::new(0));
        let provider = Arc::new(RetryControlledProvider {
            calls: Arc::clone(&calls),
            failures_before_success: usize::MAX,
            failure_error: LlmError::Timeout,
        });

        let mut registry = ProviderRegistry::new();
        registry
            .register_shared(provider)
            .expect("provider registration should succeed");

        let retry_config = GenerationRetryConfig {
            max_attempts: 5,
            initial_backoff: Duration::from_millis(400),
            max_backoff: Duration::from_millis(400),
        };
        let service = GenerationService::with_retry_config(registry, retry_config)
            .expect("retry config should be valid");

        let cancelled = Arc::new(AtomicBool::new(false));
        let cancelled_for_thread = Arc::clone(&cancelled);
        let cancellation_thread = thread::spawn(move || {
            thread::sleep(Duration::from_millis(25));
            cancelled_for_thread.store(true, Ordering::SeqCst);
        });

        let started = Instant::now();
        let error = service
            .generate_with_cancel(valid_request(), || cancelled.load(Ordering::SeqCst))
            .expect_err("cancellation should interrupt retry backoff");
        cancellation_thread
            .join()
            .expect("cancellation control thread should join");

        assert!(matches!(
            error,
            LlmError::Internal { message } if message == "generation cancelled"
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(
            started.elapsed() < Duration::from_millis(200),
            "cancellable sleep should stop before full backoff duration"
        );
    }
}
