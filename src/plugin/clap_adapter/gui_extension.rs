use clack_extensions::gui::{GuiApiType, GuiConfiguration, GuiSize, PluginGuiImpl, Window};
use clack_plugin::prelude::PluginError;
use std::ffi::CStr;
use std::mem::MaybeUninit;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use super::SonantPluginMainThread;

#[derive(Default)]
pub(super) struct SonantGuiController {
    state: HelperState,
}

#[derive(Default)]
struct HelperState {
    child: Option<Child>,
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
