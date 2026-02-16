use clack_extensions::state::PluginStateImpl;
use clack_plugin::prelude::PluginError;
use clack_plugin::stream::{InputStream, OutputStream};
use std::io::{Read, Write};

use super::SonantPluginMainThread;

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
