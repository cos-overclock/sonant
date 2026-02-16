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
        slot_reference_count: usize,
        reference: MidiReferenceSummary,
    },
    Cleared {
        slot: ReferenceSlot,
        cleared_count: usize,
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

    pub fn slot_references(&self, slot: ReferenceSlot) -> Vec<MidiReferenceSummary> {
        let state = self
            .state
            .lock()
            .expect("load MIDI state lock poisoned while reading slot references");
        state.slot_references(slot)
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
        let slot_reference_count = state.append(reference.clone());

        Ok(LoadMidiOutcome::Loaded {
            slot,
            slot_reference_count,
            reference,
        })
    }

    fn clear_slot(&self, slot: ReferenceSlot) -> LoadMidiOutcome {
        let mut state = self
            .state
            .lock()
            .expect("load MIDI state lock poisoned while clearing slot reference");
        let cleared_count = state.clear(slot);
        LoadMidiOutcome::Cleared {
            slot,
            cleared_count,
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
    fn append(&mut self, reference: MidiReferenceSummary) -> usize {
        let slot = reference.slot;
        self.references.push(reference);
        self.slot_reference_count(slot)
    }

    fn clear(&mut self, slot: ReferenceSlot) -> usize {
        let before_len = self.references.len();
        self.references.retain(|reference| reference.slot != slot);
        before_len.saturating_sub(self.references.len())
    }

    fn snapshot(&self) -> Vec<MidiReferenceSummary> {
        self.references.clone()
    }

    fn slot_reference(&self, slot: ReferenceSlot) -> Option<MidiReferenceSummary> {
        self.references
            .iter()
            .rev()
            .find(|reference| reference.slot == slot)
            .cloned()
    }

    fn slot_references(&self, slot: ReferenceSlot) -> Vec<MidiReferenceSummary> {
        self.references
            .iter()
            .filter(|reference| reference.slot == slot)
            .cloned()
            .collect()
    }

    fn slot_reference_count(&self, slot: ReferenceSlot) -> usize {
        self.references
            .iter()
            .filter(|reference| reference.slot == slot)
            .count()
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
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};

    struct StubLoader {
        responses: Mutex<VecDeque<Result<MidiReferenceData, MidiLoadError>>>,
        seen_paths: Mutex<Vec<PathBuf>>,
    }

    impl StubLoader {
        fn new(responses: Vec<Result<MidiReferenceData, MidiLoadError>>) -> Self {
            Self {
                responses: Mutex::new(responses.into()),
                seen_paths: Mutex::new(Vec::new()),
            }
        }

        fn seen_paths(&self) -> Vec<PathBuf> {
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
                .push(path.to_path_buf());

            self.responses
                .lock()
                .expect("stub loader responses lock poisoned")
                .pop_front()
                .expect("stub loader must have a prepared response for each load call")
        }
    }

    #[test]
    fn load_append_clear_flow_is_supported_for_a_slot() {
        let first_path = temp_test_path("first.mid");
        let second_path = temp_test_path("second.mid");

        let loader = Arc::new(StubLoader::new(vec![
            Ok(sample_reference_data(4, 12, 60, 72, "first")),
            Ok(sample_reference_data(8, 24, 55, 79, "second")),
        ]));
        let use_case = LoadMidiUseCase::with_loader(loader.clone());

        let loaded = use_case
            .execute(LoadMidiCommand::SetFile {
                slot: ReferenceSlot::Melody,
                path: format!("  {}  ", first_path.display()),
            })
            .expect("first load should succeed");

        assert!(matches!(
            loaded,
            LoadMidiOutcome::Loaded {
                slot: ReferenceSlot::Melody,
                slot_reference_count: 1,
                ..
            }
        ));

        let appended = use_case
            .execute(LoadMidiCommand::SetFile {
                slot: ReferenceSlot::Melody,
                path: second_path.to_string_lossy().to_string(),
            })
            .expect("second load should succeed");

        assert!(matches!(
            appended,
            LoadMidiOutcome::Loaded {
                slot: ReferenceSlot::Melody,
                slot_reference_count: 2,
                ..
            }
        ));

        let slot_references = use_case.slot_references(ReferenceSlot::Melody);
        assert_eq!(slot_references.len(), 2);
        let current = use_case
            .slot_reference(ReferenceSlot::Melody)
            .expect("slot should contain a latest reference after appending");
        assert_eq!(
            current.file.expect("file metadata must exist").path,
            second_path.to_string_lossy()
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
                cleared_count: 2,
            }
        );
        assert!(use_case.slot_reference(ReferenceSlot::Melody).is_none());
        assert!(use_case.slot_references(ReferenceSlot::Melody).is_empty());
        assert!(use_case.snapshot_references().is_empty());

        assert_eq!(loader.seen_paths(), vec![first_path, second_path]);
    }

    #[test]
    fn multiple_slots_can_be_loaded_and_cleared_independently() {
        let melody_path = temp_test_path("melody.mid");
        let chord_path = temp_test_path("chords.mid");

        let loader = Arc::new(StubLoader::new(vec![
            Ok(sample_reference_data(4, 16, 60, 72, "melody")),
            Ok(sample_reference_data(4, 12, 48, 67, "chords")),
        ]));
        let use_case = LoadMidiUseCase::with_loader(loader.clone());

        use_case
            .execute(LoadMidiCommand::SetFile {
                slot: ReferenceSlot::Melody,
                path: melody_path.to_string_lossy().to_string(),
            })
            .expect("melody slot load should succeed");
        use_case
            .execute(LoadMidiCommand::SetFile {
                slot: ReferenceSlot::ChordProgression,
                path: chord_path.to_string_lossy().to_string(),
            })
            .expect("chord slot load should succeed");

        let before_clear = use_case.snapshot_references();
        assert_eq!(before_clear.len(), 2);
        assert!(
            before_clear
                .iter()
                .any(|reference| reference.slot == ReferenceSlot::Melody)
        );
        assert!(
            before_clear
                .iter()
                .any(|reference| reference.slot == ReferenceSlot::ChordProgression)
        );

        let clear_outcome = use_case
            .execute(LoadMidiCommand::ClearSlot {
                slot: ReferenceSlot::Melody,
            })
            .expect("clear should succeed");
        assert_eq!(
            clear_outcome,
            LoadMidiOutcome::Cleared {
                slot: ReferenceSlot::Melody,
                cleared_count: 1,
            }
        );

        assert!(use_case.slot_reference(ReferenceSlot::Melody).is_none());
        let chord_reference = use_case
            .slot_reference(ReferenceSlot::ChordProgression)
            .expect("chord slot must remain after clearing melody slot");
        assert_eq!(
            chord_reference
                .file
                .expect("file metadata must exist for chord slot")
                .path,
            chord_path.to_string_lossy()
        );
        assert_eq!(use_case.snapshot_references().len(), 1);

        assert_eq!(loader.seen_paths(), vec![melody_path, chord_path]);
    }

    #[test]
    fn load_error_is_propagated_and_existing_slot_is_kept() {
        let current_path = temp_test_path("current.mid");
        let broken_path = temp_test_path("broken.mid");

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
                path: current_path.to_string_lossy().to_string(),
            })
            .expect("initial load should succeed");

        let error = use_case
            .execute(LoadMidiCommand::SetFile {
                slot: ReferenceSlot::Melody,
                path: broken_path.to_string_lossy().to_string(),
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
            current_path.to_string_lossy()
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

    #[test]
    fn user_message_for_corrupted_midi_is_actionable() {
        let error = LoadMidiError::LoadFailed {
            source: MidiLoadError::Parse {
                message: "invalid smf".to_string(),
            },
        };

        assert!(
            error.user_message().contains("could not be parsed as MIDI"),
            "expected parse error message to explain corruption"
        );
    }

    #[test]
    fn user_message_for_io_failure_suggests_recovery() {
        let error = LoadMidiError::LoadFailed {
            source: MidiLoadError::Io {
                message: "permission denied".to_string(),
            },
        };

        assert!(
            error
                .user_message()
                .contains("Check the file path and permissions"),
            "expected IO error message to suggest path/permission checks"
        );
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

    fn temp_test_path(file_name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("sonant-load-midi-use-case-{file_name}"))
    }
}
