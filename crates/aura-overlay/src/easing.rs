/// Organic easing functions for Aura's animations.
pub struct AuraEasing;

impl AuraEasing {
    pub fn breathe(t: f32) -> f32 {
        (1.0 - (t * std::f32::consts::TAU).cos()) / 2.0
    }

    pub fn drift(t: f32) -> f32 {
        debug_assert!((0.0..=1.0).contains(&t), "t must be in [0.0, 1.0], got {t}");
        1.0 - (1.0 - t).powi(4)
    }

    pub fn materialize(t: f32) -> f32 {
        debug_assert!((0.0..=1.0).contains(&t), "t must be in [0.0, 1.0], got {t}");
        if t < 0.5 {
            4.0 * t * t * t
        } else {
            1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
        }
    }

    pub fn dissolve(t: f32) -> f32 {
        debug_assert!((0.0..=1.0).contains(&t), "t must be in [0.0, 1.0], got {t}");
        (1.0 - t).powi(3)
    }

    pub fn pulse(t: f32) -> f32 {
        debug_assert!((0.0..=1.0).contains(&t), "t must be in [0.0, 1.0], got {t}");
        (-5.0 * t).exp()
    }
}
