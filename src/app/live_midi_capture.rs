use std::num::NonZeroUsize;
use std::sync::Arc;

use crossbeam_queue::ArrayQueue;
use thiserror::Error;

const DEFAULT_CAPTURE_QUEUE_CAPACITY: usize = 2048;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LiveInputEvent {
    pub time: u32,
    pub port_index: u16,
    pub data: [u8; 3],
}

pub trait LiveInputEventSource: Send + Sync {
    fn try_pop_live_input_event(&self) -> Option<LiveInputEvent>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum LiveMidiCaptureConfigError {
    #[error("live MIDI capture queue capacity must be greater than zero")]
    ZeroCapacity,
}

pub struct LiveMidiCapture {
    source: Arc<dyn LiveInputEventSource>,
    queue: ArrayQueue<LiveInputEvent>,
}

impl LiveMidiCapture {
    pub fn new(source: Arc<dyn LiveInputEventSource>) -> Self {
        let default_capacity = NonZeroUsize::new(DEFAULT_CAPTURE_QUEUE_CAPACITY)
            .expect("default live MIDI capture queue capacity must be non-zero");
        Self::with_capacity(source, default_capacity)
    }

    pub fn with_capacity(source: Arc<dyn LiveInputEventSource>, capacity: NonZeroUsize) -> Self {
        Self {
            source,
            queue: ArrayQueue::new(capacity.get()),
        }
    }

    pub fn try_with_capacity(
        source: Arc<dyn LiveInputEventSource>,
        capacity: usize,
    ) -> Result<Self, LiveMidiCaptureConfigError> {
        let capacity =
            NonZeroUsize::new(capacity).ok_or(LiveMidiCaptureConfigError::ZeroCapacity)?;
        Ok(Self::with_capacity(source, capacity))
    }

    pub fn ingest_available(&self) -> usize {
        let mut ingested = 0;
        while let Some(event) = self.source.try_pop_live_input_event() {
            let _ = self.queue.force_push(event);
            ingested += 1;
        }
        ingested
    }

    pub fn poll_event(&self) -> Option<LiveInputEvent> {
        self.queue.pop()
    }

    pub fn poll_events(&self, max_events: usize) -> Vec<LiveInputEvent> {
        if max_events == 0 {
            return Vec::new();
        }

        let capacity = std::cmp::min(max_events, self.queue.capacity());
        let mut events = Vec::with_capacity(capacity);
        while events.len() < max_events {
            let Some(event) = self.queue.pop() else {
                break;
            };
            events.push(event);
        }
        events
    }
}

#[cfg(test)]
mod tests {
    use super::{
        LiveInputEvent, LiveInputEventSource, LiveMidiCapture, LiveMidiCaptureConfigError,
    };
    use std::collections::VecDeque;
    use std::num::NonZeroUsize;
    use std::sync::{Arc, Mutex};

    struct StubLiveInputSource {
        events: Mutex<VecDeque<LiveInputEvent>>,
    }

    impl StubLiveInputSource {
        fn new(events: Vec<LiveInputEvent>) -> Self {
            Self {
                events: Mutex::new(events.into()),
            }
        }
    }

    impl LiveInputEventSource for StubLiveInputSource {
        fn try_pop_live_input_event(&self) -> Option<LiveInputEvent> {
            self.events
                .lock()
                .expect("stub live-input source lock poisoned")
                .pop_front()
        }
    }

    fn sample_event(time: u32, channel_zero_based: u8, note: u8) -> LiveInputEvent {
        LiveInputEvent {
            time,
            port_index: 0,
            data: [0x90 | (channel_zero_based & 0x0F), note, 100],
        }
    }

    #[test]
    fn ingest_available_transfers_all_current_source_events() {
        let source = Arc::new(StubLiveInputSource::new(vec![
            sample_event(1, 0, 60),
            sample_event(2, 1, 62),
            sample_event(3, 2, 64),
        ]));
        let capture = LiveMidiCapture::with_capacity(
            source,
            NonZeroUsize::new(8).expect("test capacity must be non-zero"),
        );

        let ingested = capture.ingest_available();
        assert_eq!(ingested, 3);

        assert_eq!(
            capture.poll_events(8),
            vec![
                sample_event(1, 0, 60),
                sample_event(2, 1, 62),
                sample_event(3, 2, 64),
            ]
        );
    }

    #[test]
    fn ingest_available_keeps_latest_events_when_capacity_is_exceeded() {
        let source = Arc::new(StubLiveInputSource::new(vec![
            sample_event(1, 0, 60),
            sample_event(2, 0, 61),
            sample_event(3, 0, 62),
        ]));
        let capture = LiveMidiCapture::with_capacity(
            source,
            NonZeroUsize::new(2).expect("test capacity must be non-zero"),
        );

        let ingested = capture.ingest_available();
        assert_eq!(ingested, 3);

        assert_eq!(
            capture.poll_events(8),
            vec![sample_event(2, 0, 61), sample_event(3, 0, 62)]
        );
    }

    #[test]
    fn poll_event_and_poll_events_are_non_blocking() {
        let source = Arc::new(StubLiveInputSource::new(vec![sample_event(10, 3, 69)]));
        let capture = LiveMidiCapture::with_capacity(
            source,
            NonZeroUsize::new(4).expect("test capacity must be non-zero"),
        );

        assert_eq!(capture.poll_event(), None);

        capture.ingest_available();
        assert_eq!(capture.poll_event(), Some(sample_event(10, 3, 69)));
        assert_eq!(capture.poll_event(), None);
        assert!(capture.poll_events(0).is_empty());
        assert!(capture.poll_events(4).is_empty());
    }

    #[test]
    fn try_with_capacity_rejects_zero() {
        let source = Arc::new(StubLiveInputSource::new(Vec::new()));
        assert!(matches!(
            LiveMidiCapture::try_with_capacity(source, 0),
            Err(LiveMidiCaptureConfigError::ZeroCapacity)
        ));
    }
}
