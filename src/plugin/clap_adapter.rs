use clack_extensions::audio_ports::{AudioPortInfoWriter, PluginAudioPorts, PluginAudioPortsImpl};
use clack_extensions::gui::{GuiApiType, GuiConfiguration, GuiSize, PluginGui, PluginGuiImpl};
use clack_extensions::note_ports::{
    NoteDialect, NoteDialects, NotePortInfo, NotePortInfoWriter, PluginNotePorts,
    PluginNotePortsImpl,
};
use clack_extensions::state::{PluginState, PluginStateImpl};
use clack_plugin::events::Match;
use clack_plugin::events::event_types::MidiEvent;
use clack_plugin::events::spaces::CoreEventSpace;
use clack_plugin::prelude::*;
use clack_plugin::stream::{InputStream, OutputStream};
use crossbeam_queue::ArrayQueue;
use std::ffi::CStr;
use std::io::{Read, Write};
use std::mem::MaybeUninit;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

const MIDI_EVENT_QUEUE_CAPACITY: usize = 2048;
const NOTE_PORT_INDEX_MAIN: u32 = 0;
const NOTE_PORT_ID_IN: u32 = 0;
const NOTE_PORT_ID_OUT: u32 = 1;
const NOTE_PORT_NAME_IN: &[u8] = b"midi_in";
const NOTE_PORT_NAME_OUT: &[u8] = b"midi_out";

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

const STATE_MAGIC: &[u8; 8] = b"SONANT01";
const STATE_VERSION: u32 = 1;

impl PluginStateImpl for SonantPluginMainThread<'_> {
    fn save(&mut self, output: &mut OutputStream) -> Result<(), PluginError> {
        output.write_all(STATE_MAGIC)?;
        output.write_all(&STATE_VERSION.to_le_bytes())?;
        Ok(())
    }

    fn load(&mut self, input: &mut InputStream) -> Result<(), PluginError> {
        let mut bytes = Vec::new();
        input.read_to_end(&mut bytes)?;

        // Backward compatibility: accept empty state from older plugin builds.
        if bytes.is_empty() {
            return Ok(());
        }

        if bytes.len() < STATE_MAGIC.len() + 4 {
            return Err(PluginError::Message("Invalid state payload"));
        }

        if &bytes[..STATE_MAGIC.len()] != STATE_MAGIC {
            return Err(PluginError::Message("Invalid state magic"));
        }

        let version_start = STATE_MAGIC.len();
        let version_end = version_start + 4;
        let mut version_bytes = [0u8; 4];
        version_bytes.copy_from_slice(&bytes[version_start..version_end]);
        let version = u32::from_le_bytes(version_bytes);

        if version > STATE_VERSION {
            return Err(PluginError::Message("Unsupported state version"));
        }

        Ok(())
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

impl PluginGuiImpl for SonantPluginMainThread<'_> {
    fn is_api_supported(&mut self, configuration: GuiConfiguration) -> bool {
        let Some(api_type) = GuiApiType::default_for_current_platform() else {
            return false;
        };

        configuration.api_type == api_type && configuration.is_floating
    }

    fn get_preferred_api(&mut self) -> Option<GuiConfiguration<'_>> {
        let api_type = GuiApiType::default_for_current_platform()?;
        Some(GuiConfiguration {
            api_type,
            is_floating: true,
        })
    }

    fn create(&mut self, configuration: GuiConfiguration) -> Result<(), PluginError> {
        if self.is_api_supported(configuration) {
            Ok(())
        } else {
            Err(PluginError::Message("Only floating GUI is supported"))
        }
    }

    fn destroy(&mut self) {
        self.gui.destroy();
    }

    fn set_scale(&mut self, _scale: f64) -> Result<(), PluginError> {
        Ok(())
    }

    fn get_size(&mut self) -> Option<GuiSize> {
        Some(GuiSize {
            width: 640,
            height: 420,
        })
    }

    fn set_size(&mut self, _size: GuiSize) -> Result<(), PluginError> {
        Ok(())
    }

    fn set_parent(&mut self, _window: clack_extensions::gui::Window) -> Result<(), PluginError> {
        Ok(())
    }

    fn set_transient(&mut self, _window: clack_extensions::gui::Window) -> Result<(), PluginError> {
        Ok(())
    }

    fn show(&mut self) -> Result<(), PluginError> {
        self.gui.show()
    }

    fn hide(&mut self) -> Result<(), PluginError> {
        self.gui.hide();
        Ok(())
    }
}

impl PluginAudioPortsImpl for SonantPluginMainThread<'_> {
    fn count(&mut self, is_input: bool) -> u32 {
        audio_port_count(is_input)
    }

    fn get(&mut self, _index: u32, _is_input: bool, _writer: &mut AudioPortInfoWriter) {}
}

impl PluginNotePortsImpl for SonantPluginMainThread<'_> {
    fn count(&mut self, is_input: bool) -> u32 {
        note_port_count(is_input)
    }

    fn get(&mut self, index: u32, is_input: bool, writer: &mut NotePortInfoWriter) {
        if let Some(note_port) = note_port_definition(index, is_input) {
            writer.set(&note_port);
        }
    }
}

