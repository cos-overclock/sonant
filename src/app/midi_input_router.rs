use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::num::NonZeroUsize;
use std::sync::Mutex;

use thiserror::Error;

use super::input_track_model::{MIDI_CHANNEL_MAX, MIDI_CHANNEL_MIN};
use super::{ChannelMapping, LiveInputEvent, default_live_channel_mappings};
use crate::domain::ReferenceSlot;

const PPQ_PER_BAR: f64 = 4.0;
const DEFAULT_MAX_BARS_PER_SLOT: usize = 64;
const DEFAULT_MAX_EVENTS_PER_BAR: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum MidiInputRouterError {
    #[error(
        "channel mapping for {slot:?} must be in {MIDI_CHANNEL_MIN}..={MIDI_CHANNEL_MAX} (got {channel})"
    )]
    ChannelOutOfRange { slot: ReferenceSlot, channel: u8 },
    #[error("channel mapping for {slot:?} must be unique")]
    DuplicateSlotMapping { slot: ReferenceSlot },
    #[error(
        "live channel {channel} is already assigned to {existing_slot:?} and cannot also be assigned to {conflicting_slot:?}"
    )]
    DuplicateChannelMapping {
        channel: u8,
        existing_slot: ReferenceSlot,
        conflicting_slot: ReferenceSlot,
    },
    #[error("recording channel must be in {MIDI_CHANNEL_MIN}..={MIDI_CHANNEL_MAX} (got {channel})")]
    RecordingChannelOutOfRange { channel: u8 },
    #[error("midi input router bar capacity must be greater than zero")]
    ZeroBarCapacity,
    #[error("midi input router events-per-bar capacity must be greater than zero")]
    ZeroEventsPerBarCapacity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LiveReferenceMetrics {
    pub bar_count: usize,
    pub event_count: usize,
}

pub struct MidiInputRouter {
    max_bars_per_slot: usize,
    max_events_per_bar: usize,
    state: Mutex<MidiInputRouterState>,
}

impl MidiInputRouter {
    pub fn new() -> Self {
        Self::with_limits(
            NonZeroUsize::new(DEFAULT_MAX_BARS_PER_SLOT)
                .expect("default router bar capacity must be non-zero"),
            NonZeroUsize::new(DEFAULT_MAX_EVENTS_PER_BAR)
                .expect("default router events-per-bar capacity must be non-zero"),
        )
    }

    pub fn with_limits(max_bars_per_slot: NonZeroUsize, max_events_per_bar: NonZeroUsize) -> Self {
        Self {
            max_bars_per_slot: max_bars_per_slot.get(),
            max_events_per_bar: max_events_per_bar.get(),
            state: Mutex::new(MidiInputRouterState::new(default_channel_to_slot_map())),
        }
    }

    pub fn try_with_limits(
        max_bars_per_slot: usize,
        max_events_per_bar: usize,
    ) -> Result<Self, MidiInputRouterError> {
        let max_bars_per_slot =
            NonZeroUsize::new(max_bars_per_slot).ok_or(MidiInputRouterError::ZeroBarCapacity)?;
        let max_events_per_bar = NonZeroUsize::new(max_events_per_bar)
            .ok_or(MidiInputRouterError::ZeroEventsPerBarCapacity)?;

        Ok(Self::with_limits(max_bars_per_slot, max_events_per_bar))
    }

    pub fn update_channel_mapping(
        &self,
        mappings: Vec<ChannelMapping>,
    ) -> Result<(), MidiInputRouterError> {
        let channel_to_slot = build_channel_to_slot_map(&mappings)?;
        let mut state = self
            .state
            .lock()
            .expect("midi input router state lock poisoned while updating channel mapping");
        state.channel_to_slot = channel_to_slot;
        Ok(())
    }

    pub fn set_recording_channel_enabled(
        &self,
        channel: u8,
        enabled: bool,
    ) -> Result<(), MidiInputRouterError> {
        validate_recording_channel(channel)?;

        let mut state = self
            .state
            .lock()
            .expect("midi input router state lock poisoned while updating recording channel");
        state.recording_channel_enabled[channel_index(channel)] = enabled;

        Ok(())
    }

