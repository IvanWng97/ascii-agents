use std::io::{stdout, Stdout};
use std::time::{Duration, SystemTime};

use anyhow::Result;
use ascii_agents_core::sprite::animator::frame_index_at;
use ascii_agents_core::sprite::blit::{
    blit_frame, blit_frame_outlined, draw_line,
};
use ascii_agents_core::sprite::format::Pack;
use ascii_agents_core::sprite::{Frame, Palette, Pixel, Rgb, RgbBuffer};
use ascii_agents_core::state::ActivityState;
use ascii_agents_core::SceneState;
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Terminal;

use crate::tui::frame_cache::FrameCache;

pub type Term = Terminal<CrosstermBackend<Stdout>>;

const SHIRT_PRESETS: &[Rgb] = &[
    Rgb(0x2e, 0x62, 0xcf),
    Rgb(0x16, 0xa0, 0x6e),
    Rgb(0xb0, 0x32, 0xa8),
    Rgb(0xc6, 0x6a, 0x1e),
    Rgb(0x6c, 0x4f, 0x9e),
    Rgb(0x9c, 0x27, 0x27),
    Rgb(0x32, 0x82, 0x9b),
    Rgb(0x80, 0x55, 0x32),
];

const HAIR_PRESETS: &[Rgb] = &[
    Rgb(0x2a, 0x1a, 0x0e),
    Rgb(0x52, 0x32, 0x10),
    Rgb(0xc7, 0xa3, 0x4a),
    Rgb(0x7a, 0x32, 0x10),
    Rgb(0x3a, 0x3a, 0x3a),
];

const BG: Rgb = Rgb(20, 22, 28);
const WALL: Rgb = Rgb(40, 44, 60);
const WALL_TRIM: Rgb = Rgb(64, 60, 50);
const WINDOW_FRAME: Rgb = Rgb(24, 24, 32);
const WINDOW_LIGHT: Rgb = Rgb(120, 160, 200);
const WINDOW_LIGHT_2: Rgb = Rgb(160, 190, 220);
const PARTITION: Rgb = Rgb(60, 56, 50);
const OUTLINE: Rgb = Rgb(14, 16, 22);
const FLOOR_A: Rgb = Rgb(96, 70, 44);
const FLOOR_B: Rgb = Rgb(78, 56, 34);