const fn audio_port_count(_is_input: bool) -> u32 {
    0
}

const fn note_port_count(_is_input: bool) -> u32 {
    1
}

fn note_port_definition(index: u32, is_input: bool) -> Option<NotePortInfo<'static>> {
    if index != NOTE_PORT_INDEX_MAIN {
        return None;
    }

    let (id, name) = if is_input {
        (NOTE_PORT_ID_IN, NOTE_PORT_NAME_IN)
    } else {
        (NOTE_PORT_ID_OUT, NOTE_PORT_NAME_OUT)
    };

    Some(NotePortInfo {
        id: ClapId::new(id),
        name,
        supported_dialects: NoteDialects::MIDI,
        preferred_dialect: Some(NoteDialect::Midi),
    })
}

#[derive(Default)]
struct SonantGuiController {
    state: HelperState,
}

#[derive(Default)]
struct HelperState {
    child: Option<Child>,
    launched_at: Option<Instant>,
}

impl SonantGuiController {
    fn show(&mut self) -> Result<(), PluginError> {
        reap_finished_helper(&mut self.state);

        if self.state.child.is_some() {
            return Ok(());
        }

        let helper_path = resolve_helper_binary_path().ok_or(PluginError::Message(
            "Could not resolve SonantGUIHelper path",
        ))?;

        let child = Command::new(helper_path)
            .arg("--gpui-helper")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|_| PluginError::Message("Failed to launch SonantGUIHelper"))?;

        self.state.child = Some(child);
        self.state.launched_at = Some(Instant::now());
        Ok(())
    }

    fn hide(&mut self) {
        reap_finished_helper(&mut self.state);

        // Some hosts invoke hide right after show during GUI negotiation.
        // Keep the helper alive briefly to avoid immediate window flicker/close.
        if self
            .state
            .launched_at
            .map(|t| t.elapsed() < Duration::from_secs(2))
            .unwrap_or(false)
        {
            return;
        }

        stop_helper(&mut self.state);
    }

    fn destroy(&mut self) {
        reap_finished_helper(&mut self.state);

        if self
            .state
            .launched_at
            .map(|t| t.elapsed() < Duration::from_secs(2))
            .unwrap_or(false)
        {
            return;
        }

        stop_helper(&mut self.state);
    }
}

impl Drop for SonantGuiController {
    fn drop(&mut self) {
        self.destroy();
    }
}

fn reap_finished_helper(state: &mut HelperState) {
    let finished = state
        .child
        .as_mut()
        .and_then(|child| child.try_wait().ok())
        .flatten()
        .is_some();

    if finished {
        state.child = None;
        state.launched_at = None;
    }
}

fn stop_helper(state: &mut HelperState) {
    if let Some(mut child) = state.child.take() {
        let _ = child.kill();
        let _ = child.wait();
    }
    state.launched_at = None;
}

fn resolve_helper_binary_path() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("SONANT_GUI_HELPER_PATH") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }

    let dylib_path = current_library_path()?;
    let helper = dylib_path.parent()?.join("SonantGUIHelper");
    helper.is_file().then_some(helper)
}

#[cfg(target_family = "unix")]
fn current_library_path() -> Option<PathBuf> {
    unsafe {
        let mut info = MaybeUninit::<libc::Dl_info>::zeroed();
        let symbol = current_library_path as *const () as *const libc::c_void;

        if libc::dladdr(symbol, info.as_mut_ptr()) == 0 {
            return None;
        }

        let info = info.assume_init();
        if info.dli_fname.is_null() {
            return None;
        }

        let path = CStr::from_ptr(info.dli_fname).to_str().ok()?;
        Some(PathBuf::from(path))
    }
}

#[cfg(not(target_family = "unix"))]
fn current_library_path() -> Option<PathBuf> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use clack_plugin::events::event_types::{NoteOffEvent, NoteOnEvent};

    #[test]
    fn note_port_definition_exposes_midi_in_and_out() {
        assert_eq!(audio_port_count(true), 0);
        assert_eq!(audio_port_count(false), 0);
        assert_eq!(note_port_count(true), 1);
        assert_eq!(note_port_count(false), 1);

        let input = note_port_definition(NOTE_PORT_INDEX_MAIN, true)
            .expect("input note port must be defined");
        assert_eq!(input.id, ClapId::new(NOTE_PORT_ID_IN));
        assert_eq!(input.name, NOTE_PORT_NAME_IN);
        assert_eq!(input.preferred_dialect, Some(NoteDialect::Midi));
        assert!(input.supported_dialects.supports(NoteDialect::Midi));

        let output = note_port_definition(NOTE_PORT_INDEX_MAIN, false)
            .expect("output note port must be defined");
        assert_eq!(output.id, ClapId::new(NOTE_PORT_ID_OUT));
        assert_eq!(output.name, NOTE_PORT_NAME_OUT);
        assert_eq!(output.preferred_dialect, Some(NoteDialect::Midi));
        assert!(output.supported_dialects.supports(NoteDialect::Midi));
    }

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
}

clack_export_entry!(SinglePluginEntry<SonantPlugin>);
