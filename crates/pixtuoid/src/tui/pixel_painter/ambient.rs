//! Ambient pass — non-character, non-furniture effects painted between
//! the background and the y-sorted drawables: sun spot on wall, dust
//! motes in window spill, ceiling halos above active monitors.
//!
//! Each subroutine is independently togglable and self-contained. New
//! ambient effects go here, not in `background/` or `drawable.rs`.

use crate::tui::pixel_painter::PixelCtx;

pub(super) fn paint_ambient(_ctx: &mut PixelCtx<'_>) {
    // Subroutines added in later tasks. No-op for now.
}
