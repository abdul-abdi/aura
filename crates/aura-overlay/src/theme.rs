/// Aura color system: bioluminescent depth aesthetic.
/// All colors are (r, g, b) or (r, g, b, a) as f32 0.0..1.0.
/// Design system — not all constants are used yet.
#[allow(dead_code)]
pub struct AuraColors;

impl AuraColors {
    pub const VOID: (f32, f32, f32) = (0.039, 0.055, 0.090);
    pub const GLASS: (f32, f32, f32, f32) = (0.059, 0.086, 0.157, 0.70);
    pub const GLASS_EDGE: (f32, f32, f32, f32) = (0.12, 0.16, 0.25, 0.30);
    pub const GLOW_CYAN: (f32, f32, f32) = (0.302, 0.910, 0.820);
    pub const GLOW_CYAN_DIM: (f32, f32, f32, f32) = (0.302, 0.910, 0.820, 0.15);
    pub const GLOW_AMBER: (f32, f32, f32) = (1.0, 0.702, 0.278);
    pub const GLOW_VIOLET: (f32, f32, f32) = (0.545, 0.361, 0.965);
    pub const GLOW_SUCCESS: (f32, f32, f32) = (0.302, 0.910, 0.490);
    pub const TEXT_PRIMARY: (f32, f32, f32) = (0.910, 0.894, 0.875);
    pub const TEXT_DIM: (f32, f32, f32) = (0.420, 0.447, 0.502);
}

#[allow(dead_code)]
pub struct AuraTypography;

impl AuraTypography {
    pub const FACE_PRIMARY: &str = "SF Pro Display";
    pub const FACE_MONO: &str = "SF Mono";
    pub const SIZE_RESPONSE: f32 = 17.0;
    pub const SIZE_LABEL: f32 = 13.0;
    pub const SIZE_WHISPER: f32 = 11.0;
    pub const WEIGHT_LIGHT: i32 = 300;
    pub const WEIGHT_MEDIUM: i32 = 500;
    pub const WEIGHT_REGULAR: i32 = 400;
    pub const TRACKING_RESPONSE: f32 = 0.4;
    pub const TRACKING_LABEL: f32 = 1.2;
}

#[allow(dead_code)]
pub struct AuraTiming;

impl AuraTiming {
    pub const BREATHE_CYCLE_SECS: f32 = 8.0;
    pub const IDLE_TO_LISTENING_MS: u64 = 400;
    pub const LISTENING_TO_PROCESSING_MS: u64 = 300;
    pub const PROCESSING_TO_RESPONSE_MS: u64 = 500;
    pub const CHAR_REVEAL_MS: u64 = 30;
    pub const CHAR_GLOW_DECAY_MS: u64 = 200;
    pub const RESPONSE_HOLD_SECS: f32 = 3.0;
    pub const DISSOLVE_MS: u64 = 600;
    pub const ERROR_PULSE_SECS: f32 = 2.0;
    pub const DISMISS_MS: u64 = 400;
}
