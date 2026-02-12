use clack_plugin::prelude::*;

pub struct SonantPlugin;

impl Plugin for SonantPlugin {
    type AudioProcessor<'a> = SonantAudioProcessor;

    type Shared<'a> = ();
    type MainThread<'a> = ();
}

impl DefaultPluginFactory for SonantPlugin {
    fn get_descriptor() -> PluginDescriptor {
        PluginDescriptor::new("com.sonant.midi_generator", "Sonant")
    }

    fn new_shared(_host: HostSharedHandle<'_>) -> Result<Self::Shared<'_>, PluginError> {
        Ok(())
    }

    fn new_main_thread<'a>(
        _host: HostMainThreadHandle<'a>,
        _shared: &'a Self::Shared<'a>,
    ) -> Result<Self::MainThread<'a>, PluginError> {
        Ok(())
    }
}

pub struct SonantAudioProcessor;

impl<'a> PluginAudioProcessor<'a, (), ()> for SonantAudioProcessor {
    fn activate(
        _host: HostAudioProcessorHandle<'a>,
        _main_thread: &mut (),
        _shared: &'a (),
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

clack_export_entry!(SinglePluginEntry<SonantPlugin>);
