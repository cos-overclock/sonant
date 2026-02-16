use sonant::app::{
    ChannelMapping, InputTrackModel, InputTrackModelError, LiveInputEvent, MidiInputRouter,
};
use sonant::domain::{
    GenerationMode, GenerationParams, GenerationRequest, MidiReferenceEvent, MidiReferenceSummary,
    ModelRef, ReferenceSlot, ReferenceSource, calculate_reference_density_hint,
};

const ALL_REFERENCE_SLOTS: [ReferenceSlot; 7] = [
    ReferenceSlot::Melody,
    ReferenceSlot::ChordProgression,
    ReferenceSlot::DrumPattern,
    ReferenceSlot::Bassline,
    ReferenceSlot::CounterMelody,
    ReferenceSlot::Harmony,
    ReferenceSlot::ContinuationSeed,
];

#[test]
fn live_input_builds_generation_request_references_for_recording_enabled_channels_only() {
    let mut model = InputTrackModel::new();
    model
        .set_source_for_slot(ReferenceSlot::Melody, ReferenceSource::Live)
        .expect("melody should switch to live input");
    model
        .set_source_for_slot(ReferenceSlot::ChordProgression, ReferenceSource::Live)
        .expect("chord progression should switch to live input");

    let router = MidiInputRouter::new();
    router
        .update_channel_mapping(model.live_channel_mappings())
        .expect("live channel mapping should be valid");
    router
        .set_recording_channel_enabled(1, true)
        .expect("channel 1 should be valid");
    router
        .set_recording_channel_enabled(2, true)
        .expect("channel 2 should be valid");

    router.update_transport_state(true, 0.0);
    router.push_live_event(1, note_on(1, 60, 0));
    router.push_live_event(2, note_on(2, 65, 0));

    let mut recording_channel_enabled = [false; 16];
    recording_channel_enabled[0] = true;
    recording_channel_enabled[1] = false;

    let references = collect_live_references(&model, &recording_channel_enabled, &router);

    assert_eq!(references.len(), 1);
    assert_eq!(references[0].slot, ReferenceSlot::Melody);
    assert_eq!(references[0].source, ReferenceSource::Live);
    assert_eq!(references[0].file, None);
    assert_eq!(references[0].note_count, 1);
    assert_eq!(references[0].bars, 1);
    assert_eq!(references[0].min_pitch, 60);
    assert_eq!(references[0].max_pitch, 60);
    assert_eq!(references[0].events.len(), 1);
    assert!(references[0].events[0].event.contains("LiveMidi"));

    let request = valid_continuation_request(references.clone());
    request
        .validate()
        .expect("continuation request with live references should be valid");
    assert_eq!(request.references, references);
}

#[test]
fn live_input_overwrites_reentered_bar_and_keeps_other_bars_when_building_references() {
    let mut model = InputTrackModel::new();
    model
        .set_source_for_slot(ReferenceSlot::Melody, ReferenceSource::Live)
        .expect("melody should switch to live input");

    let router = MidiInputRouter::new();
    router
        .update_channel_mapping(model.live_channel_mappings())
        .expect("live channel mapping should be valid");
    router
        .set_recording_channel_enabled(1, true)
        .expect("channel 1 should be valid");

    router.update_transport_state(true, 0.0);
    router.push_live_event(1, note_on(1, 60, 0));
    router.push_live_event(1, note_on(1, 64, 0));

    router.update_transport_state(true, 4.0);
    router.push_live_event(1, note_on(1, 67, 0));

    router.update_transport_state(true, 0.0);
    router.push_live_event(1, note_on(1, 72, 0));

    let mut recording_channel_enabled = [false; 16];
    recording_channel_enabled[0] = true;

    let references = collect_live_references(&model, &recording_channel_enabled, &router);
    assert_eq!(references.len(), 1);

    let melody_reference = &references[0];
    assert_eq!(melody_reference.slot, ReferenceSlot::Melody);
    assert_eq!(melody_reference.bars, 2);
    assert_eq!(melody_reference.note_count, 2);
    assert_eq!(melody_reference.events.len(), 2);
    assert!(melody_reference.events[0].event.contains("data1=72"));
    assert!(melody_reference.events[1].event.contains("data1=67"));

    let request = valid_continuation_request(references);
    request
        .validate()
        .expect("continuation request should stay valid after bar overwrite");
}

#[test]
fn duplicate_live_channel_mapping_is_rejected_until_mapping_is_resolved() {
    let mut model = InputTrackModel::new();
    model
        .set_source_for_slot(ReferenceSlot::Melody, ReferenceSource::Live)
        .expect("melody should switch to live input");
    model
        .set_source_for_slot(ReferenceSlot::ChordProgression, ReferenceSource::Live)
        .expect("chord progression should switch to live input");

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
        .expect_err("duplicate live channel mapping should be rejected");

    assert_eq!(
        error,
        InputTrackModelError::DuplicateLiveChannel {
            channel: 1,
            existing_slot: ReferenceSlot::Melody,
            conflicting_slot: ReferenceSlot::ChordProgression,
        }
    );

    model
        .replace_channel_mappings(vec![
            ChannelMapping {
                slot: ReferenceSlot::Melody,
                channel: 1,
            },
            ChannelMapping {
                slot: ReferenceSlot::ChordProgression,
                channel: 3,
            },
        ])
        .expect("resolved channel mapping should be accepted");

    let router = MidiInputRouter::new();
    router
        .update_channel_mapping(model.live_channel_mappings())
        .expect("resolved mapping should be routable");
    router
        .set_recording_channel_enabled(1, true)
        .expect("channel 1 should be valid");
    router
        .set_recording_channel_enabled(3, true)
        .expect("channel 3 should be valid");

    router.update_transport_state(true, 0.0);
    router.push_live_event(1, note_on(1, 60, 0));
    router.push_live_event(3, note_on(3, 65, 0));

    let mut recording_channel_enabled = [false; 16];
    recording_channel_enabled[0] = true;
    recording_channel_enabled[2] = true;
    let references = collect_live_references(&model, &recording_channel_enabled, &router);

    assert_eq!(references.len(), 2);
    assert_eq!(references[0].slot, ReferenceSlot::Melody);
    assert_eq!(references[1].slot, ReferenceSlot::ChordProgression);

    valid_continuation_request(references)
        .validate()
        .expect("generation should become possible after resolving duplicate mapping");
}

