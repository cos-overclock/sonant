use std::sync::Arc;

use crossbeam_queue::ArrayQueue;

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

pub struct LiveMidiCapture {
    source: Arc<dyn LiveInputEventSource>,
    queue: ArrayQueue<LiveInputEvent>,
}

impl LiveMidiCapture {
    pub fn new(source: Arc<dyn LiveInputEventSource>) -> Self {
        Self::with_capacity(source, DEFAULT_CAPTURE_QUEUE_CAPACITY)
    }

    pub fn with_capacity(source: Arc<dyn LiveInputEventSource>, capacity: usize) -> Self {
        assert!(capacity > 0, "live MIDI capture queue capacity must be > 0");
        Self {
            source,
            queue: ArrayQueue::new(capacity),
        }
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

        let mut events = Vec::with_capacity(max_events);
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
    use super::{LiveInputEvent, LiveInputEventSource, LiveMidiCapture};
    use std::collections::VecDeque;
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
        let capture = LiveMidiCapture::with_capacity(source, 8);

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
        let capture = LiveMidiCapture::with_capacity(source, 2);

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
        let capture = LiveMidiCapture::with_capacity(source, 4);

        assert_eq!(capture.poll_event(), None);

        capture.ingest_available();
        assert_eq!(capture.poll_event(), Some(sample_event(10, 3, 69)));
        assert_eq!(capture.poll_event(), None);
        assert!(capture.poll_events(0).is_empty());
        assert!(capture.poll_events(4).is_empty());
    }
}
