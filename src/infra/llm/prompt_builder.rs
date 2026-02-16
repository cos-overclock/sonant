use std::fmt::Write;

use crate::domain::{
    GenerationMode, GenerationRequest, MidiReferenceSummary, ReferenceSlot, ReferenceSource,
};

use super::schema_validator::GENERATION_RESULT_JSON_SCHEMA;

const SYSTEM_PROMPT: &str =
    "You are Sonant's MIDI generation backend. Follow all constraints and output strict JSON only.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuiltPrompt {
    pub system: String,
    pub user: String,
}

pub struct PromptBuilder;

impl PromptBuilder {
    pub fn build(request: &GenerationRequest) -> BuiltPrompt {
        let mode = mode_name(request.mode);
        let mode_template = mode_template(request.mode);
        let references = render_references(&request.references);
        let user_prompt = request.prompt.trim();

        let user = format!(
            "Compose a MIDI generation response for Sonant.

Generation mode: {mode}
Mode-specific instruction:
{mode_template}

User intent prompt:
{user_prompt}

Music parameters:
- bpm: {bpm}
- key: {key}
- scale: {scale}
- density: {density}
- complexity: {complexity}

Reference MIDI summaries and event sequences:
{references}

JSON output contract (must follow exactly):
{json_contract}

Required fixed fields in your JSON output:
- request_id must equal \"{request_id}\"
- model.provider must equal \"{provider}\"
- model.model must equal \"{model}\"
- candidates must contain exactly {variation_count} items

GenerationResult JSON schema:
{schema}",
            bpm = request.params.bpm,
            key = request.params.key,
            scale = request.params.scale,
            density = request.params.density,
            complexity = request.params.complexity,
            json_contract = json_output_contract(),
            request_id = request.request_id,
            provider = request.model.provider,
            model = request.model.model,
            variation_count = request.variation_count,
            schema = GENERATION_RESULT_JSON_SCHEMA,
        );

        BuiltPrompt {
            system: SYSTEM_PROMPT.to_string(),
            user,
        }
    }
}

fn mode_name(mode: GenerationMode) -> &'static str {
    match mode {
        GenerationMode::Melody => "melody",
        GenerationMode::ChordProgression => "chord_progression",
        GenerationMode::DrumPattern => "drum_pattern",
        GenerationMode::Bassline => "bassline",
        GenerationMode::CounterMelody => "counter_melody",
        GenerationMode::Harmony => "harmony",
        GenerationMode::Continuation => "continuation",
    }
}

fn mode_template(mode: GenerationMode) -> &'static str {
    match mode {
        GenerationMode::Melody => {
            "Create a lead melody that is singable, motif-driven, and clearly inside the specified key and scale. Prioritize phrase contour and rhythmic identity over dense note spam."
        }
        GenerationMode::ChordProgression => {
            "Create a chord progression pattern with strong harmonic direction and voice-leading. Chord tones should define clear changes while remaining playable in MIDI form."
        }
        GenerationMode::DrumPattern => {
            "Create a drum groove with kick/snare/hat role separation and stable meter anchoring. Use velocity and rhythmic variation to avoid mechanical repetition."
        }
        GenerationMode::Bassline => {
            "Create a bassline that locks to harmonic and rhythmic context from references. Emphasize root/approach motion and groove support rather than melodic dominance."
        }
        GenerationMode::CounterMelody => {
            "Create a counter-melody that complements the main melody without masking it. Use contrast in register and rhythm while preserving tonal coherence."
        }
        GenerationMode::Harmony => {
            "Create a harmony line that supports the main melody with smooth interval motion and consonant voice-leading. Preserve phrasing alignment with the referenced melody."
        }
        GenerationMode::Continuation => {
            "Continue the musical idea from the provided reference ending. Preserve style, groove, and tonal continuity while introducing forward motion into the next phrase."
        }
    }
}

fn json_output_contract() -> &'static str {
    "Return exactly one JSON object and nothing else. Do not output markdown fences, prose, comments, or trailing text."
}

