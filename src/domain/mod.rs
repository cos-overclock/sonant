mod errors;
mod generation_contract;
mod midi_path;

pub use errors::{LlmError, LlmErrorCategory};
pub use generation_contract::{
    FileReferenceInput, GeneratedNote, GenerationCandidate, GenerationMetadata, GenerationMode,
    GenerationParams, GenerationRequest, GenerationResult, GenerationUsage, MidiReferenceEvent,
    MidiReferenceSummary, ModelRef, ReferenceSlot, ReferenceSource,
    calculate_reference_density_hint,
};
pub use midi_path::has_supported_midi_extension;
