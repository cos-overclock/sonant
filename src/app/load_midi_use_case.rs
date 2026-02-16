use std::path::Path;
use std::sync::{Arc, Mutex};

use thiserror::Error;

use crate::domain::{FileReferenceInput, MidiReferenceSummary, ReferenceSlot, ReferenceSource};
use crate::infra::midi::{MidiLoadError, MidiReferenceData, load_midi_reference};

const DENSITY_NOTES_PER_BAR_AT_MAX_HINT: f32 = 32.0;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadMidiCommand {
    SetFile { slot: ReferenceSlot, path: String },
    ClearSlot { slot: ReferenceSlot },
}

#[derive(Debug, Clone, PartialEq)]
pub enum LoadMidiOutcome {
    Loaded {
        slot: ReferenceSlot,
        replaced: bool,
        reference: MidiReferenceSummary,
    },
    Cleared {
        slot: ReferenceSlot,
        had_reference: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum LoadMidiError {
    #[error("reference MIDI path must not be empty")]
    EmptyPath,
    #[error("failed to load reference MIDI: {source}")]
    LoadFailed { source: MidiLoadError },
    #[error("loaded reference MIDI failed validation: {message}")]
    InvalidReference { message: String },
}

impl LoadMidiError {
    pub fn user_message(&self) -> String {
        match self {
            Self::EmptyPath => "Select a MIDI file before loading.".to_string(),
            Self::LoadFailed { source } => match source {
                MidiLoadError::UnsupportedExtension { .. } => {
                    "Only .mid or .midi files are supported.".to_string()
                }
                MidiLoadError::Io { .. } => {
                    "Could not read the MIDI file. Check the file path and permissions."
                        .to_string()
                }
                MidiLoadError::Parse { .. } => {
                    "The selected file could not be parsed as MIDI. It may be corrupted."
                        .to_string()
                }
                MidiLoadError::UnsupportedTiming => {
                    "SMPTE/timecode MIDI timing is not supported. Re-export the file with metrical timing.".to_string()
                }
                MidiLoadError::InvalidTimeSignature => {
                    "Could not calculate bar length from the MIDI time signature.".to_string()
                }
                MidiLoadError::NoNoteEvents => {
                    "The MIDI file does not contain note-on events.".to_string()
                }
                MidiLoadError::Overflow { .. } => {
                    "The MIDI file is too large to process safely.".to_string()
                }
            },
            Self::InvalidReference { message } => {
                format!("Loaded MIDI reference is invalid: {message}")
            }
        }
    }
}

pub trait MidiReferenceLoader: Send + Sync {
    fn load_reference(&self, path: &Path) -> Result<MidiReferenceData, MidiLoadError>;
}

#[derive(Debug, Default)]
pub struct FileMidiReferenceLoader;

impl MidiReferenceLoader for FileMidiReferenceLoader {
    fn load_reference(&self, path: &Path) -> Result<MidiReferenceData, MidiLoadError> {
        load_midi_reference(path)
    }
}

pub struct LoadMidiUseCase {
    loader: Arc<dyn MidiReferenceLoader>,
    state: Mutex<ReferenceSlotState>,
}

impl LoadMidiUseCase {
    pub fn new() -> Self {
        Self::with_loader(Arc::new(FileMidiReferenceLoader))
    }

    pub fn with_loader(loader: Arc<dyn MidiReferenceLoader>) -> Self {
        Self {
            loader,
            state: Mutex::new(ReferenceSlotState::default()),
        }
    }

    pub fn execute(&self, command: LoadMidiCommand) -> Result<LoadMidiOutcome, LoadMidiError> {
        match command {
            LoadMidiCommand::SetFile { slot, path } => self.set_file(slot, path),
            LoadMidiCommand::ClearSlot { slot } => Ok(self.clear_slot(slot)),
        }
    }

    pub fn snapshot_references(&self) -> Vec<MidiReferenceSummary> {
        let state = self
            .state
            .lock()
            .expect("load MIDI state lock poisoned while reading snapshot");
        state.snapshot()
    }

    pub fn slot_reference(&self, slot: ReferenceSlot) -> Option<MidiReferenceSummary> {
        let state = self
            .state
            .lock()
            .expect("load MIDI state lock poisoned while reading slot reference");
        state.slot_reference(slot)
    }

    fn set_file(
        &self,
        slot: ReferenceSlot,
        path: String,
    ) -> Result<LoadMidiOutcome, LoadMidiError> {
        let normalized_path = normalize_path(path)?;
        let data = self
            .loader
            .load_reference(Path::new(&normalized_path))
            .map_err(|source| LoadMidiError::LoadFailed { source })?;
        let reference = build_reference_summary(slot, normalized_path, data)?;

        let mut state = self
            .state
            .lock()
            .expect("load MIDI state lock poisoned while writing slot reference");
        let replaced = state.upsert(reference.clone());

        Ok(LoadMidiOutcome::Loaded {
            slot,
            replaced,
            reference,
        })
    }

    fn clear_slot(&self, slot: ReferenceSlot) -> LoadMidiOutcome {
        let mut state = self
            .state
            .lock()
            .expect("load MIDI state lock poisoned while clearing slot reference");
        let had_reference = state.clear(slot);
        LoadMidiOutcome::Cleared {
            slot,
            had_reference,
        }
    }
}

impl Default for LoadMidiUseCase {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Default)]
struct ReferenceSlotState {
    references: Vec<MidiReferenceSummary>,
}

impl ReferenceSlotState {
    fn upsert(&mut self, reference: MidiReferenceSummary) -> bool {
        if let Some(existing) = self
            .references
            .iter_mut()
            .find(|existing| existing.slot == reference.slot)
        {
            *existing = reference;
            true
        } else {
            self.references.push(reference);
            false
        }
    }