    pub fn update_transport_state(&self, is_playing: bool, playhead_ppq: f64) {
        let mut state = self
            .state
            .lock()
            .expect("midi input router state lock poisoned while updating transport");

        let should_reset_active_writes =
            !is_playing || !state.is_playing || transport_rewound(state.playhead_ppq, playhead_ppq);

        if should_reset_active_writes {
            state.active_write_bar_by_slot.clear();
        }

        state.is_playing = is_playing;
        state.playhead_ppq = playhead_ppq;
    }

    pub fn push_live_event(&self, channel: u8, event: LiveInputEvent) {
        if !is_valid_channel(channel) {
            return;
        }

        let mut state = self
            .state
            .lock()
            .expect("midi input router state lock poisoned while pushing live event");

        if !state.is_playing {
            return;
        }
        if !state.recording_channel_enabled[channel_index(channel)] {
            return;
        }

        let Some(slot) = state.channel_to_slot.get(&channel).copied() else {
            return;
        };
        let Some(bar_index) = bar_index_from_playhead(state.playhead_ppq) else {
            return;
        };

        let is_new_active_bar =
            state.active_write_bar_by_slot.get(&slot).copied() != Some(bar_index);

        if is_new_active_bar {
            let slot_buffer = state.slot_buffers.entry(slot).or_default();
            slot_buffer
                .bars
                .insert(bar_index, VecDeque::with_capacity(self.max_events_per_bar));
            trim_old_bars(slot_buffer, self.max_bars_per_slot);
            state.active_write_bar_by_slot.insert(slot, bar_index);
        }

        let slot_buffer = state.slot_buffers.entry(slot).or_default();
        let bar_events = slot_buffer
            .bars
            .entry(bar_index)
            .or_insert_with(|| VecDeque::with_capacity(self.max_events_per_bar));

        if bar_events.len() >= self.max_events_per_bar {
            let _ = bar_events.pop_front();
        }
        bar_events.push_back(event);
    }

    pub fn snapshot_reference(&self, slot: ReferenceSlot) -> Vec<LiveInputEvent> {
        let state = self
            .state
            .lock()
            .expect("midi input router state lock poisoned while creating snapshot");

        let Some(slot_buffer) = state.slot_buffers.get(&slot) else {
            return Vec::new();
        };

        let mut snapshot = Vec::new();
        for events in slot_buffer.bars.values() {
            snapshot.extend(events.iter().copied());
        }
        snapshot
    }

    pub fn reference_metrics(&self, slot: ReferenceSlot) -> LiveReferenceMetrics {
        let state = self
            .state
            .lock()
            .expect("midi input router state lock poisoned while reading reference metrics");

        let Some(slot_buffer) = state.slot_buffers.get(&slot) else {
            return LiveReferenceMetrics::default();
        };

        LiveReferenceMetrics {
            bar_count: slot_buffer.bars.len(),
            event_count: slot_buffer
                .bars
                .values()
                .map(std::collections::VecDeque::len)
                .sum(),
        }
    }
}

impl Default for MidiInputRouter {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Default)]
struct MidiInputRouterState {
    channel_to_slot: HashMap<u8, ReferenceSlot>,
    recording_channel_enabled: [bool; MIDI_CHANNEL_MAX as usize],
    is_playing: bool,
    playhead_ppq: f64,
    slot_buffers: HashMap<ReferenceSlot, SlotBuffer>,
    active_write_bar_by_slot: HashMap<ReferenceSlot, u64>,
}

impl MidiInputRouterState {
    fn new(channel_to_slot: HashMap<u8, ReferenceSlot>) -> Self {
        Self {
            channel_to_slot,
            recording_channel_enabled: [false; MIDI_CHANNEL_MAX as usize],
            is_playing: false,
            playhead_ppq: 0.0,
            slot_buffers: HashMap::new(),
            active_write_bar_by_slot: HashMap::new(),
        }
    }
}

#[derive(Debug, Default)]
struct SlotBuffer {
    bars: BTreeMap<u64, VecDeque<LiveInputEvent>>,
}

fn default_channel_to_slot_map() -> HashMap<u8, ReferenceSlot> {
    build_channel_to_slot_map(&default_live_channel_mappings())
        .expect("default live channel mappings must be valid")
}

