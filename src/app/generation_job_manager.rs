use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;

use crate::domain::{GenerationRequest, GenerationResult, LlmError};

use super::GenerationService;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GenerationJobState {
    #[default]
    Idle,
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GenerationJobUpdate {
    pub job_id: u64,
    pub request_id: String,
    pub state: GenerationJobState,
    pub result: Option<GenerationResult>,
    pub error: Option<LlmError>,
}

impl GenerationJobUpdate {
    fn running(job_id: u64, request_id: String) -> Self {
        Self {
            job_id,
            request_id,
            state: GenerationJobState::Running,
            result: None,
            error: None,
        }
    }

    fn succeeded(job_id: u64, request_id: String, result: GenerationResult) -> Self {
        Self {
            job_id,
            request_id,
            state: GenerationJobState::Succeeded,
            result: Some(result),
            error: None,
        }
    }

    fn failed(job_id: u64, request_id: String, error: LlmError) -> Self {
        Self {
            job_id,
            request_id,
            state: GenerationJobState::Failed,
            result: None,
            error: Some(error),
        }
    }

    fn cancelled(job_id: u64, request_id: String) -> Self {
        Self {
            job_id,
            request_id,
            state: GenerationJobState::Cancelled,
            result: None,
            error: None,
        }
    }
}

pub struct GenerationJobManager {
    next_job_id: AtomicU64,
    command_tx: mpsc::Sender<WorkerMessage>,
    shared: Arc<Mutex<SharedState>>,
    worker_handle: Mutex<Option<thread::JoinHandle<()>>>,
}

impl GenerationJobManager {
    pub fn new(service: GenerationService) -> Result<Self, LlmError> {
        let shared = Arc::new(Mutex::new(SharedState::default()));
        let (command_tx, command_rx) = mpsc::channel();
        let worker_tx = command_tx.clone();
        let worker_shared = Arc::clone(&shared);

        let handle = thread::Builder::new()
            .name("sonant-generation-job-worker".to_string())
            .spawn(move || worker_loop(service, command_rx, worker_tx, worker_shared))
            .map_err(|error| {
                LlmError::internal(format!(
                    "failed to start generation job worker thread: {error}"
                ))
            })?;

        Ok(Self {
            next_job_id: AtomicU64::new(1),
            command_tx,
            shared,
            worker_handle: Mutex::new(Some(handle)),
        })
    }

    pub fn submit_generate(&self, request: GenerationRequest) -> Result<u64, LlmError> {
        let job_id = self.next_job_id.fetch_add(1, Ordering::SeqCst);
        self.command_tx
            .send(WorkerMessage::Start { job_id, request })
            .map_err(|error| {
                LlmError::internal(format!(
                    "failed to submit generation job to worker queue: {error}"
                ))
            })?;
        Ok(job_id)
    }

    pub fn cancel_active(&self) -> Result<(), LlmError> {
        self.command_tx
            .send(WorkerMessage::CancelActive)
            .map_err(|error| {
                LlmError::internal(format!(
                    "failed to submit cancellation command to worker queue: {error}"
                ))
            })
    }

    pub fn state(&self) -> GenerationJobState {
        self.shared
            .lock()
            .expect("generation job state lock poisoned")
            .state
    }

    pub fn latest_update(&self) -> Option<GenerationJobUpdate> {
        self.shared
            .lock()
            .expect("generation job state lock poisoned")
            .latest
            .clone()
    }

    pub fn drain_updates(&self) -> Vec<GenerationJobUpdate> {
        let mut shared = self
            .shared
            .lock()
            .expect("generation job state lock poisoned");
        shared.updates.drain(..).collect()
    }
}

impl Drop for GenerationJobManager {
    fn drop(&mut self) {
        let _ = self.command_tx.send(WorkerMessage::Shutdown);

        if let Some(handle) = self
            .worker_handle
            .lock()
            .expect("generation worker handle lock poisoned")
            .take()
        {
            let _ = handle.join();
        }
    }
}

#[derive(Default)]
struct SharedState {
    state: GenerationJobState,
    latest: Option<GenerationJobUpdate>,
    updates: VecDeque<GenerationJobUpdate>,
}

enum WorkerMessage {
    Start {
        job_id: u64,
        request: GenerationRequest,
    },
    Completion {
        job_id: u64,
        request_id: String,
        result: Result<GenerationResult, LlmError>,
        cancelled: bool,
    },
    CancelActive,
    Shutdown,
}

struct RunningJob {
    job_id: u64,
    request_id: String,
    cancel_flag: Arc<AtomicBool>,
    cancelled_reported: bool,
    task_handle: Option<thread::JoinHandle<()>>,
}

struct PendingJob {
    job_id: u64,
    request: GenerationRequest,
}

fn worker_loop(
    service: GenerationService,
    command_rx: mpsc::Receiver<WorkerMessage>,
    command_tx: mpsc::Sender<WorkerMessage>,
    shared: Arc<Mutex<SharedState>>,
) {
    let mut in_flight: Option<RunningJob> = None;
    let mut pending_job: Option<PendingJob> = None;
    let mut shutdown_requested = false;

    while let Ok(message) = command_rx.recv() {
        match message {
            WorkerMessage::Start { job_id, request } => {
                if shutdown_requested {
                    push_update(
                        &shared,
                        GenerationJobUpdate::cancelled(job_id, request.request_id),
                    );
                    continue;
                }

                if let Some(active) = in_flight.as_mut() {
                    active.cancel_flag.store(true, Ordering::SeqCst);
                    if !active.cancelled_reported {
                        active.cancelled_reported = true;
                        push_update(
                            &shared,
                            GenerationJobUpdate::cancelled(
                                active.job_id,
                                active.request_id.clone(),
                            ),
                        );
                    }

                    if let Some(previous_pending) =
                        pending_job.replace(PendingJob { job_id, request })
                    {
                        push_update(
                            &shared,
                            GenerationJobUpdate::cancelled(
                                previous_pending.job_id,
                                previous_pending.request.request_id,
                            ),
                        );
                    }
                    continue;
                }

                in_flight = Some(spawn_generation_job(
                    &service,
                    &command_tx,
                    &shared,
                    job_id,
                    request,
                ));
            }
            WorkerMessage::Completion {
                job_id,
                request_id,
                result,
                cancelled,
            } => {
                let Some(current_job) = in_flight.as_ref() else {
                    continue;
                };

                if current_job.job_id != job_id {
                    continue;
                }

                let mut finished_job = in_flight
                    .take()
                    .expect("in-flight job should exist when completion is processed");
                let was_cancelled = cancelled
                    || finished_job.cancel_flag.load(Ordering::SeqCst)
                    || finished_job.cancelled_reported;

                if was_cancelled {
                    if !finished_job.cancelled_reported {
                        finished_job.cancelled_reported = true;
                        push_update(&shared, GenerationJobUpdate::cancelled(job_id, request_id));
                    }
                } else {
                    match result {
                        Ok(result) => {
                            push_update(
                                &shared,
                                GenerationJobUpdate::succeeded(job_id, request_id, result),
                            );
                        }
                        Err(error) => {
                            push_update(
                                &shared,
                                GenerationJobUpdate::failed(job_id, request_id, error),
                            );
                        }
                    }
                }

                join_generation_task(&mut finished_job);

                if shutdown_requested {
                    if in_flight.is_none() {
                        break;
                    }
                    continue;
                }

                if let Some(next) = pending_job.take() {
                    in_flight = Some(spawn_generation_job(
                        &service,
                        &command_tx,
                        &shared,
                        next.job_id,
                        next.request,
                    ));
                }
            }
            WorkerMessage::CancelActive => {
                if let Some(active) = in_flight.as_mut() {
                    active.cancel_flag.store(true, Ordering::SeqCst);
                    if !active.cancelled_reported {
                        active.cancelled_reported = true;
                        push_update(
                            &shared,
                            GenerationJobUpdate::cancelled(
                                active.job_id,
                                active.request_id.clone(),
                            ),
                        );
                    }
                }

                if let Some(next) = pending_job.take() {
                    push_update(
                        &shared,
                        GenerationJobUpdate::cancelled(next.job_id, next.request.request_id),
                    );
                }
            }
            WorkerMessage::Shutdown => {
                shutdown_requested = true;

                if let Some(active) = in_flight.as_mut() {
                    active.cancel_flag.store(true, Ordering::SeqCst);
                    if !active.cancelled_reported {
                        active.cancelled_reported = true;
                        push_update(
                            &shared,
                            GenerationJobUpdate::cancelled(
                                active.job_id,
                                active.request_id.clone(),
                            ),
                        );
                    }
                }

                if let Some(next) = pending_job.take() {
                    push_update(
                        &shared,
                        GenerationJobUpdate::cancelled(next.job_id, next.request.request_id),
                    );
                }

                if in_flight.is_none() {
                    break;
                }
            }
        }
    }
}

fn spawn_generation_job(
    service: &GenerationService,
    command_tx: &mpsc::Sender<WorkerMessage>,
    shared: &Arc<Mutex<SharedState>>,
    job_id: u64,
    request: GenerationRequest,
) -> RunningJob {
    let request_id = request.request_id.clone();
    let cancel_flag = Arc::new(AtomicBool::new(false));
    let cancel_for_thread = Arc::clone(&cancel_flag);
    let tx_for_thread = command_tx.clone();
    let service_for_thread = service.clone();
    let request_id_for_thread = request_id.clone();

    let task_handle = thread::spawn(move || {
        if cancel_for_thread.load(Ordering::SeqCst) {
            let _ = tx_for_thread.send(WorkerMessage::Completion {
                job_id,
                request_id: request_id_for_thread,
                result: Err(LlmError::internal("job cancelled before start")),
                cancelled: true,
            });
            return;
        }

        let result = service_for_thread
            .generate_with_cancel(request, || cancel_for_thread.load(Ordering::SeqCst));
        let cancelled = cancel_for_thread.load(Ordering::SeqCst);

        let _ = tx_for_thread.send(WorkerMessage::Completion {
            job_id,
            request_id: request_id_for_thread,
            result,
            cancelled,
        });
    });

    push_update(
        shared,
        GenerationJobUpdate::running(job_id, request_id.clone()),
    );

    RunningJob {
        job_id,
        request_id,
        cancel_flag,
        cancelled_reported: false,
        task_handle: Some(task_handle),
    }
}

fn join_generation_task(job: &mut RunningJob) {
    if let Some(task_handle) = job.task_handle.take() {
        let _ = task_handle.join();
    }
}

fn push_update(shared: &Arc<Mutex<SharedState>>, update: GenerationJobUpdate) {
    let mut shared = shared
        .lock()
        .expect("generation job state lock poisoned during update");
    shared.state = update.state;
    shared.latest = Some(update.clone());
    shared.updates.push_back(update);
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex, mpsc};
    use std::thread;
    use std::time::{Duration, Instant};

    use crate::domain::{
        GeneratedNote, GenerationCandidate, GenerationMetadata, GenerationMode, GenerationParams,
        GenerationRequest, GenerationResult, LlmError, ModelRef,
    };
    use crate::infra::llm::{LlmProvider, ProviderRegistry};

    use super::{GenerationJobManager, GenerationJobState, GenerationService};

    struct DelayedProvider {
        delays: Arc<Mutex<VecDeque<Duration>>>,
        fail_requests: Arc<Mutex<Vec<String>>>,
    }

    impl LlmProvider for DelayedProvider {
        fn provider_id(&self) -> &str {
            "anthropic"
        }

        fn supports_model(&self, model_id: &str) -> bool {
            model_id == "claude-3-5-sonnet"
        }

        fn generate(&self, request: &GenerationRequest) -> Result<GenerationResult, LlmError> {
            let delay = self
                .delays
                .lock()
                .expect("delay queue lock poisoned")
                .pop_front()
                .unwrap_or(Duration::from_millis(0));
            thread::sleep(delay);

            let mut fail_requests = self.fail_requests.lock().expect("fail queue lock poisoned");
            if let Some(index) = fail_requests
                .iter()
                .position(|id| id == &request.request_id)
            {
                fail_requests.remove(index);
                return Err(LlmError::Timeout);
            }

            Ok(valid_result(&request.request_id))
        }
    }

    struct BlockingProvider {
        entered: Arc<AtomicBool>,
        release_rx: Arc<Mutex<mpsc::Receiver<()>>>,
    }

    impl LlmProvider for BlockingProvider {
        fn provider_id(&self) -> &str {
            "anthropic"
        }

        fn supports_model(&self, model_id: &str) -> bool {
            model_id == "claude-3-5-sonnet"
        }

        fn generate(&self, request: &GenerationRequest) -> Result<GenerationResult, LlmError> {
            self.entered.store(true, Ordering::SeqCst);
            let _ = self
                .release_rx
                .lock()
                .expect("release channel lock poisoned")
                .recv();
            Ok(valid_result(&request.request_id))
        }
    }

    struct ConcurrencyTrackingProvider {
        call_delay: Duration,
        active_calls: AtomicUsize,
        max_concurrent_calls: AtomicUsize,
        total_calls: AtomicUsize,
    }

    impl ConcurrencyTrackingProvider {
        fn new(call_delay: Duration) -> Self {
            Self {
                call_delay,
                active_calls: AtomicUsize::new(0),
                max_concurrent_calls: AtomicUsize::new(0),
                total_calls: AtomicUsize::new(0),
            }
        }
    }

    impl LlmProvider for ConcurrencyTrackingProvider {
        fn provider_id(&self) -> &str {
            "anthropic"
        }

        fn supports_model(&self, model_id: &str) -> bool {
            model_id == "claude-3-5-sonnet"
        }

        fn generate(&self, request: &GenerationRequest) -> Result<GenerationResult, LlmError> {
            self.total_calls.fetch_add(1, Ordering::SeqCst);
            let current = self.active_calls.fetch_add(1, Ordering::SeqCst) + 1;

            loop {
                let max_seen = self.max_concurrent_calls.load(Ordering::SeqCst);
                if current <= max_seen {
                    break;
                }
                if self
                    .max_concurrent_calls
                    .compare_exchange(max_seen, current, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok()
                {
                    break;
                }
            }

            thread::sleep(self.call_delay);
            self.active_calls.fetch_sub(1, Ordering::SeqCst);

            Ok(valid_result(&request.request_id))
        }
    }

    struct SlowCompletionProvider {
        delay: Duration,
        completed: Arc<AtomicBool>,
    }

    impl LlmProvider for SlowCompletionProvider {
        fn provider_id(&self) -> &str {
            "anthropic"
        }

        fn supports_model(&self, model_id: &str) -> bool {
            model_id == "claude-3-5-sonnet"
        }

        fn generate(&self, request: &GenerationRequest) -> Result<GenerationResult, LlmError> {
            thread::sleep(self.delay);
            self.completed.store(true, Ordering::SeqCst);
            Ok(valid_result(&request.request_id))
        }
    }

    fn valid_request(request_id: &str) -> GenerationRequest {
        GenerationRequest {
            request_id: request_id.to_string(),
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
                max_tokens: Some(256),
            },
            references: Vec::new(),
            variation_count: 1,
        }
    }

    fn valid_result(request_id: &str) -> GenerationResult {
        GenerationResult {
            request_id: request_id.to_string(),
            model: ModelRef {
                provider: "anthropic".to_string(),
                model: "claude-3-5-sonnet".to_string(),
            },
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

    fn manager_with_provider(provider: Arc<dyn LlmProvider>) -> GenerationJobManager {
        let mut registry = ProviderRegistry::new();
        registry
            .register_shared(provider)
            .expect("provider registration should succeed");

        GenerationJobManager::new(GenerationService::new(registry))
            .expect("job manager should start worker")
    }

    fn wait_for(
        manager: &GenerationJobManager,
        predicate: impl Fn(GenerationJobState) -> bool,
        timeout: Duration,
    ) {
        let start = Instant::now();
        while start.elapsed() < timeout {
            if predicate(manager.state()) {
                return;
            }
            thread::sleep(Duration::from_millis(5));
        }

        panic!("condition was not met within {:?}", timeout);
    }

    #[test]
    fn submit_generate_runs_provider_on_background_worker() {
        let entered = Arc::new(AtomicBool::new(false));
        let (release_tx, release_rx) = mpsc::channel();

        let provider = Arc::new(BlockingProvider {
            entered: Arc::clone(&entered),
            release_rx: Arc::new(Mutex::new(release_rx)),
        });

        let manager = manager_with_provider(provider);

        let start = Instant::now();
        manager
            .submit_generate(valid_request("req-bg"))
            .expect("submit should succeed");
        assert!(
            start.elapsed() < Duration::from_millis(50),
            "submit_generate should return quickly and not block caller thread"
        );

        let wait_start = Instant::now();
        while wait_start.elapsed() < Duration::from_millis(200) {
            if entered.load(Ordering::SeqCst) {
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }

        assert!(entered.load(Ordering::SeqCst));
        assert_eq!(manager.state(), GenerationJobState::Running);

        release_tx.send(()).expect("release should succeed");

        wait_for(
            &manager,
            |state| state == GenerationJobState::Succeeded,
            Duration::from_millis(500),
        );

        let latest = manager
            .latest_update()
            .expect("latest update should be set after success");
        assert_eq!(latest.request_id, "req-bg");
        assert_eq!(latest.state, GenerationJobState::Succeeded);
        assert!(latest.result.is_some());
    }

    #[test]
    fn submit_generate_cancels_previous_job_when_retriggered() {
        let provider = Arc::new(DelayedProvider {
            delays: Arc::new(Mutex::new(VecDeque::from([
                Duration::from_millis(150),
                Duration::from_millis(10),
            ]))),
            fail_requests: Arc::new(Mutex::new(Vec::new())),
        });
        let manager = manager_with_provider(provider);

        let first_job_id = manager
            .submit_generate(valid_request("req-old"))
            .expect("first submit should succeed");
        thread::sleep(Duration::from_millis(10));
        let second_job_id = manager
            .submit_generate(valid_request("req-new"))
            .expect("second submit should succeed");

        assert!(second_job_id > first_job_id);

        wait_for(
            &manager,
            |state| state == GenerationJobState::Succeeded,
            Duration::from_millis(700),
        );

        thread::sleep(Duration::from_millis(200));

        let latest = manager.latest_update().expect("latest update should exist");
        assert_eq!(latest.request_id, "req-new");
        assert_eq!(latest.state, GenerationJobState::Succeeded);

        let updates = manager.drain_updates();
        assert!(updates.iter().any(|update| {
            update.job_id == first_job_id
                && update.request_id == "req-old"
                && update.state == GenerationJobState::Cancelled
        }));
        assert!(updates.iter().any(|update| {
            update.job_id == second_job_id
                && update.request_id == "req-new"
                && update.state == GenerationJobState::Succeeded
        }));
        assert!(!updates.iter().any(|update| {
            update.job_id == first_job_id && update.state == GenerationJobState::Succeeded
        }));
    }

    #[test]
    fn completion_of_stale_job_does_not_override_latest_result() {
        let provider = Arc::new(DelayedProvider {
            delays: Arc::new(Mutex::new(VecDeque::from([
                Duration::from_millis(180),
                Duration::from_millis(10),
            ]))),
            fail_requests: Arc::new(Mutex::new(Vec::new())),
        });
        let manager = manager_with_provider(provider);

        manager
            .submit_generate(valid_request("req-1"))
            .expect("first submit should succeed");
        thread::sleep(Duration::from_millis(5));
        manager
            .submit_generate(valid_request("req-2"))
            .expect("second submit should succeed");

        wait_for(
            &manager,
            |state| state == GenerationJobState::Succeeded,
            Duration::from_millis(700),
        );

        thread::sleep(Duration::from_millis(250));

        let latest = manager
            .latest_update()
            .expect("latest update should be available");
        assert_eq!(latest.request_id, "req-2");
        assert_eq!(latest.state, GenerationJobState::Succeeded);
        assert_eq!(
            latest
                .result
                .expect("successful update should carry result")
                .request_id,
            "req-2"
        );
    }

    #[test]
    fn failed_job_transitions_to_failed_state() {
        let provider = Arc::new(DelayedProvider {
            delays: Arc::new(Mutex::new(VecDeque::from([Duration::from_millis(5)]))),
            fail_requests: Arc::new(Mutex::new(vec![
                "req-fail".to_string(),
                "req-fail".to_string(),
                "req-fail".to_string(),
            ])),
        });
        let manager = manager_with_provider(provider);

        manager
            .submit_generate(valid_request("req-fail"))
            .expect("submit should succeed");

        wait_for(
            &manager,
            |state| state == GenerationJobState::Failed,
            Duration::from_millis(1200),
        );

        let latest = manager.latest_update().expect("latest update should exist");
        assert_eq!(latest.state, GenerationJobState::Failed);
        assert_eq!(latest.request_id, "req-fail");
        assert!(matches!(latest.error, Some(LlmError::Timeout)));
    }

    #[test]
    fn cancel_active_marks_running_job_as_cancelled() {
        let entered = Arc::new(AtomicBool::new(false));
        let (release_tx, release_rx) = mpsc::channel();

        let provider = Arc::new(BlockingProvider {
            entered: Arc::clone(&entered),
            release_rx: Arc::new(Mutex::new(release_rx)),
        });

        let manager = manager_with_provider(provider);

        let job_id = manager
            .submit_generate(valid_request("req-cancel"))
            .expect("submit should succeed");

        let wait_start = Instant::now();
        while wait_start.elapsed() < Duration::from_millis(200) {
            if entered.load(Ordering::SeqCst) {
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }

        manager
            .cancel_active()
            .expect("cancel command should be accepted");

        wait_for(
            &manager,
            |state| state == GenerationJobState::Cancelled,
            Duration::from_millis(300),
        );

        release_tx.send(()).expect("release should succeed");
        thread::sleep(Duration::from_millis(50));

        let latest = manager.latest_update().expect("latest update should exist");
        assert_eq!(latest.job_id, job_id);
        assert_eq!(latest.request_id, "req-cancel");
        assert_eq!(latest.state, GenerationJobState::Cancelled);
    }

    #[test]
    fn retriggered_generates_do_not_run_provider_calls_in_parallel() {
        let provider = Arc::new(ConcurrencyTrackingProvider::new(Duration::from_millis(120)));
        let manager = manager_with_provider(provider.clone());

        let first_job = manager
            .submit_generate(valid_request("req-1"))
            .expect("first submit should succeed");
        thread::sleep(Duration::from_millis(10));
        let second_job = manager
            .submit_generate(valid_request("req-2"))
            .expect("second submit should succeed");
        thread::sleep(Duration::from_millis(10));
        let third_job = manager
            .submit_generate(valid_request("req-3"))
            .expect("third submit should succeed");

        assert!(second_job > first_job);
        assert!(third_job > second_job);

        wait_for(
            &manager,
            |state| state == GenerationJobState::Succeeded,
            Duration::from_millis(1500),
        );

        thread::sleep(Duration::from_millis(200));

        let latest = manager
            .latest_update()
            .expect("latest update should be available");
        assert_eq!(latest.state, GenerationJobState::Succeeded);
        assert_eq!(latest.request_id, "req-3");
        assert_eq!(
            latest
                .result
                .as_ref()
                .expect("successful update should carry result")
                .request_id,
            "req-3"
        );

        assert_eq!(provider.max_concurrent_calls.load(Ordering::SeqCst), 1);
        assert_eq!(provider.total_calls.load(Ordering::SeqCst), 2);

        let updates = manager.drain_updates();
        assert!(updates.iter().any(|update| {
            update.job_id == first_job
                && update.request_id == "req-1"
                && update.state == GenerationJobState::Cancelled
        }));
        assert!(updates.iter().any(|update| {
            update.job_id == second_job
                && update.request_id == "req-2"
                && update.state == GenerationJobState::Cancelled
        }));
        assert!(updates.iter().any(|update| {
            update.job_id == third_job
                && update.request_id == "req-3"
                && update.state == GenerationJobState::Succeeded
        }));
    }

    #[test]
    fn drop_waits_for_in_flight_generation_thread_to_finish() {
        let completed = Arc::new(AtomicBool::new(false));
        let provider = Arc::new(SlowCompletionProvider {
            delay: Duration::from_millis(150),
            completed: Arc::clone(&completed),
        });
        let manager = manager_with_provider(provider);

        manager
            .submit_generate(valid_request("req-drop"))
            .expect("submit should succeed");

        wait_for(
            &manager,
            |state| state == GenerationJobState::Running || state == GenerationJobState::Succeeded,
            Duration::from_millis(300),
        );

        let drop_started_at = Instant::now();
        drop(manager);
        let drop_elapsed = drop_started_at.elapsed();

        assert!(
            completed.load(Ordering::SeqCst),
            "drop should only return after generation thread completion"
        );
        assert!(
            drop_elapsed >= Duration::from_millis(100),
            "drop should wait for in-flight generation thread"
        );
    }
}
