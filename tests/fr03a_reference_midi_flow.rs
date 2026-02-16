use midly::num::{u4, u7, u15, u28};
use midly::{Format, Header, MetaMessage, MidiMessage, Smf, Timing, TrackEvent, TrackEventKind};
use sonant::app::{LoadMidiCommand, LoadMidiOutcome, LoadMidiUseCase};
use sonant::domain::{
    GenerationMode, GenerationParams, GenerationRequest, LlmError, MidiReferenceSummary, ModelRef,
    ReferenceSlot,
};

#[path = "support/temp_file_fixture.rs"]
mod temp_file_fixture;

use temp_file_fixture::write_midi_file;

#[test]
fn generation_request_references_are_built_from_loaded_midi_file() {
    let midi_file = write_midi_file(
        "sonant-fr03a-reference-flow",
        "mid",
        &smf_with_notes(&[60, 64]),
    );
    let use_case = LoadMidiUseCase::new();

    let load_outcome = use_case
        .execute(LoadMidiCommand::SetFile {
            slot: ReferenceSlot::Melody,
            path: midi_file.path().display().to_string(),
        })
        .expect("MIDI load should succeed");

    assert!(matches!(
        load_outcome,
        LoadMidiOutcome::Loaded {
            slot: ReferenceSlot::Melody,
            replaced: false,
            ..
        }
    ));

    let request = valid_request(GenerationMode::Continuation, use_case.snapshot_references());

    request
        .validate()
        .expect("continuation request with loaded reference should be valid");
    assert_eq!(request.references.len(), 1);

    let reference = &request.references[0];
    assert_eq!(reference.note_count, 2);
    assert_eq!(reference.min_pitch, 60);
    assert_eq!(reference.max_pitch, 64);
    assert!(!reference.events.is_empty());
    assert_eq!(
        reference
            .file
            .as_ref()
            .expect("file metadata should be present")
            .path,
        midi_file.path().to_string_lossy().to_string()
    );
}

#[test]
fn continuation_request_tracks_reference_replace_and_clear_transitions() {
    let first_midi = write_midi_file("sonant-fr03a-reference-flow", "mid", &smf_with_notes(&[60]));
    let second_midi = write_midi_file(
        "sonant-fr03a-reference-flow",
        "mid",
        &smf_with_notes(&[67, 72]),
    );
    let use_case = LoadMidiUseCase::new();

    let first_outcome = use_case
        .execute(LoadMidiCommand::SetFile {
            slot: ReferenceSlot::Melody,
            path: first_midi.path().display().to_string(),
        })
        .expect("initial MIDI load should succeed");
    assert!(matches!(
        first_outcome,
        LoadMidiOutcome::Loaded {
            slot: ReferenceSlot::Melody,
            replaced: false,
            ..
        }
    ));

    let replaced_outcome = use_case
        .execute(LoadMidiCommand::SetFile {
            slot: ReferenceSlot::Melody,
            path: second_midi.path().display().to_string(),
        })
        .expect("replacement MIDI load should succeed");
    assert!(matches!(
        replaced_outcome,
        LoadMidiOutcome::Loaded {
            slot: ReferenceSlot::Melody,
            replaced: true,
            ..
        }
    ));

    let request_after_replace =
        valid_request(GenerationMode::Continuation, use_case.snapshot_references());
    request_after_replace
        .validate()
        .expect("continuation request should stay valid while slot is populated");
    assert_eq!(request_after_replace.references.len(), 1);
    assert_eq!(request_after_replace.references[0].note_count, 2);
    assert_eq!(
        request_after_replace.references[0]
            .file
            .as_ref()
            .expect("file metadata should be present")
            .path,
        second_midi.path().to_string_lossy().to_string()
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
            had_reference: true,
        }
    );

    let request_after_clear =
        valid_request(GenerationMode::Continuation, use_case.snapshot_references());
    assert!(matches!(
        request_after_clear.validate(),
        Err(LlmError::Validation { message })
            if message == "continuation mode requires at least one MIDI reference"
    ));
}

fn valid_request(mode: GenerationMode, references: Vec<MidiReferenceSummary>) -> GenerationRequest {
    GenerationRequest {
        request_id: "fr03a-it-req".to_string(),
        model: ModelRef {
            provider: "anthropic".to_string(),
            model: "claude-3-5-sonnet".to_string(),
        },
        mode,
        prompt: "continue this idea".to_string(),
        params: GenerationParams {
            bpm: 120,
            key: "C".to_string(),
            scale: "major".to_string(),
            density: 3,
            complexity: 3,
            temperature: Some(0.7),
            top_p: Some(0.9),
            max_tokens: Some(512),
        },
        references,
        variation_count: 1,
    }
}

fn smf_with_notes(note_pitches: &[u8]) -> Smf<'static> {
    assert!(
        !note_pitches.is_empty(),
        "integration test MIDI fixture must contain at least one note"
    );

    let mut track = vec![TrackEvent {
        delta: u28::new(0),
        kind: TrackEventKind::Meta(MetaMessage::TimeSignature(4, 2, 24, 8)),
    }];

    for (index, pitch) in note_pitches.iter().copied().enumerate() {
        track.push(TrackEvent {
            delta: u28::new(if index == 0 { 0 } else { 96 }),
            kind: TrackEventKind::Midi {
                channel: u4::new(0),
                message: MidiMessage::NoteOn {
                    key: u7::new(pitch),
                    vel: u7::new(100),
                },
            },
        });
        track.push(TrackEvent {
            delta: u28::new(96),
            kind: TrackEventKind::Midi {
                channel: u4::new(0),
                message: MidiMessage::NoteOff {
                    key: u7::new(pitch),
                    vel: u7::new(0),
                },
            },
        });
    }

    track.push(TrackEvent {
        delta: u28::new(0),
        kind: TrackEventKind::Meta(MetaMessage::EndOfTrack),
    });

    Smf {
        header: Header::new(Format::SingleTrack, Timing::Metrical(u15::new(96))),
        tracks: vec![track],
    }
}
