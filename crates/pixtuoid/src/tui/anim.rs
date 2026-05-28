//! Stateless easing curves for animations.
//!
//! `Easing::apply` maps a normalized `t ∈ [0.0, 1.0]` through a chosen curve.
//! `eased_progress` (added in Task 2) is the convenience wrapper that takes a
//! wall-clock `started_at` + `duration_ms` and returns the eased progress.
//!
//! SystemTime: matches existing animation state (FloorTransition,
//! LightingState, PoseHistory) for v2 daemon-split compatibility.
//! See CLAUDE.md "Known sharp edges".

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Easing {
    Linear,
    EaseOutCubic,
    EaseInOutCubic,
    EaseOutExpo,
    EaseInQuad,
}

impl Easing {
    /// Apply the easing curve to a normalized `t ∈ [0.0, 1.0]`.
    /// Inputs outside that range are clamped.
    pub fn apply(self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Easing::Linear => t,
            Easing::EaseOutCubic => 1.0 - (1.0 - t).powi(3),
            Easing::EaseInOutCubic => {
                if t < 0.5 {
                    4.0 * t.powi(3)
                } else {
                    1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
                }
            }
            Easing::EaseOutExpo => {
                if t >= 1.0 {
                    1.0
                } else {
                    1.0 - 2.0_f32.powf(-10.0 * t)
                }
            }
            Easing::EaseInQuad => t * t,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx_eq(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn linear_endpoints() {
        assert!(approx_eq(Easing::Linear.apply(0.0), 0.0));
        assert!(approx_eq(Easing::Linear.apply(1.0), 1.0));
        assert!(approx_eq(Easing::Linear.apply(0.5), 0.5));
    }

    #[test]
    fn ease_out_cubic_endpoints() {
        assert!(approx_eq(Easing::EaseOutCubic.apply(0.0), 0.0));
        assert!(approx_eq(Easing::EaseOutCubic.apply(1.0), 1.0));
        // Should overshoot midpoint (fast start, slow end)
        assert!(Easing::EaseOutCubic.apply(0.5) > 0.5);
    }

    #[test]
    fn ease_in_out_cubic_endpoints() {
        assert!(approx_eq(Easing::EaseInOutCubic.apply(0.0), 0.0));
        assert!(approx_eq(Easing::EaseInOutCubic.apply(1.0), 1.0));
        assert!(approx_eq(Easing::EaseInOutCubic.apply(0.5), 0.5));
    }

    #[test]
    fn ease_out_expo_endpoints() {
        assert!(approx_eq(Easing::EaseOutExpo.apply(0.0), 0.0));
        assert!(approx_eq(Easing::EaseOutExpo.apply(1.0), 1.0));
        assert!(Easing::EaseOutExpo.apply(0.5) > 0.9);
    }

    #[test]
    fn ease_in_quad_endpoints() {
        assert!(approx_eq(Easing::EaseInQuad.apply(0.0), 0.0));
        assert!(approx_eq(Easing::EaseInQuad.apply(1.0), 1.0));
        assert!(approx_eq(Easing::EaseInQuad.apply(0.5), 0.25));
    }

    #[test]
    fn all_curves_are_monotone_non_decreasing() {
        for curve in [
            Easing::Linear,
            Easing::EaseOutCubic,
            Easing::EaseInOutCubic,
            Easing::EaseOutExpo,
            Easing::EaseInQuad,
        ] {
            let mut prev = -1.0_f32;
            for i in 0..=100 {
                let t = i as f32 / 100.0;
                let v = curve.apply(t);
                assert!(v >= prev, "{:?} not monotone at t={t}: {v} < {prev}", curve);
                prev = v;
            }
        }
    }

    #[test]
    fn out_of_range_inputs_clamp() {
        assert!(approx_eq(Easing::Linear.apply(-1.0), 0.0));
        assert!(approx_eq(Easing::Linear.apply(2.0), 1.0));
        assert!(approx_eq(Easing::EaseOutCubic.apply(-0.5), 0.0));
        assert!(approx_eq(Easing::EaseInOutCubic.apply(1.5), 1.0));
    }
}
