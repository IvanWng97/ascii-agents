pub mod embedded_pack;
pub mod frame_cache;
pub mod layout;
pub mod pathfind;
pub mod pixel_painter;
pub mod pose;
pub mod renderer;
pub mod theme;
pub mod tui_renderer;

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use anyhow::Result;
use ascii_agents_core::Renderer;
use crossterm::event::{self, Event, KeyCode, KeyModifiers, MouseButton, MouseEventKind};

use renderer::{setup_terminal, teardown_terminal};
use tui_renderer::TuiRenderer;

use crate::runtime::SceneRx;

pub async fn run_tui(
    mut scene_rx: SceneRx,
    pack_dir: Option<std::path::PathBuf>,
    max_desks: Arc<std::sync::atomic::AtomicUsize>,
    _theme: &'static theme::Theme,
) -> Result<()> {
    let pack = embedded_pack::load_sprite_pack(pack_dir)?;
    let term = setup_terminal()?;
    let mut renderer = TuiRenderer::new(term);
    let mut last_layout_sig: Option<(u16, u16, usize)> = None;
    let mut paused = false;
    let mut frozen_now: Option<SystemTime> = None;

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
            let snapshot = {
                let mut s = (*snapshot).clone();
                s.max_desks = max_desks.load(std::sync::atomic::Ordering::Relaxed);
                Arc::new(s)
            };
            renderer.evict_missing(&snapshot);
            let sig = (
                renderer.buf().width,
                renderer.buf().height,
                snapshot.max_desks,
            );
            if last_layout_sig != Some(sig) {
                renderer.invalidate_routes();
                last_layout_sig = Some(sig);
            }
            renderer.render(&snapshot, &pack, now)?;

            let start = Instant::now();
            let mut polled = event::poll(tick)?;
            let mut quit = false;
            while polled {
                match event::read()? {
                    Event::Key(k) => match (k.code, k.modifiers) {
                        (KeyCode::Char('q'), _)
                        | (KeyCode::Esc, _)
                        | (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                            quit = true;
                        }
                        (KeyCode::Char('p'), _) => {
                            paused = !paused;
                        }
                        (KeyCode::Char('+') | KeyCode::Char('='), _) => {
                            let cur = max_desks.load(std::sync::atomic::Ordering::Relaxed);
                            if cur < 16 {
                                max_desks.store(cur + 1, std::sync::atomic::Ordering::Relaxed);
                            }
                        }
                        (KeyCode::Char('-'), _) => {
                            let cur = max_desks.load(std::sync::atomic::Ordering::Relaxed);
                            if cur > 1 {
                                max_desks.store(cur - 1, std::sync::atomic::Ordering::Relaxed);
                            }
                        }
                        _ => {}
                    },
                    Event::Mouse(m) => match m.kind {
                        MouseEventKind::Moved | MouseEventKind::Drag(_) => {
                            renderer.set_mouse_pos(Some((m.column, m.row)));
                        }
                        MouseEventKind::Down(MouseButton::Left) => {
                            renderer.set_mouse_pos(Some((m.column, m.row)));
                            if m.row <= 1 && m.column >= 1 && m.column < 31 {
                                let _ = open::that("https://github.com/IvanWng97/ascii-agents");
                            } else {
                                let pinned = renderer.pinned_agent();
                                if pinned.is_some() {
                                    renderer.set_pinned_agent(None);
                                } else {
                                    let snap = scene_rx.borrow().clone();
                                    let hit = renderer::hit_test_from_tui(
                                        &snap,
                                        snap.max_desks,
                                        m.column,
                                        m.row,
                                        renderer.buf(),
                                    );
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
