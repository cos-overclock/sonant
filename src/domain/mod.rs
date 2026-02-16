mod errors;
mod generation_contract;

pub use errors::{LlmError, LlmErrorCategory};
pub use generation_contract::{
    FileReferenceInput, GeneratedNote, GenerationCandidate, GenerationMetadata, GenerationMode,
    GenerationParams, GenerationRequest, GenerationResult, GenerationUsage, MidiReferenceEvent,
    MidiReferenceSummary, ModelRef, ReferenceSlot, ReferenceSource,
};
