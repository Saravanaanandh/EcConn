//! Library surface for the existing `server` implementation (M7 UI).
//!
//! The CLI binary in `src/main.rs` keeps its own module tree unchanged; this
//! file only republishes the same modules so the GUI host can call them
//! (virtual-display attach/detach, monitor enumeration) without rewriting the
//! streaming pipeline.

pub mod capture;
pub mod encoder;
pub mod input;
pub mod net;

#[cfg(windows)]
pub mod driver_install;
#[cfg(windows)]
pub mod orientation;
#[cfg(windows)]
pub mod virtual_display;