pub fn setup_terminal() -> Result<Term> {
    enable_raw_mode()?;
    let mut out = stdout();
    execute!(out, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(out);
    Ok(Terminal::new(backend)?)
}

pub fn teardown_terminal(term: &mut Term) -> Result<()> {
    disable_raw_mode()?;
    execute!(term.backend_mut(), LeaveAlternateScreen)?;
    term.show_cursor()?;
    Ok(())
}

fn agent_shirt(seed: u64) -> Rgb {
    SHIRT_PRESETS[(seed as usize) % SHIRT_PRESETS.len()]
}

const WANDER_WALK_OUT_MS: u64 = 2000;
const WANDER_IDLE_MS: u64 = 2000;
const WANDER_WALK_BACK_MS: u64 = 2000;
const WANDER_TOTAL_MS: u64 = WANDER_WALK_OUT_MS + WANDER_IDLE_MS + WANDER_WALK_BACK_MS;

/// Per-agent deterministic wander offset: returns (dx, dy, in_wander, is_walking).
/// Only fires when the slot has been Idle for less than WANDER_TOTAL_MS.
fn wander_offset(
    slot: &ascii_agents_core::state::AgentSlot,
    now: SystemTime,
) -> (i32, i32, bool, bool) {
    if !matches!(slot.state, ActivityState::Idle) {
        return (0, 0, false, false);
    }
    let elapsed_ms = now
        .duration_since(slot.state_started_at)
        .unwrap_or(Duration::ZERO)
        .as_millis() as u64;
    if elapsed_ms >= WANDER_TOTAL_MS {
        return (0, 0, false, false);
    }

    let h = slot.agent_id.raw();
    // Direction: -1 or +1 based on a bit of the hash.
    let sign: i32 = if (h >> 4) & 1 == 0 { -1 } else { 1 };
    // Magnitude: 5..10 px sideways.
    let mag: i32 = 5 + ((h >> 5) % 6) as i32;
    let target_dx = sign * mag;
    // Forward (toward camera) 4..8 px.
    let target_dy = 4 + ((h >> 8) % 5) as i32;

    if elapsed_ms < WANDER_WALK_OUT_MS {
        // Walking out: interpolate 0 → target.
        let p = elapsed_ms as f32 / WANDER_WALK_OUT_MS as f32;
        let dx = (target_dx as f32 * p) as i32;
        let dy = (target_dy as f32 * p) as i32;
        (dx, dy, true, true)
    } else if elapsed_ms < WANDER_WALK_OUT_MS + WANDER_IDLE_MS {
        // Standing at wander spot.
        (target_dx, target_dy, true, false)
    } else {
        // Walking back: interpolate target → 0.
        let phase_elapsed = elapsed_ms - WANDER_WALK_OUT_MS - WANDER_IDLE_MS;
        let p = phase_elapsed as f32 / WANDER_WALK_BACK_MS as f32;
        let dx = (target_dx as f32 * (1.0 - p)) as i32;
        let dy = (target_dy as f32 * (1.0 - p)) as i32;
        (dx, dy, true, true)
    }
}

const SCREEN_IDLE: Rgb = Rgb(70, 110, 140);
const SCREEN_TYPING: Rgb = Rgb(80, 220, 110);
const SCREEN_WAITING: Rgb = Rgb(240, 200, 60);

/// Blit the monitor sprite with its screen recolored to reflect agent state.
fn blit_monitor_state(
    pack: &Pack,
    state: &ActivityState,
    dx: u16,
    dy: u16,
    buf: &mut RgbBuffer,
) {
    let Some(anim) = pack.animation("monitor") else { return; };
    let Some(frame) = anim.frames.first() else { return; };
    let base_c = base_rgb_for(&pack.palette, 'c');
    let target = match state {
        ActivityState::Idle => SCREEN_IDLE,
        ActivityState::Active { .. } => SCREEN_TYPING,
        ActivityState::Waiting { .. } => SCREEN_WAITING,
    };
    let mut out = frame.clone();
    for px in out.pixels.iter_mut() {
        if let Some(rgb) = *px {
            if Some(rgb) == base_c {
                *px = Some(target);
            }
        }
    }
    blit_frame_outlined(&out, dx, dy, buf, OUTLINE);
}

fn agent_hair(seed: u64) -> Rgb {
    HAIR_PRESETS[((seed >> 8) as usize) % HAIR_PRESETS.len()]
}

/// Look up the base RGB for a palette key. Returns None if the key isn't
/// defined or maps to transparent.
fn base_rgb_for(palette: &Palette, key: char) -> Option<Rgb> {
    palette.get(key).flatten()
}

/// Recolor a frame: substitute any pixel matching base 'B' or 'H' RGB
/// with the per-agent equivalents. v1's "pixel substitution" approach —
/// works because each palette key has a unique RGB.
fn recolor_frame(frame: &Frame, base_palette: &Palette, shirt: Rgb, hair: Rgb) -> Frame {
    let base_b = base_rgb_for(base_palette, 'B');
    let base_h = base_rgb_for(base_palette, 'H');
    let mut out = frame.clone();
    for px in out.pixels.iter_mut() {
        if let Some(rgb) = *px {
            if Some(rgb) == base_b {
                *px = Some(shirt);
            } else if Some(rgb) == base_h {
                *px = Some(hair);
            }
        }
    }
    out
}

/// Derived dimensions / coordinates for one frame. Computed once at the top
/// of draw_scene and passed to the paint_* helpers so they share consistent
/// geometry.
#[derive(Debug, Clone, Copy)]
struct Layout {
    buf_w: u16,
    buf_h: u16,
    wall_h: u16,
    floor_start: u16,
    slot_w: u16,
    slot_left_padding: u16,
    stack_h: u16,
    row_h: u16,
    grid_top: u16,
    cols_per_row: u16,
    rows_per_screen: u16,
    max_slots: u16,
}

impl Layout {
    fn compute(scene_w: u16, scene_h: u16) -> Self {
        let buf_w = scene_w;
        let buf_h = scene_h * 2;
        let wall_h: u16 = 8;
        let floor_start = wall_h + 1;
        let slot_w: u16 = 18;
        let slot_left_padding: u16 = 4;
        let stack_h: u16 = 4 + 12 + 6; // chair gap + character + desk
        let row_gap: u16 = 3;
        let row_h = stack_h + row_gap;
        let floor_h = buf_h.saturating_sub(floor_start);
        let cols_per_row = (buf_w.saturating_sub(slot_left_padding)) / slot_w;
        let rows_per_screen = std::cmp::max(1u16, floor_h / row_h);
        // grid_h = N row stacks + (N-1) gaps between them.
        let grid_h = rows_per_screen * row_h - row_gap;
        let grid_top = floor_start + floor_h.saturating_sub(grid_h) / 2;
        let max_slots = rows_per_screen * cols_per_row;
        Layout {
            buf_w,
            buf_h,
            wall_h,
            floor_start,
            slot_w,
            slot_left_padding,
            stack_h,
            row_h,
            grid_top,
            cols_per_row,
            rows_per_screen,
            max_slots,
        }
    }

    fn slot_origin(&self, i: u16) -> (u16, u16) {
        let row = i / self.cols_per_row;
        let col = i % self.cols_per_row;
        let sx = self.slot_left_padding + col * self.slot_w;
        let sy = self.grid_top + row * self.row_h;
        (sx, sy)
    }
}

pub fn draw_scene<B: Backend>(
    term: &mut Terminal<B>,
    scene: &SceneState,
    pack: &Pack,
    now: SystemTime,
    buf: &mut RgbBuffer,
    cache: &mut FrameCache,
) -> Result<()> {
    let agents: Vec<_> = scene.agents.values().cloned().collect();
    term.draw(|f| {
        let size = f.area();

        // Top status bar.
        let title = Paragraph::new(Line::from(vec![
            Span::raw(" ascii-agents — "),
            Span::raw(format!(
                "{} session{} ",
                agents.len(),
                if agents.len() == 1 { "" } else { "s" }
            )),
        ]))
        .block(Block::default().borders(Borders::BOTTOM));
        f.render_widget(
            title,
            Rect {
                x: size.x,
                y: size.y,
                width: size.width,
                height: 2,
            },
        );

        // Footer.
        let footer = Paragraph::new(Span::raw(" [q] quit "))
            .style(Style::default().fg(Color::DarkGray))
            .block(Block::default().borders(Borders::TOP));
        let footer_rect = Rect {
            x: size.x,
            y: size.y + size.height - 2,
            width: size.width,
            height: 2,
        };
        f.render_widget(footer, footer_rect);

        // Scene area between title (2 rows) and footer (2 rows).
        let scene_rect = Rect {
            x: size.x,
            y: size.y + 2,
            width: size.width,
            height: size.height.saturating_sub(4),
        };

        if scene_rect.width < 16 || scene_rect.height < 10 {
            let warn = Paragraph::new("terminal too small — resize to at least 24x14");
            f.render_widget(warn, scene_rect);
            return;
        }

        // Compute one Layout for this frame and reuse the pixel buffer.
        let layout = Layout::compute(scene_rect.width, scene_rect.height);
        let buf_w = layout.buf_w;
        let buf_h = layout.buf_h;
        buf.ensure_size(buf_w, buf_h, BG);

        // --- Background: top wall band + checkered floor below ---
        let wall_h = layout.wall_h;
        for y in 0..wall_h.min(buf_h) {
            for x in 0..buf_w {
                buf.put(x, y, WALL);
            }
        }
        // Wall/floor trim line.
        if buf_h > wall_h {
            for x in 0..buf_w {
                buf.put(x, wall_h, WALL_TRIM);
            }
        }
        // Floor tiles (checkered) below the trim.
        let floor_start = layout.floor_start;
        for y in floor_start..buf_h {
            for x in 0..buf_w {
                let cell = ((x / 4) + ((y - floor_start) / 2)) % 2;
                let c = if cell == 0 { FLOOR_A } else { FLOOR_B };
                buf.put(x, y, c);
            }
        }

        // --- Windows + framed posters in the top wall band ---
        let window_w: u16 = 6;
        let window_h: u16 = 5;
        let window_y: u16 = 1;
        let stride: u16 = 16;
        let mut wx: u16 = 4;
        let mut window_idx: u32 = 0;
        while wx + window_w < buf_w {
            for y in window_y..window_y + window_h {
                for x in wx..wx + window_w {
                    if y < buf_h && x < buf_w {
                        let inner = x > wx && x < wx + window_w - 1;
                        buf.put(x, y, if inner { WINDOW_LIGHT } else { WINDOW_FRAME });
                    }
                }
            }
            // Horizontal mullion at mid-window.
            let mid = window_y + window_h / 2;
            for x in wx..wx + window_w {
                if x < buf_w && mid < buf_h {
                    buf.put(x, mid, WINDOW_LIGHT_2);
                }
            }
            // Vertical mullion.
            let vmid = wx + window_w / 2;
            for y in window_y..window_y + window_h {
                if vmid < buf_w && y < buf_h {
                    buf.put(vmid, y, WINDOW_FRAME);
                }
            }
            // Poster in every other gap, centered between this window and the next.
            if window_idx % 2 == 0 {
                let poster_x = wx + window_w + (stride - window_w) / 2 - 3;
                let poster_y: u16 = 2;
                if poster_x + 6 < buf_w {
                    if let Some(anim) = pack.animation("poster") {
                        if let Some(frame) = anim.frames.first() {
                            blit_frame(frame, poster_x, poster_y, buf);
                        }
                    }
                }
            }
            wx += stride;
            window_idx += 1;
        }

        // --- Furniture + characters per desk slot ---
        // Dimensions all live on `layout`; aliased here for readability.
        let slot_w = layout.slot_w;
        let slot_left_padding = layout.slot_left_padding;
        let stack_h = layout.stack_h;
        let row_h = layout.row_h;
        let cols_per_row = layout.cols_per_row;
        let rows_per_screen = layout.rows_per_screen;
        let grid_top = layout.grid_top;

        // --- Cubicle partitions between adjacent slot columns ---
        // Vertical lines that separate workstations, drawn BEFORE the furniture
        // so desks + characters paint on top of them.
        if cols_per_row > 1 {
            for row in 0..rows_per_screen {
                let y_top = grid_top + row * row_h;
                let y_bot = (y_top + stack_h).min(buf_h.saturating_sub(1));
                for col in 1..cols_per_row {
                    let px = slot_left_padding + col * slot_w - 1;
                    draw_line(
                        buf,
                        px as i32,
                        y_top.saturating_sub(1) as i32,
                        px as i32,
                        y_bot as i32,
                        PARTITION,
                    );
                }
            }
        }

        // Helper to safely blit a pack animation's first frame.
        // `outlined`: paint a 1-px dark halo around the silhouette (good for
        // furniture against the busy floor pattern; bad for small accents
        // like the chair which then looks like horns above the character).
        let blit_static = |buf: &mut RgbBuffer, name: &str, dx: u16, dy: u16, outlined: bool| {
            if let Some(anim) = pack.animation(name) {
                if let Some(frame) = anim.frames.first() {
                    if outlined {
                        blit_frame_outlined(frame, dx, dy, buf, OUTLINE);
                    } else {
                        blit_frame(frame, dx, dy, buf);
                    }
                }
            }
        };

        let max_slots = layout.max_slots;
        for slot in &agents {
            let i = slot.desk_index as u16;
            if i >= max_slots {
                continue;
            }
            let (slot_x, stack_top) = layout.slot_origin(i);
            let shirt = agent_shirt(slot.agent_id.raw());
            let hair = agent_hair(slot.agent_id.raw());

            // (Chair sprite removed in v0.1.2 — the dark-brown backrest above
            // the character read as awkward "hat" pixels at this scale.
            // Chair is now implied; future revival should use a lighter office
            // chair color and a thinner backrest.)

            // 2. Character animation with positional wander.
            // After a task finishes the character takes a break: walks to a
            // wander spot offset from their desk, idles there briefly, then
            // walks back. All renderer-derived from time-since-idle.
            let (offset_x, offset_y, in_wander, is_walking) =
                wander_offset(slot, now);

            let anim_name: &'static str = if in_wander && is_walking {
                "walking"
            } else if in_wander {
                "idle"
            } else {
                match slot.state {
                    ActivityState::Idle => "idle",
                    ActivityState::Active { .. } => "typing",
                    ActivityState::Waiting { .. } => "waiting",
                }
            };

            if let Some(anim) = pack.animation(anim_name).or_else(|| pack.animation("idle")) {
                let idx = frame_index_at(
                    slot.state_started_at,
                    now,
                    anim.frame_ms,
                    anim.frames.len(),
                );
                // Cached recolor: same agent + same animation + same frame_idx
                // → reuse the previously recolored Frame instead of cloning + rewriting.
                let palette = &pack.palette;
                let frames = &anim.frames;
                let frame_rc = cache.get_or_make(slot.agent_id, anim_name, idx, || {
                    recolor_frame(&frames[idx], palette, shirt, hair)
                });

                let base_x = slot_x as i32 + 3;
                // Waiting sprite is 14 tall (raised arm) — shift up.
                let base_y = if matches!(slot.state, ActivityState::Waiting { .. }) {
                    stack_top.saturating_add(1) as i32
                } else if in_wander {
                    // Stand out of chair while wandering.
                    (stack_top + 1) as i32
                } else {
                    (stack_top + 3) as i32
                };

                let char_x = (base_x + offset_x).max(0) as u16;
                let char_y = (base_y + offset_y).max(0) as u16;
                blit_frame_outlined(frame_rc, char_x, char_y, buf, OUTLINE);
            }

            // 3. Desk in front of character (16 wide, 6 tall, slightly oversized
            //    so it occludes the character's lower body / hands).
            let desk_y = stack_top + 4 + 12;
            blit_static(buf, "desk", slot_x, desk_y, true);

            // 4. Monitor sitting on desk — color reflects current activity state.
            let monitor_y = desk_y + 1;
            let monitor_x = slot_x + 5;
            blit_monitor_state(pack, &slot.state, monitor_x, monitor_y, buf);
        }

        // --- Decorative plant in each empty visible slot ---
        for i in 0..max_slots {
            let occupied = agents.iter().any(|a| a.desk_index as u16 == i);
            if occupied {
                continue;
            }
            let (slot_x, slot_y) = layout.slot_origin(i);
            // Plant sits on a desk surface — same desk row as a normal slot.
            blit_static(buf, "desk", slot_x, slot_y + 4 + 12, true);
            blit_static(buf, "plant", slot_x + 5, slot_y + 4 + 8, true);
        }

        // --- Overflow indicator if there are more agents than visible slots ---
        let hidden = agents
            .iter()
            .filter(|a| a.desk_index as u16 >= max_slots)
            .count();
        let overflow_text = if hidden > 0 {
            Some(format!("+{hidden} more agent{}", if hidden == 1 { "" } else { "s" }))
        } else {
            None
        };

        // Write pixel buffer directly into ratatui's terminal Buffer as
        // half-block cells. Avoids allocating Vec<Vec<HalfCell>> + Vec<Line>
        // + ~2000 Spans per frame.
        let term_buf = f.buffer_mut();
        let w = buf.width as usize;
        let h = buf.height as usize;
        let cell_rows = h.div_ceil(2);
        for cy in 0..cell_rows {
            let py_top = cy * 2;
            let py_bot = (py_top + 1).min(h.saturating_sub(1));
            for cx in 0..w {
                let x = scene_rect.x + cx as u16;
                let y = scene_rect.y + cy as u16;
                if x >= scene_rect.x + scene_rect.width
                    || y >= scene_rect.y + scene_rect.height
                {
                    continue;
                }
                let fg = buf.pixels[py_top * w + cx];
                let bg = buf.pixels[py_bot * w + cx];
                let cell = &mut term_buf[(x, y)];
                cell.set_symbol("▀");
                cell.fg = Color::Rgb(fg.0, fg.1, fg.2);
                cell.bg = Color::Rgb(bg.0, bg.1, bg.2);
            }
        }

        // Labels under each desk + speech bubble overlay for waiting state.
        for slot in &agents {
            let i = slot.desk_index as u16;
            if i >= max_slots {
                continue;
            }
            let (sx, sy) = layout.slot_origin(i);
            let slot_x = scene_rect.x + sx;
            // Label sits just below the desk row of this slot, in cell coords
            // (each cell = 2 px, so divide by 2).
            let label_y = scene_rect.y + (sy + stack_h).div_ceil(2);
            let style = Style::default().fg(Color::White);
            let label = Paragraph::new(Line::from(vec![Span::styled(
                format!("{} {}", slot.label, summarize_state(&slot.state)),
                style,
            )]));
            f.render_widget(
                label,
                Rect {
                    x: slot_x,
                    y: label_y.min(scene_rect.y + scene_rect.height.saturating_sub(1)),
                    width: slot_w,
                    height: 1,
                },
            );

            if let ActivityState::Waiting { .. } = slot.state {
                let bubble_y = scene_rect
                    .y
                    .saturating_add((sy / 2).saturating_sub(2));
                let bubble = Paragraph::new(vec![
                    Line::from(Span::styled(
                        "┌─?─┐",
                        Style::default().fg(Color::Yellow),
                    )),
                    Line::from(Span::styled(
                        "└─v─┘",
                        Style::default().fg(Color::Yellow),
                    )),
                ]);
                f.render_widget(
                    bubble,
                    Rect {
                        x: slot_x + 6,
                        y: bubble_y,
                        width: 6,
                        height: 2,
                    },
                );
            }
        }

        // Overflow text in the corner of the scene.
        if let Some(text) = overflow_text {
            let para = Paragraph::new(Line::from(Span::styled(
                text,
                Style::default().fg(Color::Yellow),
            )));
            let w = 20.min(scene_rect.width);
            f.render_widget(
                para,
                Rect {
                    x: scene_rect.x + scene_rect.width.saturating_sub(w + 1),
                    y: scene_rect.y,
                    width: w,
                    height: 1,
                },
            );
        }

        let _ = Pixel::None; // silence unused-import warning on some builds
    })?;
    Ok(())
}

fn summarize_state(s: &ActivityState) -> &'static str {
    match s {
        ActivityState::Idle => "idle",
        ActivityState::Active { .. } => "typing",
        ActivityState::Waiting { .. } => "wait?",
    }
}