    fn clear(&mut self, slot: ReferenceSlot) -> bool {
        if let Some(index) = self
            .references
            .iter()
            .position(|reference| reference.slot == slot)
        {
            self.references.remove(index);
            true
        } else {
            false
        }
    }

    fn snapshot(&self) -> Vec<MidiReferenceSummary> {
        self.references.clone()
    }

    fn slot_reference(&self, slot: ReferenceSlot) -> Option<MidiReferenceSummary> {
        self.references
            .iter()
            .find(|reference| reference.slot == slot)
            .cloned()
    }
}

fn normalize_path(path: String) -> Result<String, LoadMidiError> {
    let normalized = path.trim();
    if normalized.is_empty() {
        Err(LoadMidiError::EmptyPath)
    } else {
        Ok(normalized.to_string())
    }
}

fn build_reference_summary(
    slot: ReferenceSlot,
    path: String,
    data: MidiReferenceData,
) -> Result<MidiReferenceSummary, LoadMidiError> {
    let reference = MidiReferenceSummary {
        slot,
        source: ReferenceSource::File,
        file: Some(FileReferenceInput { path }),
        bars: data.summary.bars,
        note_count: data.summary.note_count,
        density_hint: calculate_density_hint(data.summary.note_count, data.summary.bars),
        min_pitch: data.summary.min_pitch,
        max_pitch: data.summary.max_pitch,
        events: data.events,
    };

    reference
        .validate()
        .map_err(|error| LoadMidiError::InvalidReference {
            message: error.to_string(),
        })?;

    Ok(reference)
}

fn calculate_density_hint(note_count: u32, bars: u16) -> f32 {
    if bars == 0 {
        return 1.0;
    }
    let notes_per_bar = note_count as f32 / f32::from(bars);
    (notes_per_bar / DENSITY_NOTES_PER_BAR_AT_MAX_HINT).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::{
        LoadMidiCommand, LoadMidiError, LoadMidiOutcome, LoadMidiUseCase, MidiReferenceLoader,
    };
    use crate::domain::{MidiReferenceEvent, ReferenceSlot};
    use crate::infra::midi::{MidiLoadError, MidiReferenceData, MidiSummary};
    use std::collections::VecDeque;
    use std::path::Path;
    use std::sync::{Arc, Mutex};

    struct StubLoader {
        responses: Mutex<VecDeque<Result<MidiReferenceData, MidiLoadError>>>,
        seen_paths: Mutex<Vec<String>>,
    }

    impl StubLoader {
        fn new(responses: Vec<Result<MidiReferenceData, MidiLoadError>>) -> Self {
            Self {
                responses: Mutex::new(responses.into()),
                seen_paths: Mutex::new(Vec::new()),
            }
        }

        fn seen_paths(&self) -> Vec<String> {
            self.seen_paths
                .lock()
                .expect("stub loader seen path lock poisoned")
                .clone()
        }
    }

    impl MidiReferenceLoader for StubLoader {
        fn load_reference(&self, path: &Path) -> Result<MidiReferenceData, MidiLoadError> {
            self.seen_paths
                .lock()
                .expect("stub loader seen path lock poisoned")
                .push(path.display().to_string());

            self.responses
                .lock()
                .expect("stub loader responses lock poisoned")
                .pop_front()
                .expect("stub loader must have a prepared response for each load call")
        }
    }

    #[test]
    fn load_replace_clear_flow_is_supported_for_a_slot() {
        let loader = Arc::new(StubLoader::new(vec![
            Ok(sample_reference_data(4, 12, 60, 72, "first")),
            Ok(sample_reference_data(8, 24, 55, 79, "second")),
        ]));
        let use_case = LoadMidiUseCase::with_loader(loader.clone());

        let loaded = use_case
            .execute(LoadMidiCommand::SetFile {
                slot: ReferenceSlot::Melody,
                path: "  /tmp/first.mid  ".to_string(),
            })
            .expect("first load should succeed");

        assert!(matches!(
            loaded,
            LoadMidiOutcome::Loaded {
                slot: ReferenceSlot::Melody,
                replaced: false,
                ..
            }
        ));

        let replaced = use_case
            .execute(LoadMidiCommand::SetFile {
                slot: ReferenceSlot::Melody,
                path: "/tmp/second.mid".to_string(),
            })
            .expect("second load should succeed");

        assert!(matches!(
            replaced,
            LoadMidiOutcome::Loaded {
                slot: ReferenceSlot::Melody,
                replaced: true,
                ..
            }
        ));

        let current = use_case
            .slot_reference(ReferenceSlot::Melody)
            .expect("slot should contain a reference after replacement");
        assert_eq!(
            current.file.expect("file metadata must exist").path,
            "/tmp/second.mid"
        );
        assert_eq!(current.bars, 8);
        assert_eq!(current.note_count, 24);

        let cleared = use_case
            .execute(LoadMidiCommand::ClearSlot {
                slot: ReferenceSlot::Melody,
            })
            .expect("clear command should succeed");
        assert_eq!(
            cleared,
            LoadMidiOutcome::Cleared {
                slot: ReferenceSlot::Melody,
                had_reference: true,
            }
        );
        assert!(use_case.slot_reference(ReferenceSlot::Melody).is_none());
        assert!(use_case.snapshot_references().is_empty());

        assert_eq!(
            loader.seen_paths(),
            vec!["/tmp/first.mid".to_string(), "/tmp/second.mid".to_string()]
        );
    }

    #[test]
    fn load_error_is_propagated_and_existing_slot_is_kept() {
        let loader = Arc::new(StubLoader::new(vec![
            Ok(sample_reference_data(4, 8, 60, 67, "ok")),
            Err(MidiLoadError::Parse {
                message: "invalid smf".to_string(),
            }),
        ]));
        let use_case = LoadMidiUseCase::with_loader(loader);

        use_case
            .execute(LoadMidiCommand::SetFile {
                slot: ReferenceSlot::Melody,
                path: "/tmp/current.mid".to_string(),
            })
            .expect("initial load should succeed");

        let error = use_case
            .execute(LoadMidiCommand::SetFile {
                slot: ReferenceSlot::Melody,
                path: "/tmp/broken.mid".to_string(),
            })
            .expect_err("broken MIDI should surface a load error");

        assert!(matches!(
            error,
            LoadMidiError::LoadFailed {
                source: MidiLoadError::Parse { .. }
            }
        ));

        let current = use_case
            .slot_reference(ReferenceSlot::Melody)
            .expect("existing slot reference should be preserved on load failure");
        assert_eq!(
            current.file.expect("file metadata must exist").path,
            "/tmp/current.mid"
        );
    }

    #[test]
    fn empty_path_is_rejected_without_invoking_loader() {
        let loader = Arc::new(StubLoader::new(Vec::new()));
        let use_case = LoadMidiUseCase::with_loader(loader.clone());

        let error = use_case
            .execute(LoadMidiCommand::SetFile {
                slot: ReferenceSlot::Melody,
                path: "   ".to_string(),
            })
            .expect_err("empty path should be rejected");

        assert_eq!(error, LoadMidiError::EmptyPath);
        assert!(loader.seen_paths().is_empty());
    }

    #[test]
    fn user_message_for_extension_error_is_actionable() {
        let error = LoadMidiError::LoadFailed {
            source: MidiLoadError::UnsupportedExtension {
                path: "/tmp/not-midi.wav".to_string(),
            },
        };

        assert!(error.user_message().contains(".mid"));
    }

    fn sample_reference_data(
        bars: u16,
        note_count: u32,
        min_pitch: u8,
        max_pitch: u8,
        event_label: &str,
    ) -> MidiReferenceData {
        MidiReferenceData {
            summary: MidiSummary {
                bars,
                note_count,
                min_pitch,
                max_pitch,
            },
            events: vec![MidiReferenceEvent {
                track: 0,
                absolute_tick: 0,
                delta_tick: 0,
                event: format!("Event({event_label})"),
            }],
        }
    }
}
