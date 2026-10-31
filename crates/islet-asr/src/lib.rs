//! Local speech recognition for Islet: which models exist, getting them onto
//! disk, and talking to the worker process that runs them.
//!
//! ONNX Runtime is deliberately *not* linked here. It adds ~18 MB and holds
//! memory after long clips, so it lives in `islet-asr-worker.exe`: the island
//! stays small, "unloading" the model is ending the process, and an ASR crash
//! cannot take the island down with it.

mod chunks;
pub mod client;
pub mod manager;
pub mod polish;
pub mod protocol;
pub mod registry;

pub use client::{default_worker_path, AsrClient, AsrEvent};
pub use manager::{ManagerError, ModelManager, ModelStatus, Progress};
pub use polish::{PolishError, Polisher};
pub use protocol::{LoadRequest, SAMPLE_RATE};
pub use registry::{
    find, find_or_default, AddOn, EngineKind, ModelFile, ModelSpec, AI_CLEANUP, DEFAULT_MODEL_ID,
    MODELS,
};
