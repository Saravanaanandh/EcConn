//! Counters and rolling timings shared between the client threads.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

#[derive(Default)]
pub struct Stats {
    pub packets: AtomicU64,
    pub bytes: AtomicU64,
    pub frames_received: AtomicU64,
    pub frames_dropped: AtomicU64,
    pub frames_replaced: AtomicU64,
    pub keyframe_requests: AtomicU64,
    pub frames_rendered: AtomicU64,

    assembly_ms: Mutex<Vec<f32>>,
    decode_ms: Mutex<Vec<f32>>,
    render_ms: Mutex<Vec<f32>>,
}

impl Stats {
    pub fn record_assembly(&self, ms: f32) {
        push(&self.assembly_ms, ms);
    }

    pub fn record_decode(&self, ms: f32) {
        push(&self.decode_ms, ms);
    }

    pub fn record_render(&self, ms: f32) {
        push(&self.render_ms, ms);
    }

    pub fn take_assembly(&self) -> (f32, f32) {
        take(&self.assembly_ms)
    }

    pub fn take_decode(&self) -> (f32, f32) {
        take(&self.decode_ms)
    }

    pub fn take_render(&self) -> (f32, f32) {
        take(&self.render_ms)
    }
}

fn push(slot: &Mutex<Vec<f32>>, ms: f32) {
    if let Ok(mut values) = slot.lock() {
        values.push(ms);
    }
}

/// Returns `(avg, max)` and clears the window.
fn take(slot: &Mutex<Vec<f32>>) -> (f32, f32) {
    let Ok(mut values) = slot.lock() else {
        return (0.0, 0.0);
    };
    if values.is_empty() {
        return (0.0, 0.0);
    }
    let avg = values.iter().sum::<f32>() / values.len() as f32;
    let max = values.iter().copied().fold(f32::MIN, f32::max);
    values.clear();
    (avg, max)
}

/// Convenience for the per-second report: read and reset a counter.
pub fn take_counter(counter: &AtomicU64) -> u64 {
    counter.swap(0, Ordering::Relaxed)
}
