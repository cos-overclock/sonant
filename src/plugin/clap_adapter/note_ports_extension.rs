use clack_extensions::note_ports::{
    NoteDialect, NoteDialects, NotePortInfo, NotePortInfoWriter, PluginNotePortsImpl,
};
use clack_plugin::prelude::ClapId;

use super::SonantPluginMainThread;

const NOTE_PORT_INDEX_MAIN: u32 = 0;
const NOTE_PORT_ID_IN: u32 = 0;
const NOTE_PORT_ID_OUT: u32 = 1;
const NOTE_PORT_NAME_IN: &[u8] = b"midi_in";
const NOTE_PORT_NAME_OUT: &[u8] = b"midi_out";

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

#[cfg(test)]
mod tests {
    use super::{
        NOTE_PORT_ID_IN, NOTE_PORT_ID_OUT, NOTE_PORT_INDEX_MAIN, NOTE_PORT_NAME_IN,
        NOTE_PORT_NAME_OUT, note_port_count, note_port_definition,
    };
    use clack_extensions::note_ports::{NoteDialect, NoteDialects};
    use clack_plugin::prelude::ClapId;

    #[test]
    fn note_port_definition_exposes_midi_in_and_out() {
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
    fn note_port_definition_rejects_unknown_index() {
        assert!(note_port_definition(99, true).is_none());
    }

    #[test]
    fn note_port_supports_midi_dialect_only() {
        let input = note_port_definition(NOTE_PORT_INDEX_MAIN, true)
            .expect("input note port must be defined");
        assert_eq!(input.supported_dialects, NoteDialects::MIDI);
    }
}
