//! Library surface for the existing `client` implementation (M7 UI).
//!
//! The CLI binary in `src/main.rs` keeps its own module tree unchanged; this
//! file only republishes the receive/decode/stats modules for reuse.

pub mod decoder;
pub mod net;
pub mod stats;
