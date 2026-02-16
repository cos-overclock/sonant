mod errors;
mod generation_contract;

pub use errors::{LlmError, LlmErrorCategory};
pub use generation_contract::{
    FileReferenceInput, GeneratedNote, GenerationCandidate, GenerationMetadata, GenerationMode,
    GenerationParams, GenerationRequest, GenerationResult, GenerationUsage, MidiReferenceSummary,
    ModelRef, ReferenceSlot, ReferenceSource,
};
