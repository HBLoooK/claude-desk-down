//! Local dictation (SenseVoice): transcription engine, streaming protocol for Claude native dictation, standalone local dictation service.

pub mod engine;
pub mod server;
pub mod stream;

pub use engine::{EngineError, model_dir, model_ready, model_status, prepare_model};