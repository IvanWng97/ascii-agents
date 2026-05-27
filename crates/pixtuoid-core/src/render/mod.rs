use std::time::SystemTime;

use anyhow::Result;

use crate::sprite::format::Pack;
use crate::state::SceneState;

/// Anything that can paint a `SceneState` for a user. Real renderers
/// (terminal half-block, web canvas, PNG, GIF) all need scene + pack +
/// clock; layout is intentionally NOT in the trait — the half-block TUI
/// recomputes layout per frame from terminal size, and fixed-canvas
/// renderers compute theirs once at construction. Forcing layout through
/// the trait would mean every caller carries a value half the impls
/// ignore.
pub trait Renderer {
    fn render(&mut self, scene: &SceneState, pack: &Pack, now: SystemTime) -> Result<()>;
}

#[cfg(feature = "test-renderer")]
pub mod test_renderer;