fn build_channel_to_slot_map(
    mappings: &[ChannelMapping],
) -> Result<HashMap<u8, ReferenceSlot>, MidiInputRouterError> {
    let mut seen_slots = HashSet::new();
    let mut channel_to_slot = HashMap::new();

    for mapping in mappings {
        if !is_valid_channel(mapping.channel) {
            return Err(MidiInputRouterError::ChannelOutOfRange {
                slot: mapping.slot,
                channel: mapping.channel,
            });
        }

        if !seen_slots.insert(mapping.slot) {
            return Err(MidiInputRouterError::DuplicateSlotMapping { slot: mapping.slot });
        }

        if let Some(existing_slot) = channel_to_slot.insert(mapping.channel, mapping.slot)
            && existing_slot != mapping.slot
        {
            return Err(MidiInputRouterError::DuplicateChannelMapping {
                channel: mapping.channel,
                existing_slot,
                conflicting_slot: mapping.slot,
            });
        }
    }

    Ok(channel_to_slot)
}

fn validate_recording_channel(channel: u8) -> Result<(), MidiInputRouterError> {
    if is_valid_channel(channel) {
        Ok(())
    } else {
        Err(MidiInputRouterError::RecordingChannelOutOfRange { channel })
    }
}

fn is_valid_channel(channel: u8) -> bool {
    (MIDI_CHANNEL_MIN..=MIDI_CHANNEL_MAX).contains(&channel)
}

fn channel_index(channel: u8) -> usize {
    usize::from(channel - MIDI_CHANNEL_MIN)
}

fn bar_index_from_playhead(playhead_ppq: f64) -> Option<u64> {
    if !playhead_ppq.is_finite() || playhead_ppq < 0.0 {
        return None;
    }

    Some((playhead_ppq / PPQ_PER_BAR).floor() as u64)
}

fn transport_rewound(previous_ppq: f64, current_ppq: f64) -> bool {
    match (
        normalize_playhead_ppq(previous_ppq),
        normalize_playhead_ppq(current_ppq),
    ) {
        (Some(previous), Some(current)) => current < previous,
        _ => true,
    }
}

fn normalize_playhead_ppq(playhead_ppq: f64) -> Option<f64> {
    if playhead_ppq.is_finite() && playhead_ppq >= 0.0 {
        Some(playhead_ppq)
    } else {
        None
    }
}

fn trim_old_bars(slot_buffer: &mut SlotBuffer, max_bars_per_slot: usize) {
    while slot_buffer.bars.len() > max_bars_per_slot {
        let Some((&oldest_bar, _)) = slot_buffer.bars.first_key_value() else {
            break;
        };
        slot_buffer.bars.remove(&oldest_bar);
    }
}

#[cfg(test)]
mod tests {
    use super::{LiveReferenceMetrics, MidiInputRouter, MidiInputRouterError};
    use crate::app::ChannelMapping;
    use crate::domain::ReferenceSlot;

    fn note_on(channel: u8, note: u8) -> crate::app::LiveInputEvent {
        crate::app::LiveInputEvent {
            time: 0,
            port_index: 0,
            data: [0x90 | ((channel - 1) & 0x0F), note, 100],
            is_transport_playing: true,
            playhead_ppq: 0.0,
        }
    }

    #[test]
    fn routes_event_to_slot_for_mapped_channel() {
        let router = MidiInputRouter::new();
        router
            .set_recording_channel_enabled(1, true)
            .expect("channel 1 should be valid");
        router.update_transport_state(true, 0.0);

        let event = note_on(1, 60);
        router.push_live_event(1, event);

        assert_eq!(
            router.snapshot_reference(ReferenceSlot::Melody),
            vec![event]
        );
        assert!(
            router
                .snapshot_reference(ReferenceSlot::ChordProgression)
                .is_empty()
        );
    }

    #[test]
    fn ignores_unassigned_channel() {
        let router = MidiInputRouter::new();
        router
            .update_channel_mapping(vec![ChannelMapping {
                slot: ReferenceSlot::Melody,
                channel: 1,
            }])
            .expect("mapping should be valid");
        router
            .set_recording_channel_enabled(5, true)
            .expect("channel 5 should be valid");
        router.update_transport_state(true, 0.0);

        router.push_live_event(5, note_on(5, 72));

        assert!(router.snapshot_reference(ReferenceSlot::Melody).is_empty());
    }

    #[test]
    fn ignores_event_when_recording_is_disabled() {
        let router = MidiInputRouter::new();
        router
            .update_channel_mapping(vec![ChannelMapping {
                slot: ReferenceSlot::Melody,
                channel: 1,
            }])
            .expect("mapping should be valid");
        router.update_transport_state(true, 0.0);

        router.push_live_event(1, note_on(1, 60));

        assert!(router.snapshot_reference(ReferenceSlot::Melody).is_empty());
    }

