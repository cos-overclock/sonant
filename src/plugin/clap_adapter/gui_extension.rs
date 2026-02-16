use clack_extensions::gui::{GuiApiType, GuiConfiguration, GuiSize, PluginGuiImpl, Window};
use clack_plugin::prelude::PluginError;
use std::ffi::CStr;
use std::mem::MaybeUninit;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use crate::app::LiveInputEvent;
#[cfg(target_family = "unix")]
use crate::app::{LIVE_INPUT_IPC_SOCKET_ENV, LiveInputIpcSender};

use super::SonantPluginMainThread;

#[derive(Default)]
pub(super) struct SonantGuiController {
    state: HelperState,
}

#[derive(Default)]
struct HelperState {
    child: Option<Child>,
    #[cfg(target_family = "unix")]
    live_input_sender: Option<LiveInputIpcSender>,
    launched_at: Option<Instant>,
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

    fn set_parent(&mut self, _window: Window) -> Result<(), PluginError> {
        Ok(())
    }

    fn set_transient(&mut self, _window: Window) -> Result<(), PluginError> {
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

impl SonantGuiController {
    fn show(&mut self) -> Result<(), PluginError> {
        reap_finished_helper(&mut self.state);

        if self.state.child.is_some() {
            return Ok(());
        }

        let helper_path = resolve_helper_binary_path().ok_or(PluginError::Message(
            "Could not resolve SonantGUIHelper path",
        ))?;
        let mut command = Command::new(helper_path);
        command
            .arg("--gpui-helper")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit());

        #[cfg(target_family = "unix")]
        let live_input_sender = {
            let live_input_socket_path = helper_live_input_socket_path();
            let sender = LiveInputIpcSender::new(&live_input_socket_path).map_err(|_| {
                PluginError::Message("Failed to initialize helper live-input socket")
            })?;
            command.env(LIVE_INPUT_IPC_SOCKET_ENV, &live_input_socket_path);
            sender
        };

        let child = command
            .spawn()
            .map_err(|_| PluginError::Message("Failed to launch SonantGUIHelper"))?;

        self.state.child = Some(child);
        #[cfg(target_family = "unix")]
        {
            self.state.live_input_sender = Some(live_input_sender);
        }
        self.state.launched_at = Some(Instant::now());
        Ok(())
    }

    pub(super) fn send_live_input_events(&mut self, events: &[LiveInputEvent]) {
        #[cfg(not(target_family = "unix"))]
        {
            let _ = events;
        }
        #[cfg(target_family = "unix")]
        {
            if events.is_empty() {
                return;
            }
            if let Some(sender) = self.state.live_input_sender.as_ref() {
                sender.send_events(events);
            }
        }
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
        #[cfg(target_family = "unix")]
        {
            state.live_input_sender = None;
        }
        state.launched_at = None;
    }
}

fn stop_helper(state: &mut HelperState) {
    if let Some(mut child) = state.child.take() {
        let _ = child.kill();
        let _ = child.wait();
    }
    #[cfg(target_family = "unix")]
    {
        state.live_input_sender = None;
    }
    state.launched_at = None;
}

#[cfg(target_family = "unix")]
fn helper_live_input_socket_path() -> PathBuf {
    use std::env::temp_dir;
    use std::time::{SystemTime, UNIX_EPOCH};

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    temp_dir().join(format!("snt-live-in-{}-{nonce:x}.sock", std::process::id()))
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

#[cfg(all(test, target_family = "unix"))]
mod tests {
    use super::helper_live_input_socket_path;

    #[test]
    fn helper_live_input_socket_path_uses_temp_dir_and_fits_unix_socket_limit() {
        // macOS accepts up to 104 bytes (including the NUL terminator) for sockaddr_un.sun_path.
        let path = helper_live_input_socket_path();
        assert!(path.starts_with(std::env::temp_dir()));
        let path_len = path.to_string_lossy().len();
        assert!(
            path_len <= 103,
            "socket path must fit in sockaddr_un.sun_path, got {path_len}: {}",
            path.display()
        );
    }
}