fn render_references(references: &[MidiReferenceSummary]) -> String {
    if references.is_empty() {
        return "- none".to_string();
    }

    let mut rendered = String::new();

    for (index, reference) in references.iter().enumerate() {
        if index > 0 {
            rendered.push('\n');
        }

        let file_path = reference
            .file
            .as_ref()
            .map(|file| file.path.as_str())
            .unwrap_or("n/a");

        writeln!(rendered, "- reference #{}", index + 1)
            .expect("failed to write reference header to String");
        writeln!(rendered, "  slot: {}", reference_slot_name(reference.slot))
            .expect("failed to write reference slot to String");
        writeln!(
            rendered,
            "  source: {}",
            reference_source_name(reference.source)
        )
        .expect("failed to write reference source to String");
        writeln!(rendered, "  file_path: {file_path}")
            .expect("failed to write reference file_path to String");
        writeln!(rendered, "  bars: {}", reference.bars)
            .expect("failed to write reference bars to String");
        writeln!(rendered, "  note_count: {}", reference.note_count)
            .expect("failed to write reference note_count to String");
        writeln!(rendered, "  density_hint: {:.3}", reference.density_hint)
            .expect("failed to write reference density_hint to String");
        writeln!(
            rendered,
            "  pitch_range: {}..{}",
            reference.min_pitch, reference.max_pitch
        )
        .expect("failed to write reference pitch_range to String");

        if reference.events.is_empty() {
            writeln!(rendered, "  events: []")
                .expect("failed to write empty events list to String");
        } else {
            writeln!(rendered, "  events:")
                .expect("failed to write events header to String");
            for event in &reference.events {
                writeln!(
                    rendered,
                    "    - track={} abs_tick={} delta_tick={} event={}",
                    event.track, event.absolute_tick, event.delta_tick, event.event
                )
                .expect("failed to write reference event to String");
            }
        }
    }

    rendered.trim_end().to_string()
}

fn reference_slot_name(slot: ReferenceSlot) -> &'static str {
    match slot {
        ReferenceSlot::Melody => "melody",
        ReferenceSlot::ChordProgression => "chord_progression",
        ReferenceSlot::DrumPattern => "drum_pattern",
        ReferenceSlot::Bassline => "bassline",
        ReferenceSlot::CounterMelody => "counter_melody",
        ReferenceSlot::Harmony => "harmony",
        ReferenceSlot::ContinuationSeed => "continuation_seed",
    }
}

fn reference_source_name(source: ReferenceSource) -> &'static str {
    match source {
        ReferenceSource::File => "file",
        ReferenceSource::Live => "live",
    }
}

#[cfg(test)]
mod tests {
    use super::PromptBuilder;
    use crate::domain::{
        FileReferenceInput, GenerationMode, GenerationParams, GenerationRequest,
        MidiReferenceEvent, MidiReferenceSummary, ModelRef, ReferenceSlot, ReferenceSource,
    };
    use crate::infra::llm::schema_validator::GENERATION_RESULT_JSON_SCHEMA;

    fn request_with_mode(mode: GenerationMode) -> GenerationRequest {
        GenerationRequest {
            request_id: "req-42".to_string(),
            model: ModelRef {
                provider: "anthropic".to_string(),
                model: "claude-3-5-sonnet".to_string(),
            },
            mode,
            prompt: "warm synth texture".to_string(),
            params: GenerationParams {
                bpm: 128,
                key: "D".to_string(),
                scale: "minor".to_string(),
                density: 4,
                complexity: 3,
                temperature: Some(0.5),
                top_p: Some(0.9),
                max_tokens: Some(512),
            },
            references: Vec::new(),
            variation_count: 2,
        }
    }

    fn file_reference() -> MidiReferenceSummary {
        MidiReferenceSummary {
            slot: ReferenceSlot::Melody,
            source: ReferenceSource::File,
            file: Some(FileReferenceInput {
                path: "refs/melody.mid".to_string(),
            }),
            bars: 4,
            note_count: 24,
            density_hint: 0.42,
            min_pitch: 60,
            max_pitch: 74,
            events: vec![MidiReferenceEvent {
                track: 0,
                absolute_tick: 0,
                delta_tick: 0,
                event: "NoteOn channel=0 key=60 vel=96".to_string(),
            }],
        }
    }

