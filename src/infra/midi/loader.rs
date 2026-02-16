use std::fs;
use std::path::Path;

use crate::domain::MidiReferenceEvent;
use midly::{MetaMessage, MidiMessage, Smf, Timing, TrackEventKind};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MidiSummary {
    pub bars: u16,
    pub note_count: u32,
    pub min_pitch: u8,
    pub max_pitch: u8,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MidiReferenceData {
    pub summary: MidiSummary,
    pub events: Vec<MidiReferenceEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum MidiLoadError {
    #[error("unsupported file extension for MIDI file: {path}")]
    UnsupportedExtension { path: String },
    #[error("failed to read MIDI file: {message}")]
    Io { message: String },
    #[error("failed to parse MIDI file: {message}")]
    Parse { message: String },
    #[error("unsupported MIDI timing format: SMPTE/timecode is not supported")]
    UnsupportedTiming,
    #[error("failed to calculate bar length from time signature")]
    InvalidTimeSignature,
    #[error("MIDI file does not contain note-on events")]
    NoNoteEvents,
    #[error("summary value overflowed for field: {field}")]
    Overflow { field: &'static str },
}

pub fn load_midi_summary(path: impl AsRef<Path>) -> Result<MidiSummary, MidiLoadError> {
    load_midi_reference(path).map(|reference| reference.summary)
}

pub fn load_midi_reference(path: impl AsRef<Path>) -> Result<MidiReferenceData, MidiLoadError> {
    let path = path.as_ref();
    validate_midi_extension(path)?;
    let bytes = fs::read(path).map_err(|error| MidiLoadError::Io {
        message: error.to_string(),
    })?;
    parse_midi_reference(&bytes)
}

pub fn parse_midi_summary(bytes: &[u8]) -> Result<MidiSummary, MidiLoadError> {
    parse_midi_reference(bytes).map(|reference| reference.summary)
}

pub fn parse_midi_reference(bytes: &[u8]) -> Result<MidiReferenceData, MidiLoadError> {
    let smf = Smf::parse(bytes).map_err(|error| MidiLoadError::Parse {
        message: error.to_string(),
    })?;
    let ticks_per_quarter = match smf.header.timing {
        Timing::Metrical(value) => value.as_int(),
        Timing::Timecode(_, _) => return Err(MidiLoadError::UnsupportedTiming),
    };

    let mut signature = TimeSignature::default();
    let mut note_count: u64 = 0;
    let mut min_pitch = u8::MAX;
    let mut max_pitch = u8::MIN;
    let mut max_tick: u64 = 0;
    let mut events = Vec::new();

    for (track_index, track_events) in smf.tracks.iter().enumerate() {
        let track_id = u16::try_from(track_index).map_err(|_| MidiLoadError::Overflow {
            field: "track_index",
        })?;
        let mut absolute_tick: u64 = 0;
        for event in track_events {
            absolute_tick += u64::from(event.delta.as_int());
            if absolute_tick > max_tick {
                max_tick = absolute_tick;
            }
            let absolute_tick_u32 =
                u32::try_from(absolute_tick).map_err(|_| MidiLoadError::Overflow {
                    field: "absolute_tick",
                })?;
            events.push(MidiReferenceEvent {
                track: track_id,
                absolute_tick: absolute_tick_u32,
                delta_tick: event.delta.as_int(),
                event: format!("{:?}", event.kind),
            });

            match &event.kind {
                TrackEventKind::Midi { message, .. } => {
                    if let MidiMessage::NoteOn { key, vel } = message
                        && vel.as_int() > 0
                    {
                        note_count += 1;
                        let pitch = key.as_int();
                        min_pitch = min_pitch.min(pitch);
                        max_pitch = max_pitch.max(pitch);
                    }
                }
                TrackEventKind::Meta(MetaMessage::TimeSignature(
                    numerator,
                    denominator_exponent,
                    _,
                    _,
                )) => {
                    signature = TimeSignature {
                        numerator: *numerator,
                        denominator_exponent: *denominator_exponent,
                    };
                }
                _ => {}
            }
        }
    }

    if note_count == 0 {
        return Err(MidiLoadError::NoNoteEvents);
    }

    let ticks_per_bar = calculate_ticks_per_bar(ticks_per_quarter, signature)?;
    let bars_u64 = if max_tick == 0 {
        1
    } else {
        max_tick.div_ceil(ticks_per_bar).max(1)
    };
    let bars = u16::try_from(bars_u64).map_err(|_| MidiLoadError::Overflow { field: "bars" })?;
    let note_count = u32::try_from(note_count).map_err(|_| MidiLoadError::Overflow {
        field: "note_count",
    })?;

    Ok(MidiReferenceData {
        summary: MidiSummary {
            bars,
            note_count,
            min_pitch,
            max_pitch,
        },
        events,
    })
}

fn validate_midi_extension(path: &Path) -> Result<(), MidiLoadError> {
    let is_supported = path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("mid") || ext.eq_ignore_ascii_case("midi"));
    if is_supported {
        Ok(())
    } else {
        Err(MidiLoadError::UnsupportedExtension {
            path: path.display().to_string(),
        })
    }
}

fn calculate_ticks_per_bar(
    ticks_per_quarter: u16,
    signature: TimeSignature,
) -> Result<u64, MidiLoadError> {
    let denominator = 1_u64
        .checked_shl(u32::from(signature.denominator_exponent))
        .ok_or(MidiLoadError::InvalidTimeSignature)?;
    if denominator == 0 || signature.numerator == 0 {
        return Err(MidiLoadError::InvalidTimeSignature);
    }

    let numerator = u64::from(ticks_per_quarter)
        .checked_mul(4)
        .and_then(|value| value.checked_mul(u64::from(signature.numerator)))
        .ok_or(MidiLoadError::Overflow {
            field: "ticks_per_bar",
        })?;
    let ticks_per_bar = numerator / denominator;
    if ticks_per_bar == 0 {
        return Err(MidiLoadError::InvalidTimeSignature);
    }
    Ok(ticks_per_bar)
}

#[derive(Debug, Clone, Copy)]
struct TimeSignature {
    numerator: u8,
    denominator_exponent: u8,
}

impl Default for TimeSignature {
    fn default() -> Self {
        Self {
            numerator: 4,
            denominator_exponent: 2,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use midly::num::{u4, u7, u15, u28};
    use midly::{
        Format, Fps, Header, MetaMessage, MidiMessage, Smf, Timing, TrackEvent, TrackEventKind,
    };

    use super::{MidiLoadError, load_midi_reference, load_midi_summary};

    #[test]
    fn load_midi_summary_extracts_basic_metrics() {
        let smf = Smf {
            header: Header::new(Format::SingleTrack, Timing::Metrical(u15::new(96))),
            tracks: vec![vec![
                TrackEvent {
                    delta: u28::new(0),
                    kind: TrackEventKind::Meta(MetaMessage::TimeSignature(4, 2, 24, 8)),
                },
                TrackEvent {
                    delta: u28::new(0),
                    kind: TrackEventKind::Midi {
                        channel: u4::new(0),
                        message: MidiMessage::NoteOn {
                            key: u7::new(60),
                            vel: u7::new(100),
                        },
                    },
                },
                TrackEvent {
                    delta: u28::new(96),
                    kind: TrackEventKind::Midi {
                        channel: u4::new(0),
                        message: MidiMessage::NoteOff {
                            key: u7::new(60),
                            vel: u7::new(0),
                        },
                    },
                },
                TrackEvent {
                    delta: u28::new(0),
                    kind: TrackEventKind::Midi {
                        channel: u4::new(0),
                        message: MidiMessage::NoteOn {
                            key: u7::new(64),
                            vel: u7::new(110),
                        },
                    },
                },
                TrackEvent {
                    delta: u28::new(96),
                    kind: TrackEventKind::Midi {
                        channel: u4::new(0),
                        message: MidiMessage::NoteOff {
                            key: u7::new(64),
                            vel: u7::new(0),
                        },
                    },
                },
                TrackEvent {
                    delta: u28::new(192),
                    kind: TrackEventKind::Midi {
                        channel: u4::new(0),
                        message: MidiMessage::NoteOn {
                            key: u7::new(67),
                            vel: u7::new(100),
                        },
                    },
                },
                TrackEvent {
                    delta: u28::new(96),
                    kind: TrackEventKind::Midi {
                        channel: u4::new(0),
                        message: MidiMessage::NoteOff {
                            key: u7::new(67),
                            vel: u7::new(0),
                        },
                    },
                },
                TrackEvent {
                    delta: u28::new(0),
                    kind: TrackEventKind::Meta(MetaMessage::EndOfTrack),
                },
            ]],
        };

        let midi_file = write_midi_file("mid", smf);
        let summary = load_midi_summary(midi_file.path()).expect("valid midi should load");

        assert_eq!(summary.bars, 2);
        assert_eq!(summary.note_count, 3);
        assert_eq!(summary.min_pitch, 60);
        assert_eq!(summary.max_pitch, 67);
    }

    #[test]
    fn load_midi_summary_defaults_to_four_four_when_time_signature_missing() {
        let smf = Smf {
            header: Header::new(Format::SingleTrack, Timing::Metrical(u15::new(96))),
            tracks: vec![vec![
                TrackEvent {
                    delta: u28::new(0),
                    kind: TrackEventKind::Midi {
                        channel: u4::new(0),
                        message: MidiMessage::NoteOn {
                            key: u7::new(60),
                            vel: u7::new(100),
                        },
                    },
                },
                TrackEvent {
                    delta: u28::new(192),
                    kind: TrackEventKind::Midi {
                        channel: u4::new(0),
                        message: MidiMessage::NoteOff {
                            key: u7::new(60),
                            vel: u7::new(0),
                        },
                    },
                },
                TrackEvent {
                    delta: u28::new(193),
                    kind: TrackEventKind::Midi {
                        channel: u4::new(0),
                        message: MidiMessage::NoteOn {
                            key: u7::new(64),
                            vel: u7::new(96),
                        },
                    },
                },
                TrackEvent {
                    delta: u28::new(0),
                    kind: TrackEventKind::Meta(MetaMessage::EndOfTrack),
                },
            ]],
        };

        let midi_file = write_midi_file("mid", smf);
        let summary = load_midi_summary(midi_file.path()).expect("valid midi should load");

        // Without an explicit time-signature event, loader should use default 4/4.
        // At 96 TPQ, one bar is 384 ticks, and max tick 385 should map to 2 bars.
        assert_eq!(summary.bars, 2);
        assert_eq!(summary.note_count, 2);
        assert_eq!(summary.min_pitch, 60);
        assert_eq!(summary.max_pitch, 64);
    }

    #[test]
    fn load_midi_reference_extracts_all_track_events() {
        let smf = Smf {
            header: Header::new(Format::SingleTrack, Timing::Metrical(u15::new(96))),
            tracks: vec![vec![
                TrackEvent {
                    delta: u28::new(0),
                    kind: TrackEventKind::Meta(MetaMessage::TimeSignature(4, 2, 24, 8)),
                },
                TrackEvent {
                    delta: u28::new(0),
                    kind: TrackEventKind::Midi {
                        channel: u4::new(0),
                        message: MidiMessage::NoteOn {
                            key: u7::new(60),
                            vel: u7::new(100),
                        },
                    },
                },
                TrackEvent {
                    delta: u28::new(96),
                    kind: TrackEventKind::Midi {
                        channel: u4::new(0),
                        message: MidiMessage::NoteOff {
                            key: u7::new(60),
                            vel: u7::new(0),
                        },
                    },
                },
                TrackEvent {
                    delta: u28::new(0),
                    kind: TrackEventKind::Meta(MetaMessage::EndOfTrack),
                },
            ]],
        };

        let midi_file = write_midi_file("mid", smf);
        let reference = load_midi_reference(midi_file.path()).expect("valid midi should load");

        assert_eq!(reference.summary.note_count, 1);
        assert_eq!(reference.events.len(), 4);
        assert_eq!(reference.events[0].absolute_tick, 0);
        assert!(reference.events[0].event.contains("TimeSignature"));
        assert!(reference.events[1].event.contains("NoteOn"));
        assert!(reference.events[3].event.contains("EndOfTrack"));
    }

    #[test]
    fn load_midi_summary_rejects_unsupported_extension() {
        let midi_file = write_bytes_file("txt", b"dummy");
        let err = load_midi_summary(midi_file.path()).expect_err("non-midi extension must fail");

        assert!(matches!(err, MidiLoadError::UnsupportedExtension { .. }));
    }

    #[test]
    fn load_midi_summary_fails_on_corrupted_file() {
        let midi_file = write_bytes_file("mid", &[0x00, 0x01, 0x02, 0x03]);
        let err = load_midi_summary(midi_file.path()).expect_err("corrupted midi must fail");

        assert!(matches!(err, MidiLoadError::Parse { .. }));
    }

    #[test]
    fn load_midi_summary_fails_on_unsupported_timing() {
        let smf = Smf {
            header: Header::new(Format::SingleTrack, Timing::Timecode(Fps::Fps24, 40)),
            tracks: vec![vec![
                TrackEvent {
                    delta: u28::new(0),
                    kind: TrackEventKind::Midi {
                        channel: u4::new(0),
                        message: MidiMessage::NoteOn {
                            key: u7::new(60),
                            vel: u7::new(100),
                        },
                    },
                },
                TrackEvent {
                    delta: u28::new(0),
                    kind: TrackEventKind::Meta(MetaMessage::EndOfTrack),
                },
            ]],
        };

        let midi_file = write_midi_file("mid", smf);
        let err = load_midi_summary(midi_file.path()).expect_err("SMPTE/timecode timing must fail");

        assert_eq!(err, MidiLoadError::UnsupportedTiming);
    }

    #[test]
    fn load_midi_summary_fails_when_no_note_on_events_exist() {
        let smf = Smf {
            header: Header::new(Format::SingleTrack, Timing::Metrical(u15::new(96))),
            tracks: vec![vec![TrackEvent {
                delta: u28::new(0),
                kind: TrackEventKind::Meta(MetaMessage::EndOfTrack),
            }]],
        };

        let midi_file = write_midi_file("mid", smf);
        let err = load_midi_summary(midi_file.path()).expect_err("note-less midi must fail");

        assert_eq!(err, MidiLoadError::NoNoteEvents);
    }

    fn write_midi_file(extension: &str, smf: Smf<'static>) -> TestFile {
        let mut bytes = Vec::new();
        smf.write_std(&mut bytes)
            .expect("test midi serialization must succeed");
        write_bytes_file(extension, &bytes)
    }

    fn write_bytes_file(extension: &str, bytes: &[u8]) -> TestFile {
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be monotonic")
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("sonant-midi-loader-{nanos}-{id}.{extension}"));

        fs::write(&path, bytes).expect("test file must be writable");
        TestFile { path }
    }

    struct TestFile {
        path: PathBuf,
    }

    impl TestFile {
        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TestFile {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.path);
        }
    }
}
