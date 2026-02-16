use std::collections::HashMap;

use thiserror::Error;

use crate::domain::{ReferenceSlot, ReferenceSource};

pub const MIDI_CHANNEL_MIN: u8 = 1;
pub const MIDI_CHANNEL_MAX: u8 = 16;

const REFERENCE_SLOT_COUNT: usize = 7;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChannelMapping {
    pub slot: ReferenceSlot,
    pub channel: u8,
}

impl ChannelMapping {
    pub fn validate(self) -> Result<(), InputTrackModelError> {
        if !(MIDI_CHANNEL_MIN..=MIDI_CHANNEL_MAX).contains(&self.channel) {
            return Err(InputTrackModelError::ChannelOutOfRange {
                slot: self.slot,
                channel: self.channel,
            });
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum InputTrackModelError {
    #[error("channel for {slot:?} must be in 1..=16 (got {channel})")]
    ChannelOutOfRange { slot: ReferenceSlot, channel: u8 },
    #[error(
        "live channel {channel} is already assigned to {existing_slot:?} and cannot also be assigned to {conflicting_slot:?}"
    )]
    DuplicateLiveChannel {
        channel: u8,
        existing_slot: ReferenceSlot,
        conflicting_slot: ReferenceSlot,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputTrackModel {
    slot_sources: [ReferenceSource; REFERENCE_SLOT_COUNT],
    channel_mappings: Vec<ChannelMapping>,
}

impl InputTrackModel {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn source_for_slot(&self, slot: ReferenceSlot) -> ReferenceSource {
        self.slot_sources[slot_index(slot)]
    }

    pub fn set_source_for_slot(&mut self, slot: ReferenceSlot, source: ReferenceSource) {
        self.slot_sources[slot_index(slot)] = source;
    }

    pub fn channel_mappings(&self) -> &[ChannelMapping] {
        &self.channel_mappings
    }

    pub fn live_channel_mappings(&self) -> Vec<ChannelMapping> {
        self.channel_mappings
            .iter()
            .copied()
            .filter(|mapping| self.source_for_slot(mapping.slot) == ReferenceSource::Live)
            .collect()
    }

    pub fn replace_channel_mappings(
        &mut self,
        mappings: Vec<ChannelMapping>,
    ) -> Result<(), InputTrackModelError> {
        validate_channel_mappings(&self.slot_sources, &mappings)?;
        self.channel_mappings = mappings;
        Ok(())
    }

    pub fn set_channel_mapping(
        &mut self,
        mapping: ChannelMapping,
    ) -> Result<(), InputTrackModelError> {
        let mut next = self.channel_mappings.clone();
        if let Some(existing) = next.iter_mut().find(|item| item.slot == mapping.slot) {
            *existing = mapping;
        } else {
            next.push(mapping);
        }

        validate_channel_mappings(&self.slot_sources, &next)?;
        self.channel_mappings = next;
        Ok(())
    }

    pub fn validate(&self) -> Result<(), InputTrackModelError> {
        validate_channel_mappings(&self.slot_sources, &self.channel_mappings)
    }
}

impl Default for InputTrackModel {
    fn default() -> Self {
        Self {
            slot_sources: [ReferenceSource::File; REFERENCE_SLOT_COUNT],
            channel_mappings: default_live_channel_mappings(),
        }
    }
}

pub fn default_live_channel_mappings() -> Vec<ChannelMapping> {
    vec![
        ChannelMapping {
            slot: ReferenceSlot::Melody,
            channel: 1,
        },
        ChannelMapping {
            slot: ReferenceSlot::ChordProgression,
            channel: 2,
        },
        ChannelMapping {
            slot: ReferenceSlot::DrumPattern,
            channel: 10,
        },
        ChannelMapping {
            slot: ReferenceSlot::Bassline,
            channel: 3,
        },
    ]
}

fn validate_channel_mappings(
    slot_sources: &[ReferenceSource; REFERENCE_SLOT_COUNT],
    channel_mappings: &[ChannelMapping],
) -> Result<(), InputTrackModelError> {
    let mut live_channel_slots = HashMap::new();

    for mapping in channel_mappings {
        mapping.validate()?;

        if source_for_slot(slot_sources, mapping.slot) != ReferenceSource::Live {
            continue;
        }

        if let Some(existing_slot) = live_channel_slots.insert(mapping.channel, mapping.slot)
            && existing_slot != mapping.slot
        {
            return Err(InputTrackModelError::DuplicateLiveChannel {
                channel: mapping.channel,
                existing_slot,
                conflicting_slot: mapping.slot,
            });
        }
    }

    Ok(())
}

fn source_for_slot(
    slot_sources: &[ReferenceSource; REFERENCE_SLOT_COUNT],
    slot: ReferenceSlot,
) -> ReferenceSource {
    slot_sources[slot_index(slot)]
}

fn slot_index(slot: ReferenceSlot) -> usize {
    match slot {
        ReferenceSlot::Melody => 0,
        ReferenceSlot::ChordProgression => 1,
        ReferenceSlot::DrumPattern => 2,
        ReferenceSlot::Bassline => 3,
        ReferenceSlot::CounterMelody => 4,
        ReferenceSlot::Harmony => 5,
        ReferenceSlot::ContinuationSeed => 6,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ChannelMapping, InputTrackModel, InputTrackModelError, default_live_channel_mappings,
    };
    use crate::domain::{ReferenceSlot, ReferenceSource};

    #[test]
    fn source_can_be_stored_per_reference_slot() {
        let mut model = InputTrackModel::new();

        assert_eq!(
            model.source_for_slot(ReferenceSlot::Melody),
            ReferenceSource::File
        );
        assert_eq!(
            model.source_for_slot(ReferenceSlot::ChordProgression),
            ReferenceSource::File
        );

        model.set_source_for_slot(ReferenceSlot::Melody, ReferenceSource::Live);
        model.set_source_for_slot(ReferenceSlot::DrumPattern, ReferenceSource::Live);

        assert_eq!(
            model.source_for_slot(ReferenceSlot::Melody),
            ReferenceSource::Live
        );
        assert_eq!(
            model.source_for_slot(ReferenceSlot::DrumPattern),
            ReferenceSource::Live
        );
        assert_eq!(
            model.source_for_slot(ReferenceSlot::Harmony),
            ReferenceSource::File
        );
    }

    #[test]
    fn default_channel_mapping_helper_matches_spec() {
        assert_eq!(
            default_live_channel_mappings(),
            vec![
                ChannelMapping {
                    slot: ReferenceSlot::Melody,
                    channel: 1,
                },
                ChannelMapping {
                    slot: ReferenceSlot::ChordProgression,
                    channel: 2,
                },
                ChannelMapping {
                    slot: ReferenceSlot::DrumPattern,
                    channel: 10,
                },
                ChannelMapping {
                    slot: ReferenceSlot::Bassline,
                    channel: 3,
                },
            ]
        );
    }

    #[test]
    fn channel_range_validation_rejects_values_outside_midi_channel_range() {
        let mut model = InputTrackModel::new();
        model.set_source_for_slot(ReferenceSlot::Melody, ReferenceSource::Live);

        let error = model
            .replace_channel_mappings(vec![ChannelMapping {
                slot: ReferenceSlot::Melody,
                channel: 0,
            }])
            .expect_err("channel 0 should be rejected");

        assert_eq!(
            error,
            InputTrackModelError::ChannelOutOfRange {
                slot: ReferenceSlot::Melody,
                channel: 0,
            }
        );
    }

    #[test]
    fn duplicate_live_channel_is_rejected() {
        let mut model = InputTrackModel::new();
        model.set_source_for_slot(ReferenceSlot::Melody, ReferenceSource::Live);
        model.set_source_for_slot(ReferenceSlot::ChordProgression, ReferenceSource::Live);

        let error = model
            .replace_channel_mappings(vec![
                ChannelMapping {
                    slot: ReferenceSlot::Melody,
                    channel: 1,
                },
                ChannelMapping {
                    slot: ReferenceSlot::ChordProgression,
                    channel: 1,
                },
            ])
            .expect_err("duplicate live channel should be rejected");

        assert_eq!(
            error,
            InputTrackModelError::DuplicateLiveChannel {
                channel: 1,
                existing_slot: ReferenceSlot::Melody,
                conflicting_slot: ReferenceSlot::ChordProgression,
            }
        );
    }

    #[test]
    fn duplicate_channel_is_allowed_when_slots_are_not_both_live() {
        let mut model = InputTrackModel::new();
        model.set_source_for_slot(ReferenceSlot::Melody, ReferenceSource::Live);
        model.set_source_for_slot(ReferenceSlot::ChordProgression, ReferenceSource::File);

        model
            .replace_channel_mappings(vec![
                ChannelMapping {
                    slot: ReferenceSlot::Melody,
                    channel: 1,
                },
                ChannelMapping {
                    slot: ReferenceSlot::ChordProgression,
                    channel: 1,
                },
            ])
            .expect("duplicate channel is valid when one slot is file source");

        assert_eq!(
            model.live_channel_mappings(),
            vec![ChannelMapping {
                slot: ReferenceSlot::Melody,
                channel: 1,
            }]
        );
    }
}