    #[test]
    fn keeps_events_only_while_transport_is_playing() {
        let router = MidiInputRouter::new();
        router
            .set_recording_channel_enabled(1, true)
            .expect("channel 1 should be valid");
        router.update_transport_state(false, 0.0);

        router.push_live_event(1, note_on(1, 60));

        assert!(router.snapshot_reference(ReferenceSlot::Melody).is_empty());
    }

    #[test]
    fn overwrite_same_bar_on_reinput_while_preserving_other_bars() {
        let router = MidiInputRouter::new();
        router
            .update_channel_mapping(vec![ChannelMapping {
                slot: ReferenceSlot::Melody,
                channel: 1,
            }])
            .expect("mapping should be valid");
        router
            .set_recording_channel_enabled(1, true)
            .expect("channel 1 should be valid");

        let first_bar_note_a = note_on(1, 60);
        let first_bar_note_b = note_on(1, 64);
        router.update_transport_state(true, 0.0);
        router.push_live_event(1, first_bar_note_a);
        router.push_live_event(1, first_bar_note_b);

        let second_bar_note = note_on(1, 67);
        router.update_transport_state(true, 4.0);
        router.push_live_event(1, second_bar_note);

        let replacement_first_bar_note = note_on(1, 72);
        router.update_transport_state(true, 0.0);
        router.push_live_event(1, replacement_first_bar_note);

        assert_eq!(
            router.snapshot_reference(ReferenceSlot::Melody),
            vec![replacement_first_bar_note, second_bar_note]
        );
    }

    #[test]
    fn mapping_update_is_applied_immediately() {
        let router = MidiInputRouter::new();
        router
            .update_channel_mapping(vec![
                ChannelMapping {
                    slot: ReferenceSlot::Melody,
                    channel: 1,
                },
                ChannelMapping {
                    slot: ReferenceSlot::ChordProgression,
                    channel: 2,
                },
            ])
            .expect("mapping should be valid");
        router
            .set_recording_channel_enabled(1, true)
            .expect("channel 1 should be valid");
        router
            .set_recording_channel_enabled(2, true)
            .expect("channel 2 should be valid");
        router.update_transport_state(true, 0.0);

        let before_update = note_on(1, 60);
        router.push_live_event(1, before_update);

        router
            .update_channel_mapping(vec![
                ChannelMapping {
                    slot: ReferenceSlot::Melody,
                    channel: 2,
                },
                ChannelMapping {
                    slot: ReferenceSlot::ChordProgression,
                    channel: 1,
                },
            ])
            .expect("updated mapping should be valid");

        let after_update = note_on(1, 62);
        router.push_live_event(1, after_update);

        assert_eq!(
            router.snapshot_reference(ReferenceSlot::Melody),
            vec![before_update]
        );
        assert_eq!(
            router.snapshot_reference(ReferenceSlot::ChordProgression),
            vec![after_update]
        );
    }

    #[test]
    fn drops_oldest_events_when_events_per_bar_capacity_is_exceeded() {
        let router =
            MidiInputRouter::try_with_limits(4, 2).expect("non-zero capacities should be valid");
        router
            .update_channel_mapping(vec![ChannelMapping {
                slot: ReferenceSlot::Melody,
                channel: 1,
            }])
            .expect("mapping should be valid");
        router
            .set_recording_channel_enabled(1, true)
            .expect("channel 1 should be valid");
        router.update_transport_state(true, 0.0);

        let event_1 = note_on(1, 60);
        let event_2 = note_on(1, 62);
        let event_3 = note_on(1, 64);
        router.push_live_event(1, event_1);
        router.push_live_event(1, event_2);
        router.push_live_event(1, event_3);

        assert_eq!(
            router.snapshot_reference(ReferenceSlot::Melody),
            vec![event_2, event_3]
        );
    }

