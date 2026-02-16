use clack_extensions::audio_ports::{AudioPortInfoWriter, PluginAudioPortsImpl};

use super::SonantPluginMainThread;

impl PluginAudioPortsImpl for SonantPluginMainThread<'_> {
    fn count(&mut self, is_input: bool) -> u32 {
        audio_port_count(is_input)
    }

    fn get(&mut self, _index: u32, _is_input: bool, _writer: &mut AudioPortInfoWriter) {}
}

const fn audio_port_count(_is_input: bool) -> u32 {
    0
}

#[cfg(test)]
mod tests {
    use super::audio_port_count;

    #[test]
    fn audio_port_count_reports_no_ports() {
        assert_eq!(audio_port_count(true), 0);
        assert_eq!(audio_port_count(false), 0);
    }
}
