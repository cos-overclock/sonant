use clack_extensions::audio_ports::PluginAudioPorts;
use clack_extensions::gui::PluginGui;
use clack_extensions::note_ports::PluginNotePorts;
use clack_extensions::state::PluginState;
use clack_plugin::events::Match;
use clack_plugin::events::event_types::MidiEvent;
use clack_plugin::events::spaces::CoreEventSpace;
use clack_plugin::prelude::*;
use crossbeam_queue::ArrayQueue;
use std::sync::Arc;

mod audio_ports_extension;
mod gui_extension;
mod note_ports_extension;
mod state_extension;

use gui_extension::SonantGuiController;

const MIDI_EVENT_QUEUE_CAPACITY: usize = 2048;

pub struct SonantPlugin;

impl Plugin for SonantPlugin {
    type AudioProcessor<'a> = SonantAudioProcessor<'a>;
    type Shared<'a> = SonantShared;
    type MainThread<'a> = SonantPluginMainThread<'a>;

    fn declare_extensions(builder: &mut PluginExtensions<Self>, _shared: Option<&SonantShared>) {
        builder
            .register::<PluginGui>()
            .register::<PluginAudioPorts>()
            .register::<PluginNotePorts>()
            .register::<PluginState>();
    }
}

impl DefaultPluginFactory for SonantPlugin {
    fn get_descriptor() -> PluginDescriptor {
        use clack_plugin::plugin::features::*;

        PluginDescriptor::new("com.sonant.midi_generator", "Sonant")
            .with_vendor("Sonant")
            .with_url("https://example.com/sonant")
            .with_version("0.1.0")
            .with_description("Sonant GPUI CLAP PoC")
            .with_features([NOTE_EFFECT, UTILITY])
    }

    fn new_shared(_host: HostSharedHandle<'_>) -> Result<Self::Shared<'_>, PluginError> {
        Ok(SonantShared::new())
    }

    fn new_main_thread<'a>(
        _host: HostMainThreadHandle<'a>,
        shared: &'a Self::Shared<'a>,
    ) -> Result<Self::MainThread<'a>, PluginError> {
        Ok(SonantPluginMainThread {
            shared,
            gui: SonantGuiController::default(),
        })
    }
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
struct RtMidiEvent {
    time: u32,
    port_index: u16,
    data: [u8; 3],
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub struct LiveInputEvent {
    pub time: u32,
    pub port_index: u16,
    pub data: [u8; 3],
}

impl RtMidiEvent {
    fn from_midi(event: &MidiEvent) -> Self {
        Self {
            time: event.time(),
            port_index: event.port_index(),
            data: event.data(),
        }
    }

    fn to_clap(self) -> MidiEvent {
        MidiEvent::new(self.time, self.port_index, self.data)
    }
}

fn map_input_event(event: &UnknownEvent) -> Option<RtMidiEvent> {
    match event.as_core_event() {
        Some(CoreEventSpace::Midi(event)) => Some(RtMidiEvent::from_midi(event)),
        Some(CoreEventSpace::NoteOn(event)) => note_event_to_midi(
            event.time(),
            event.port_index(),
            event.channel(),
            event.key(),
            event.velocity(),
            true,
        ),
        Some(CoreEventSpace::NoteOff(event)) => note_event_to_midi(
            event.time(),
            event.port_index(),
            event.channel(),
            event.key(),
            event.velocity(),
            false,
        ),
        Some(CoreEventSpace::NoteChoke(event)) => note_event_to_midi(
            event.time(),
            event.port_index(),
            event.channel(),
            event.key(),
            0.0,
            false,
        ),
        _ => None,
    }
}

fn note_event_to_midi(
    time: u32,
    port_index: Match<u16>,
    channel: Match<u16>,
    key: Match<u16>,
    velocity: f64,
    is_note_on: bool,
) -> Option<RtMidiEvent> {
    let port_index = port_index.into_specific()?;
    let channel = channel.into_specific()?;
    let key = key.into_specific()?;

    if channel > 0x0F || key > 0x7F {
        return None;
    }

    let status = if is_note_on { 0x90 } else { 0x80 } | ((channel as u8) & 0x0F);

    Some(RtMidiEvent {
        time,
        port_index,
        data: [status, key as u8, velocity_to_midi_byte(velocity)],
    })
}

fn velocity_to_midi_byte(velocity: f64) -> u8 {
    (velocity.clamp(0.0, 1.0) * 127.0).round() as u8
}

struct MidiBridge {
    live_input_queue: ArrayQueue<RtMidiEvent>,
    app_input_queue: ArrayQueue<RtMidiEvent>,
    generated_output_queue: ArrayQueue<RtMidiEvent>,
}

impl MidiBridge {
    fn new(capacity: usize) -> Self {
        Self {
            live_input_queue: ArrayQueue::new(capacity),
            app_input_queue: ArrayQueue::new(capacity),
            generated_output_queue: ArrayQueue::new(capacity),
        }
    }