    #[test]
    fn drops_oldest_bars_when_bar_capacity_is_exceeded() {
        let router =
            MidiInputRouter::try_with_limits(2, 8).expect("non-zero capacities should be valid");
        router
            .update_channel_mapping(vec![ChannelMapping {
                slot: ReferenceSlot::Melody,
                channel: 1,
            }])
            .expect("mapping should be valid");
        router
            .set_recording_channel_enabled(1, true)
            .expect("channel 1 should be valid");

        let bar0 = note_on(1, 60);
        router.update_transport_state(true, 0.0);
        router.push_live_event(1, bar0);

        let bar1 = note_on(1, 62);
        router.update_transport_state(true, 4.0);
        router.push_live_event(1, bar1);

        let bar2 = note_on(1, 64);
        router.update_transport_state(true, 8.0);
        router.push_live_event(1, bar2);

        assert_eq!(
            router.snapshot_reference(ReferenceSlot::Melody),
            vec![bar1, bar2]
        );
    }

    #[test]
    fn reference_metrics_report_counts_for_recorded_slot() {
        let router = MidiInputRouter::new();
        router
            .update_channel_mapping(vec![ChannelMapping {
                slot: ReferenceSlot::Melody,
                channel: 1,
            }])
            .expect("mapping should be valid");
        router
            .set_recording_channel_enabled(1, true)
            .expect("channel 1 should be valid");

        router.update_transport_state(true, 0.0);
        router.push_live_event(1, note_on(1, 60));
        router.push_live_event(1, note_on(1, 62));

        router.update_transport_state(true, 4.0);
        router.push_live_event(1, note_on(1, 64));

        assert_eq!(
            router.reference_metrics(ReferenceSlot::Melody),
            LiveReferenceMetrics {
                bar_count: 2,
                event_count: 3
            }
        );
    }

    #[test]
    fn reference_metrics_are_empty_for_unrecorded_slot() {
        let router = MidiInputRouter::new();
        assert_eq!(
            router.reference_metrics(ReferenceSlot::Harmony),
            LiveReferenceMetrics {
                bar_count: 0,
                event_count: 0
            }
        );
    }

    #[test]
    fn rejects_out_of_range_mapping_channel() {
        let router = MidiInputRouter::new();

        let error = router
            .update_channel_mapping(vec![ChannelMapping {
                slot: ReferenceSlot::Melody,
                channel: 0,
            }])
            .expect_err("channel 0 should be rejected");

        assert_eq!(
            error,
            MidiInputRouterError::ChannelOutOfRange {
                slot: ReferenceSlot::Melody,
                channel: 0,
            }
        );
    }

    #[test]
    fn rejects_duplicate_mapping_channel() {
        let router = MidiInputRouter::new();

        let error = router
            .update_channel_mapping(vec![
                ChannelMapping {
                    slot: ReferenceSlot::Melody,
                    channel: 1,
                },
                ChannelMapping {
                    slot: ReferenceSlot::ChordProgression,
                    channel: 1,
                },
            ])
            .expect_err("duplicate mapping channel should be rejected");

        assert_eq!(
            error,
            MidiInputRouterError::DuplicateChannelMapping {
                channel: 1,
                existing_slot: ReferenceSlot::Melody,
                conflicting_slot: ReferenceSlot::ChordProgression,
            }
        );
    }

    #[test]
    fn rejects_duplicate_slot_mapping() {
        let router = MidiInputRouter::new();

        let error = router
            .update_channel_mapping(vec![
                ChannelMapping {
                    slot: ReferenceSlot::Melody,
                    channel: 1,
                },
                ChannelMapping {
                    slot: ReferenceSlot::Melody,
                    channel: 2,
                },
            ])
            .expect_err("duplicate slot mapping should be rejected");

        assert_eq!(
            error,
            MidiInputRouterError::DuplicateSlotMapping {
                slot: ReferenceSlot::Melody,
            }
        );
    }

    #[test]
    fn rejects_out_of_range_recording_channel() {
        let router = MidiInputRouter::new();

        let error = router
            .set_recording_channel_enabled(17, true)
            .expect_err("channel 17 should be rejected");

        assert_eq!(
            error,
            MidiInputRouterError::RecordingChannelOutOfRange { channel: 17 }
        );
    }

    #[test]
    fn try_with_limits_rejects_zero_bar_capacity() {
        assert!(matches!(
            MidiInputRouter::try_with_limits(0, 8),
            Err(MidiInputRouterError::ZeroBarCapacity)
        ));
    }

    #[test]
    fn try_with_limits_rejects_zero_events_per_bar_capacity() {
        assert!(matches!(
            MidiInputRouter::try_with_limits(8, 0),
            Err(MidiInputRouterError::ZeroEventsPerBarCapacity)
        ));
    }
}
