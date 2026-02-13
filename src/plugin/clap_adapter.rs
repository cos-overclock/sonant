use clack_extensions::audio_ports::{
    AudioPortFlags, AudioPortInfo, AudioPortInfoWriter, AudioPortType, PluginAudioPorts,
    PluginAudioPortsImpl,
};
use clack_extensions::gui::{GuiApiType, GuiConfiguration, GuiSize, PluginGui, PluginGuiImpl};
use clack_extensions::state::{PluginState, PluginStateImpl};
use clack_plugin::prelude::*;
use clack_plugin::stream::{InputStream, OutputStream};
use std::ffi::CStr;
use std::io::{Read, Write};
use std::mem::MaybeUninit;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

pub struct SonantPlugin;

impl Plugin for SonantPlugin {
    type AudioProcessor<'a> = SonantAudioProcessor;
    type Shared<'a> = SonantShared;
    type MainThread<'a> = SonantPluginMainThread<'a>;

    fn declare_extensions(builder: &mut PluginExtensions<Self>, _shared: Option<&SonantShared>) {
        builder
            .register::<PluginGui>()
            .register::<PluginAudioPorts>()
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
            .with_features([AUDIO_EFFECT, STEREO, UTILITY])
    }

    fn new_shared(_host: HostSharedHandle<'_>) -> Result<Self::Shared<'_>, PluginError> {
        Ok(SonantShared)
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

pub struct SonantShared;

impl PluginShared<'_> for SonantShared {}

pub struct SonantPluginMainThread<'a> {
    #[allow(dead_code)]
    shared: &'a SonantShared,
    gui: SonantGuiController,
}

impl<'a> PluginMainThread<'a, SonantShared> for SonantPluginMainThread<'a> {}

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

pub struct SonantAudioProcessor;

impl<'a> PluginAudioProcessor<'a, SonantShared, SonantPluginMainThread<'a>>
    for SonantAudioProcessor
{
    fn activate(
        _host: HostAudioProcessorHandle<'a>,
        _main_thread: &mut SonantPluginMainThread<'a>,
        _shared: &'a SonantShared,
        _audio_config: PluginAudioConfiguration,
    ) -> Result<Self, PluginError> {
        Ok(Self)
    }

    fn process(
        &mut self,
        _process: Process,
        _audio: Audio,
        _events: Events,
    ) -> Result<ProcessStatus, PluginError> {
        Ok(ProcessStatus::Continue)
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
    fn count(&mut self, _is_input: bool) -> u32 {
        1
    }

    fn get(&mut self, index: u32, _is_input: bool, writer: &mut AudioPortInfoWriter) {
        if index == 0 {
            writer.set(&AudioPortInfo {
                id: ClapId::new(0),
                name: b"main",
                channel_count: 2,
                flags: AudioPortFlags::IS_MAIN,
                port_type: Some(AudioPortType::STEREO),
                in_place_pair: None,
            });
        }
    }
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

clack_export_entry!(SinglePluginEntry<SonantPlugin>);