    fn push_live_input(&self, event: RtMidiEvent) {
        let _ = self.live_input_queue.force_push(event);
    }

    fn pop_live_input(&self) -> Option<RtMidiEvent> {
        self.live_input_queue.pop()
    }

    fn push_app_input(&self, event: RtMidiEvent) {
        let _ = self.app_input_queue.force_push(event);
    }

    fn pop_app_input(&self) -> Option<RtMidiEvent> {
        self.app_input_queue.pop()
    }

    fn push_generated_output(&self, event: RtMidiEvent) {
        let _ = self.generated_output_queue.force_push(event);
    }

    fn pop_generated_output(&self) -> Option<RtMidiEvent> {
        self.generated_output_queue.pop()
    }

    fn pop_latest_generated_or(&self, mut fallback: Option<RtMidiEvent>) -> Option<RtMidiEvent> {
        while let Some(latest_event) = self.generated_output_queue.pop() {
            fallback = Some(latest_event);
        }
        fallback
    }

    fn reset(&self) {
        while self.live_input_queue.pop().is_some() {}
        while self.app_input_queue.pop().is_some() {}
        while self.generated_output_queue.pop().is_some() {}
    }
}

pub struct SonantShared {
    midi_bridge: Arc<MidiBridge>,
}

impl SonantShared {
    fn new() -> Self {
        Self {
            midi_bridge: Arc::new(MidiBridge::new(MIDI_EVENT_QUEUE_CAPACITY)),
        }
    }

    fn reset_queues(&self) {
        self.midi_bridge.reset();
    }

    fn flush_live_input_to_app(&self) {
        while let Some(event) = self.midi_bridge.pop_live_input() {
            self.midi_bridge.push_app_input(event);
        }
    }

    pub fn pop_live_input_event(&self) -> Option<LiveInputEvent> {
        self.midi_bridge
            .pop_app_input()
            .map(|event| LiveInputEvent {
                time: event.time,
                port_index: event.port_index,
                data: event.data,
            })
    }

    #[allow(dead_code)]
    pub fn enqueue_generated_raw_midi(&self, time: u32, port_index: u16, data: [u8; 3]) {
        self.midi_bridge.push_generated_output(RtMidiEvent {
            time,
            port_index,
            data,
        });
    }
}

impl From<LiveInputEvent> for crate::app::LiveInputEvent {
    fn from(event: LiveInputEvent) -> Self {
        crate::app::LiveInputEvent {
            time: event.time,
            port_index: event.port_index,
            data: event.data,
        }
    }
}

impl crate::app::LiveInputEventSource for SonantShared {
    fn try_pop_live_input_event(&self) -> Option<crate::app::LiveInputEvent> {
        self.pop_live_input_event().map(Into::into)
    }
}

impl PluginShared<'_> for SonantShared {}

pub struct SonantPluginMainThread<'a> {
    shared: &'a SonantShared,
    gui: SonantGuiController,
}

