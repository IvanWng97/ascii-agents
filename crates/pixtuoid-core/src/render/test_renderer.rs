use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use anyhow::Result;

use crate::render::Renderer;
use crate::sprite::format::Pack;
use crate::state::SceneState;

/// Captures every SceneState handed to it. Used in e2e tests.
#[derive(Clone, Default)]
pub struct TestRenderer {
    pub snapshots: Arc<Mutex<Vec<SceneState>>>,
}

impl TestRenderer {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn count(&self) -> usize {
        self.snapshots.lock().unwrap().len()
    }
    /// Direct snapshot capture — avoids the test having to construct a
    /// dummy `Pack` just to satisfy the `Renderer` trait. Tests that want
    /// to assert on the full trait signature can still use
    /// `<Self as Renderer>::render`.
    pub fn record(&mut self, scene: &SceneState) {
        self.snapshots.lock().unwrap().push(scene.clone());
    }
}

impl Renderer for TestRenderer {
    fn render(&mut self, scene: &SceneState, _pack: &Pack, _now: SystemTime) -> Result<()> {
        self.snapshots.lock().unwrap().push(scene.clone());
        Ok(())
    }
}