fn valid_continuation_request(references: Vec<MidiReferenceSummary>) -> GenerationRequest {
    GenerationRequest {
        request_id: "fr03b-live-ref-req".to_string(),
        model: ModelRef {
            provider: "anthropic".to_string(),
            model: "claude-3-5-sonnet".to_string(),
        },
        mode: GenerationMode::Continuation,
        prompt: "continue with live groove".to_string(),
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

fn note_on(channel: u8, note: u8, time: u32) -> LiveInputEvent {
    LiveInputEvent {
        time,
        port_index: 0,
        data: [0x90 | ((channel - 1) & 0x0F), note, 100],
    }
}

fn collect_live_references(
    model: &InputTrackModel,
    recording_channel_enabled: &[bool; 16],
    router: &MidiInputRouter,
) -> Vec<MidiReferenceSummary> {
    let channel_mappings = model.channel_mappings();

    ALL_REFERENCE_SLOTS
        .iter()
        .copied()
        .filter_map(|slot| {
            if model.source_for_slot(slot) != ReferenceSource::Live {
                return None;
            }

            let channel = channel_mappings
                .iter()
                .find(|mapping| mapping.slot == slot)
                .map(|mapping| mapping.channel)?;
            if !recording_enabled_for_channel(recording_channel_enabled, channel) {
                return None;
            }

            let events = router.snapshot_reference(slot);
            let metrics = router.reference_metrics(slot);
            build_live_reference_summary(slot, &events, metrics.bar_count)
        })
        .collect()
}

fn recording_enabled_for_channel(recording_channel_enabled: &[bool; 16], channel: u8) -> bool {
    if !(1..=16).contains(&channel) {
        return false;
    }

    recording_channel_enabled[usize::from(channel - 1)]
}

fn build_live_reference_summary(
    slot: ReferenceSlot,
    events: &[LiveInputEvent],
    bar_count: usize,
) -> Option<MidiReferenceSummary> {
    let mut note_count = 0_u32;
    let mut min_pitch: Option<u8> = None;
    let mut max_pitch: Option<u8> = None;
    for event in events.iter().copied() {
        if is_note_on_event(event) {
            let pitch = event.data[1];
            note_count = note_count.saturating_add(1);
            min_pitch = Some(min_pitch.map_or(pitch, |current| current.min(pitch)));
            max_pitch = Some(max_pitch.map_or(pitch, |current| current.max(pitch)));
        }
    }

    let (Some(min_pitch), Some(max_pitch)) = (min_pitch, max_pitch) else {
        return None;
    };
    if note_count == 0 {
        return None;
    }

    let bars = u16::try_from(bar_count.max(1)).unwrap_or(u16::MAX);
    let reference = MidiReferenceSummary {
        slot,
        source: ReferenceSource::Live,
        file: None,
        bars,
        note_count,
        density_hint: calculate_reference_density_hint(note_count, bars),
        min_pitch,
        max_pitch,
        events: build_live_reference_events(events),
    };

    reference.validate().ok().map(|_| reference)
}

fn build_live_reference_events(events: &[LiveInputEvent]) -> Vec<MidiReferenceEvent> {
    let mut absolute_tick = 0_u32;

    events
        .iter()
        .copied()
        .map(|event| {
            let delta_tick = event.time;
            absolute_tick = absolute_tick.saturating_add(delta_tick);
            MidiReferenceEvent {
                track: event.port_index,
                absolute_tick,
                delta_tick,
                event: format_live_reference_event_payload(event),
            }
        })
        .collect()
}

fn format_live_reference_event_payload(event: LiveInputEvent) -> String {
    let channel = midi_channel_from_status(event.data[0])
        .map(|channel| channel.to_string())
        .unwrap_or_else(|| "n/a".to_string());
    format!(
        "LiveMidi channel={channel} status=0x{:02X} data1={} data2={} port={} time={}",
        event.data[0], event.data[1], event.data[2], event.port_index, event.time
    )
}

fn midi_channel_from_status(status: u8) -> Option<u8> {
    if (status & 0x80) == 0 {
        return None;
    }

    match status & 0xF0 {
        0x80 | 0x90 | 0xA0 | 0xB0 | 0xC0 | 0xD0 | 0xE0 => Some((status & 0x0F) + 1),
        _ => None,
    }
}

fn is_note_on_event(event: LiveInputEvent) -> bool {
    (event.data[0] & 0xF0) == 0x90 && event.data[2] > 0
}