impl<'a> PluginMainThread<'a, SonantShared> for SonantPluginMainThread<'a> {
    fn on_main_thread(&mut self) {
        self.shared.flush_live_input_to_app();
    }
}

pub struct SonantAudioProcessor<'a> {
    host: HostAudioProcessorHandle<'a>,
    midi_bridge: Arc<MidiBridge>,
    pending_output_event: Option<RtMidiEvent>,
}

impl<'a> PluginAudioProcessor<'a, SonantShared, SonantPluginMainThread<'a>>
    for SonantAudioProcessor<'a>
{
    fn activate(
        host: HostAudioProcessorHandle<'a>,
        _main_thread: &mut SonantPluginMainThread<'a>,
        shared: &'a SonantShared,
        _audio_config: PluginAudioConfiguration,
    ) -> Result<Self, PluginError> {
        shared.reset_queues();
        Ok(Self {
            host,
            midi_bridge: Arc::clone(&shared.midi_bridge),
            pending_output_event: None,
        })
    }

    fn process(
        &mut self,
        _process: Process,
        _audio: Audio,
        events: Events,
    ) -> Result<ProcessStatus, PluginError> {
        let mut received_live_input = false;
        for event in events.input.iter() {
            if let Some(midi_event) = map_input_event(event) {
                self.midi_bridge.push_live_input(midi_event);
                received_live_input = true;
            }
        }

        if received_live_input {
            self.host.request_callback();
        }

        if let Some(event) = self.pending_output_event.take()
            && events.output.try_push(event.to_clap()).is_err()
        {
            // Host output is still saturated. Keep only the latest generated event.
            self.pending_output_event = self.midi_bridge.pop_latest_generated_or(Some(event));
            return Ok(ProcessStatus::Continue);
        }

        while let Some(event) = self.midi_bridge.pop_generated_output() {
            if events.output.try_push(event.to_clap()).is_err() {
                // Host output buffer is saturated. Keep the newest event and drop stale ones.
                self.pending_output_event = self.midi_bridge.pop_latest_generated_or(Some(event));
                break;
            }
        }

        Ok(ProcessStatus::Continue)
    }

    fn deactivate(self, _main_thread: &mut SonantPluginMainThread<'a>) {
        self.midi_bridge.reset();
    }

    fn reset(&mut self) {
        self.pending_output_event = None;
        self.midi_bridge.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clack_plugin::events::event_types::{NoteOffEvent, NoteOnEvent};
    use std::sync::Arc;

    #[test]
    fn map_input_event_converts_note_events_to_midi() {
        let note_on = NoteOnEvent::new(12, Pckn::new(0u16, 2u16, 64u16, 0u32), 0.5);
        let mapped_on = map_input_event(note_on.as_ref()).expect("note on should convert");
        assert_eq!(
            mapped_on,
            RtMidiEvent {
                time: 12,
                port_index: 0,
                data: [0x92, 64, 64],
            }
        );

        let note_off = NoteOffEvent::new(15, Pckn::new(0u16, 2u16, 64u16, 0u32), 0.25);
        let mapped_off = map_input_event(note_off.as_ref()).expect("note off should convert");
        assert_eq!(
            mapped_off,
            RtMidiEvent {
                time: 15,
                port_index: 0,
                data: [0x82, 64, 32],
            }
        );
    }

    #[test]
    fn map_input_event_ignores_non_specific_note_targets() {
        let wildcard_note =
            NoteOnEvent::new(0, Pckn::new(Match::<u16>::All, 1u16, 64u16, 0u32), 1.0);

        assert!(map_input_event(wildcard_note.as_ref()).is_none());
    }

    #[test]
    fn map_input_event_passes_through_midi_events() {
        let midi_event = MidiEvent::new(8, 1, [0x90, 60, 100]);
        let mapped = map_input_event(midi_event.as_ref()).expect("midi events should pass through");

        assert_eq!(
            mapped,
            RtMidiEvent {
                time: 8,
                port_index: 1,
                data: [0x90, 60, 100],
            }
        );
    }

    #[test]
    fn midi_bridge_drops_oldest_when_queue_is_full() {
        let bridge = MidiBridge::new(2);
        let event_1 = RtMidiEvent {
            time: 1,
            port_index: 0,
            data: [0x90, 60, 100],
        };
        let event_2 = RtMidiEvent {
            time: 2,
            port_index: 0,
            data: [0x90, 61, 100],
        };
        let event_3 = RtMidiEvent {
            time: 3,
            port_index: 0,
            data: [0x90, 62, 100],
        };

        bridge.push_generated_output(event_1);
        bridge.push_generated_output(event_2);
        bridge.push_generated_output(event_3);

        assert_eq!(bridge.pop_generated_output(), Some(event_2));
        assert_eq!(bridge.pop_generated_output(), Some(event_3));
        assert_eq!(bridge.pop_generated_output(), None);
    }

    #[test]
    fn midi_bridge_reset_clears_both_queues() {
        let bridge = MidiBridge::new(2);
        bridge.push_live_input(RtMidiEvent {
            time: 1,
            port_index: 0,
            data: [0x90, 60, 1],
        });
        bridge.push_generated_output(RtMidiEvent {
            time: 2,
            port_index: 0,
            data: [0x80, 60, 0],
        });

        bridge.reset();

        assert_eq!(bridge.pop_live_input(), None);
        assert_eq!(bridge.pop_app_input(), None);
        assert_eq!(bridge.pop_generated_output(), None);
    }

    #[test]
    fn midi_bridge_flushes_live_input_into_app_queue() {
        let shared = SonantShared::new();

        shared.midi_bridge.push_live_input(RtMidiEvent {
            time: 10,
            port_index: 2,
            data: [0x90, 60, 100],
        });
        shared.flush_live_input_to_app();

        assert_eq!(
            shared.pop_live_input_event(),
            Some(LiveInputEvent {
                time: 10,
                port_index: 2,
                data: [0x90, 60, 100],
            })
        );
        assert_eq!(shared.pop_live_input_event(), None);
    }

    #[test]
    fn pop_latest_generated_or_returns_newest_queued_event() {
        let bridge = MidiBridge::new(4);
        let fallback = RtMidiEvent {
            time: 1,
            port_index: 0,
            data: [0x90, 60, 1],
        };
        let newest = RtMidiEvent {
            time: 3,
            port_index: 0,
            data: [0x90, 62, 3],
        };

        bridge.push_generated_output(RtMidiEvent {
            time: 2,
            port_index: 0,
            data: [0x90, 61, 2],
        });
        bridge.push_generated_output(newest);

        assert_eq!(bridge.pop_latest_generated_or(Some(fallback)), Some(newest));
        assert_eq!(bridge.pop_generated_output(), None);
    }

    #[test]
    fn pop_latest_generated_or_keeps_fallback_when_queue_is_empty() {
        let bridge = MidiBridge::new(2);
        let fallback = RtMidiEvent {
            time: 7,
            port_index: 1,
            data: [0x80, 64, 0],
        };

        assert_eq!(
            bridge.pop_latest_generated_or(Some(fallback)),
            Some(fallback)
        );
    }

    #[test]
    fn live_capture_path_exposes_clap_live_input_to_app_layer() {
        let shared = Arc::new(SonantShared::new());
        shared.midi_bridge.push_live_input(RtMidiEvent {
            time: 42,
            port_index: 1,
            data: [0x92, 65, 127],
        });
        shared.flush_live_input_to_app();

        let source: Arc<dyn crate::app::LiveInputEventSource> = shared.clone();
        let capture = crate::app::LiveMidiCapture::with_capacity(source, 8);

        assert_eq!(capture.ingest_available(), 1);
        assert_eq!(
            capture.poll_event(),
            Some(crate::app::LiveInputEvent {
                time: 42,
                port_index: 1,
                data: [0x92, 65, 127],
            })
        );
        assert_eq!(capture.poll_event(), None);
    }
}

clack_export_entry!(SinglePluginEntry<SonantPlugin>);
