pub mod chitchat;
pub mod embedded_pack;
pub mod floor;
pub mod frame_cache;
pub mod hit_test;
pub mod layout;
pub mod pathfind;
pub mod pet;
pub mod pixel_painter;
pub mod pose;
pub mod renderer;
pub mod theme;
pub mod tui_renderer;
pub mod widgets;

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyModifiers, MouseButton, MouseEventKind};
use pixtuoid_core::Renderer;

use renderer::{setup_terminal, teardown_terminal};
use tui_renderer::TuiRenderer;

use crate::runtime::SceneRx;

pub async fn run_tui(
    mut scene_rx: SceneRx,
    pack_dir: Option<std::path::PathBuf>,
    floor_caps: Arc<[std::sync::atomic::AtomicUsize; pixtuoid_core::state::MAX_FLOORS]>,
    theme: &'static theme::Theme,
    config_path: std::path::PathBuf,
    desk_cap: Option<usize>,
    enabled_pets: Vec<pet::PetKind>,
) -> Result<()> {
    let pack = embedded_pack::load_sprite_pack(pack_dir)?;
    let term = setup_terminal()?;
    let mut renderer = TuiRenderer::new(term, theme, enabled_pets);
    let mut last_layout_sig: Option<(u16, u16)> = None;
    let mut paused = false;
    let mut frozen_now: Option<SystemTime> = None;
    let mut theme_picker: Option<usize> = None;
    let mut saved_theme_idx: usize = theme::ALL_THEMES
        .iter()
        .position(|t| std::ptr::eq(*t, theme))
        .unwrap_or(0);

    let tick = Duration::from_millis(33);
    let result: Result<()> = (async {
        loop {
            let now = if paused {
                *frozen_now.get_or_insert(SystemTime::now())
            } else {
                frozen_now = None;
                SystemTime::now()
            };
            let snapshot = scene_rx.borrow_and_update().clone();
            renderer.evict_missing(&snapshot);
            let sig = (renderer.buf().width, renderer.buf().height);
            if last_layout_sig != Some(sig) {
                renderer.invalidate_routes();
                renderer.cancel_transition();
                last_layout_sig = Some(sig);
            }
            renderer.set_theme_picker(theme_picker);
            renderer.render(&snapshot, &pack, now)?;

            // Auto-compute per-floor desk capacity. Each floor uses its
            // own layout seed, so different variants may have different
            // desk counts. fetch_max ensures capacity only grows (monotone)
            // to prevent orphaning agents already assigned to higher desks.
            // If the user set a desk cap (config or CLI), each floor is
            // clamped to that value.
            if let Some(layout) = renderer.cached_layout() {
                use pixtuoid_core::layout::{SceneLayout, MAX_VISIBLE_DESKS};
                use pixtuoid_core::state::MAX_FLOORS;
                let buf_w = layout.buf_w;
                let buf_h = layout.buf_h;
                for floor_idx in 0..MAX_FLOORS {
                    let seed =
                        (floor_idx as u64).wrapping_mul(crate::tui::floor::FLOOR_SEED_MULTIPLIER);
                    let mut capacity =
                        SceneLayout::compute_with_seed(buf_w, buf_h, MAX_VISIBLE_DESKS, seed)
                            .map(|l| l.home_desks.len())
                            .unwrap_or(MAX_VISIBLE_DESKS);
                    if let Some(cap) = desk_cap {
                        capacity = capacity.min(cap);
                    }
                    if capacity > 0 {
                        floor_caps[floor_idx]
                            .fetch_max(capacity, std::sync::atomic::Ordering::Relaxed);
                    }
                }
            }

            let start = Instant::now();
            let mut polled = event::poll(tick)?;
            let mut quit = false;
            while polled {
                match event::read()? {
                    Event::Key(k) => {
                        if let Some(idx) = theme_picker.as_mut() {
                            match k.code {
                                KeyCode::Up | KeyCode::Char('k') => {
                                    if *idx > 0 {
                                        *idx -= 1;
                                    }
                                    renderer.set_theme(theme::ALL_THEMES[*idx]);
                                }
                                KeyCode::Down | KeyCode::Char('j') => {
                                    if *idx + 1 < theme::ALL_THEMES.len() {
                                        *idx += 1;
                                    }
                                    renderer.set_theme(theme::ALL_THEMES[*idx]);
                                }
                                KeyCode::Enter => {
                                    let chosen = *idx;
                                    saved_theme_idx = chosen;
                                    theme_picker = None;
                                    let name = theme::ALL_THEMES[chosen].name;
                                    if let Err(e) = crate::config::save(&config_path, name) {
                                        tracing::warn!("failed to persist theme: {e}");
                                    }
                                }
                                KeyCode::Esc => {
                                    renderer.set_theme(theme::ALL_THEMES[saved_theme_idx]);
                                    theme_picker = None;
                                }
                                _ => {}
                            }
                        } else {
                            match (k.code, k.modifiers) {
                                (KeyCode::Char('q'), _)
                                | (KeyCode::Esc, _)
                                | (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                                    quit = true;
                                }
                                (KeyCode::Char('p'), _) => {
                                    paused = !paused;
                                }
                                (KeyCode::Char('t'), _) => {
                                    theme_picker = Some(saved_theme_idx);
                                }
                                (KeyCode::PageUp, _)
                                | (KeyCode::Up, _)
                                | (KeyCode::Char('k'), _) => {
                                    let n_floors = crate::tui::floor::num_floors(&snapshot);
                                    let cur = renderer.current_floor();
                                    if cur + 1 < n_floors && renderer.transition().is_none() {
                                        renderer.navigate_floor(cur + 1, now);
                                    }
                                }
                                (KeyCode::PageDown, _)
                                | (KeyCode::Down, _)
                                | (KeyCode::Char('j'), _) => {
                                    let cur = renderer.current_floor();
                                    if cur > 0 && renderer.transition().is_none() {
                                        renderer.navigate_floor(cur - 1, now);
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    Event::Mouse(m) => match m.kind {
                        MouseEventKind::Moved | MouseEventKind::Drag(_) => {
                            renderer.set_mouse_pos(Some((m.column, m.row)));
                        }
                        MouseEventKind::Down(MouseButton::Left) => {
                            renderer.set_mouse_pos(Some((m.column, m.row)));
                            if m.row <= 1 && m.column >= 1 && m.column < 31 {
                                let _ = open::that("https://github.com/IvanWng97/pixtuoid");
                            } else if renderer.cached_layout().is_some_and(|layout| {
                                renderer::hit_test_coffee_machine(layout, m.column, m.row)
                            }) {
                                let _ = open::that("https://buymeacoffee.com/IvanWng97");
                            } else if let Some((pet_pos, anim, kind)) = renderer.cached_pet_pos() {
                                if renderer
                                    .active_pet_ref()
                                    .map_or(true, |p| !p.is_active(now))
                                    && renderer::hit_test_pet(kind, pet_pos, anim, m.column, m.row)
                                {
                                    renderer.set_active_pet(Some(renderer::PetState {
                                        petted_at: now,
                                        pet_pos,
                                        kind,
                                        floor_idx: renderer.current_floor(),
                                    }));
                                } else {
                                    let pinned = renderer.pinned_agent();
                                    if pinned.is_some() {
                                        renderer.set_pinned_agent(None);
                                    } else {
                                        let snap = scene_rx.borrow().clone();
                                        let hit = renderer.cached_layout().and_then(|layout| {
                                            renderer::hit_test_from_tui(
                                                &snap, layout, m.column, m.row,
                                            )
                                        });
                                        renderer.set_pinned_agent(hit);
                                    }
                                }
                            } else {
                                let pinned = renderer.pinned_agent();
                                if pinned.is_some() {
                                    renderer.set_pinned_agent(None);
                                } else {
                                    let snap = scene_rx.borrow().clone();
                                    let hit = renderer.cached_layout().and_then(|layout| {
                                        renderer::hit_test_from_tui(&snap, layout, m.column, m.row)
                                    });
                                    renderer.set_pinned_agent(hit);
                                }
                            }
                        }
                        _ => {}
                    },
                    _ => {}
                }
                polled = event::poll(Duration::from_millis(0))?;
            }
            if quit {
                if theme_picker.is_some() {
                    renderer.set_theme(theme::ALL_THEMES[saved_theme_idx]);
                }
                break;
            }
            let elapsed = start.elapsed();
            if let Some(rem) = tick.checked_sub(elapsed) {
                tokio::time::sleep(rem).await;
            }
            tokio::task::yield_now().await;
        }
        Ok(())
    })
    .await;

    teardown_terminal(&mut renderer.terminal)?;
    result
}