    #[test]
    fn melody_template_is_selected() {
        let prompt = PromptBuilder::build(&request_with_mode(GenerationMode::Melody));
        assert!(prompt.user.contains("Create a lead melody"));
    }

    #[test]
    fn chord_progression_template_is_selected() {
        let prompt = PromptBuilder::build(&request_with_mode(GenerationMode::ChordProgression));
        assert!(prompt.user.contains("Create a chord progression pattern"));
    }

    #[test]
    fn drum_pattern_template_is_selected() {
        let prompt = PromptBuilder::build(&request_with_mode(GenerationMode::DrumPattern));
        assert!(prompt.user.contains("Create a drum groove"));
    }

    #[test]
    fn bassline_template_is_selected() {
        let prompt = PromptBuilder::build(&request_with_mode(GenerationMode::Bassline));
        assert!(prompt.user.contains("Create a bassline"));
    }

    #[test]
    fn counter_melody_template_is_selected() {
        let prompt = PromptBuilder::build(&request_with_mode(GenerationMode::CounterMelody));
        assert!(prompt.user.contains("Create a counter-melody"));
    }

    #[test]
    fn harmony_template_is_selected() {
        let prompt = PromptBuilder::build(&request_with_mode(GenerationMode::Harmony));
        assert!(prompt.user.contains("Create a harmony line"));
    }

    #[test]
    fn continuation_template_is_selected() {
        let prompt = PromptBuilder::build(&request_with_mode(GenerationMode::Continuation));
        assert!(prompt.user.contains("Continue the musical idea"));
    }

    #[test]
    fn prompt_includes_params_and_json_output_constraints() {
        let prompt = PromptBuilder::build(&request_with_mode(GenerationMode::Melody));

        assert_eq!(
            prompt.system,
            "You are Sonant's MIDI generation backend. Follow all constraints and output strict JSON only."
        );
        assert!(prompt.user.contains("- bpm: 128"));
        assert!(prompt.user.contains("- key: D"));
        assert!(prompt.user.contains("- scale: minor"));
        assert!(prompt.user.contains("- density: 4"));
        assert!(prompt.user.contains("- complexity: 3"));
        assert!(prompt.user.contains("request_id must equal \"req-42\""));
        assert!(
            prompt
                .user
                .contains("model.provider must equal \"anthropic\"")
        );
        assert!(
            prompt
                .user
                .contains("model.model must equal \"claude-3-5-sonnet\"")
        );
        assert!(
            prompt
                .user
                .contains("candidates must contain exactly 2 items")
        );
        assert!(
            prompt
                .user
                .contains("Return exactly one JSON object and nothing else.")
        );
        assert!(prompt.user.contains(GENERATION_RESULT_JSON_SCHEMA.trim()));
    }

    #[test]
    fn prompt_includes_reference_summary_and_event_rows() {
        let mut request = request_with_mode(GenerationMode::CounterMelody);
        request.references = vec![file_reference()];

        let prompt = PromptBuilder::build(&request);

        assert!(prompt.user.contains("slot: melody"));
        assert!(prompt.user.contains("source: file"));
        assert!(prompt.user.contains("file_path: refs/melody.mid"));
        assert!(prompt.user.contains("bars: 4"));
        assert!(prompt.user.contains("note_count: 24"));
        assert!(prompt.user.contains("density_hint: 0.420"));
        assert!(prompt.user.contains("pitch_range: 60..74"));
        assert!(
            prompt
                .user
                .contains("track=0 abs_tick=0 delta_tick=0 event=NoteOn channel=0 key=60 vel=96")
        );
    }

    #[test]
    fn prompt_marks_missing_references_explicitly() {
        let prompt = PromptBuilder::build(&request_with_mode(GenerationMode::Melody));
        assert!(
            prompt
                .user
                .contains("Reference MIDI summaries and event sequences:\n- none")
        );
    }
}
