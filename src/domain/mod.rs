mod errors;
mod generation_contract;

pub use errors::LlmError;
pub use generation_contract::{
    GeneratedNote, GenerationCandidate, GenerationMode, GenerationParams, GenerationRequest,
    GenerationResult, MidiReferenceSummary, ModelRef, ReferenceSource,
};
