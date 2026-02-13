mod errors;
mod generation_contract;

pub use errors::LlmError;
pub use generation_contract::{
    GeneratedNote, GenerationCandidate, GenerationMetadata, GenerationMode, GenerationParams,
    GenerationRequest, GenerationResult, GenerationUsage, MidiReferenceSummary, ModelRef,
    ReferenceSource,
};
