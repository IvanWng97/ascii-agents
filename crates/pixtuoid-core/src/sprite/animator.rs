use std::time::{Duration, SystemTime};

pub fn frame_index_at(start: SystemTime, now: SystemTime, frame_ms: u32, n_frames: usize) -> usize {
    if n_frames <= 1 {
        return 0;
    }
    let elapsed = now
        .duration_since(start)
        .unwrap_or(Duration::ZERO)
        .as_millis();
    let frame_ms = frame_ms.max(1) as u128;
    (elapsed / frame_ms) as usize % n_frames
}
