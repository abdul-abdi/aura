/// Bioluminescent overlay renderer — draws all overlay states via Skia.
use std::f32::consts::PI;

use skia_safe::{
    BlurStyle, Canvas, Color4f, Font, FontMgr, FontStyle, MaskFilter, Paint, Path, Point, RRect,
    Rect, Shader, TileMode,
    font_style::{Slant, Weight, Width},
    paint,
};

use crate::easing::AuraEasing;
use crate::particles::ParticleSystem;
use crate::theme::{AuraColors, AuraTiming, AuraTypography};

// ---------------------------------------------------------------------------
// Overlay state
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum OverlayState {
    Idle {
        breath_phase: f32,
    },
    Listening {
        audio_levels: Vec<f32>,
        phase: f32,
        transition: f32,
    },
    Processing {
        phase: f32,
        transition: f32,
    },
    Response {
        text: String,
        chars_revealed: usize,
        card_opacity: f32,
    },
    Error {
        message: String,
        card_opacity: f32,
        pulse_phase: f32,
    },
    Dissolving {
        previous_state: Box<OverlayState>,
        progress: f32,
        focal_x: f32,
        focal_y: f32,
    },
}

// ---------------------------------------------------------------------------
// Renderer
// ---------------------------------------------------------------------------

pub struct OverlayRenderer {
    width: f32,
    height: f32,
    particles: ParticleSystem,
    font_mgr: FontMgr,
    cached_font: Font,
}

impl OverlayRenderer {
    const MAX_PARTICLES: usize = 256;

    pub fn new(width: f32, height: f32) -> Self {
        let font_mgr = FontMgr::default();
        let cached_font = Self::resolve_font(&font_mgr, AuraTypography::SIZE_RESPONSE);
        Self {
            width,
            height,
            particles: ParticleSystem::new(Self::MAX_PARTICLES),
            font_mgr,
            cached_font,
        }
    }

    fn resolve_font(font_mgr: &FontMgr, size: f32) -> Font {
        let style = FontStyle::new(
            Weight::from(AuraTypography::WEIGHT_LIGHT),
            Width::NORMAL,
            Slant::Upright,
        );
        if let Some(typeface) = font_mgr.match_family_style(AuraTypography::FACE_PRIMARY, style) {
            Font::new(typeface, size)
        } else {
            let mut font = Font::default();
            font.set_size(size);
            font
        }
    }

    pub fn resize(&mut self, width: f32, height: f32) {
        self.width = width;
        self.height = height;
    }

    pub fn particles_mut(&mut self) -> &mut ParticleSystem {
        &mut self.particles
    }

    // ------------------------------------------------------------------
    // Main dispatch
    // ------------------------------------------------------------------

    pub fn render(&mut self, canvas: &Canvas, state: &OverlayState, dt: f32) {
        self.particles.update(dt);

        match state {
            OverlayState::Idle { breath_phase } => {
                self.draw_idle_edge(canvas, *breath_phase);
            }
            OverlayState::Listening {
                audio_levels,
                phase,
                transition,
            } => {
                self.draw_listening_waveform(canvas, audio_levels, *phase, *transition);
            }
            OverlayState::Processing { phase, transition } => {
                self.draw_processing_orb(canvas, *phase, *transition);
            }
            OverlayState::Response {
                text,
                chars_revealed,
                card_opacity,
            } => {
                self.draw_response_card(canvas, text, *chars_revealed, *card_opacity);
            }
            OverlayState::Error {
                message,
                card_opacity,
                pulse_phase,
            } => {
                self.draw_error_card(canvas, message, *card_opacity, *pulse_phase);
            }
            OverlayState::Dissolving {
                previous_state,
                progress,
                focal_x,
                focal_y,
            } => {
                // Guard against nested Dissolving — don't recurse infinitely
                if matches!(previous_state.as_ref(), OverlayState::Dissolving { .. }) {
                    return;
                }

                let scale = AuraEasing::dissolve(*progress);
                let alpha = scale;
                canvas.save();
                canvas.translate((*focal_x * (1.0 - scale), *focal_y * (1.0 - scale)));
                canvas.scale((scale, scale));

                // Re-render the previous state at shrinking scale (borrow, no clone)
                self.render_inner(canvas, previous_state.as_ref());

                canvas.restore();

                // Fade out via a transparent overlay
                if alpha < 1.0 {
                    let mut fade = Paint::default();
                    fade.set_color4f(Color4f::new(0.0, 0.0, 0.0, 1.0 - alpha), None);
                    fade.set_anti_alias(true);
                    canvas.draw_rect(Rect::from_xywh(0.0, 0.0, self.width, self.height), &fade);
                }
            }
        }

        self.draw_particles(canvas);
    }

    /// Render a state without updating particles or drawing them.
    /// Used by Dissolving to render the previous state without cloning.
    fn render_inner(&self, canvas: &Canvas, state: &OverlayState) {
        match state {
            OverlayState::Idle { breath_phase } => {
                self.draw_idle_edge(canvas, *breath_phase);
            }
            OverlayState::Listening {
                audio_levels,
                phase,
                transition,
            } => {
                // Use immutable draw (no particle spawn) for dissolving snapshots
                self.draw_listening_waveform_static(canvas, audio_levels, *phase, *transition);
            }
            OverlayState::Processing { phase, transition } => {
                self.draw_processing_orb(canvas, *phase, *transition);
            }
            OverlayState::Response {
                text,
                chars_revealed,
                card_opacity,
            } => {
                self.draw_response_card(canvas, text, *chars_revealed, *card_opacity);
            }
            OverlayState::Error {
                message,
                card_opacity,
                pulse_phase,
            } => {
                self.draw_error_card(canvas, message, *card_opacity, *pulse_phase);
            }
            OverlayState::Dissolving { .. } => {
                // Nested dissolving is guarded in render() — should not reach here
            }
        }
    }

    // ------------------------------------------------------------------
    // Idle: breathing hair-thin cyan line at screen bottom
    // ------------------------------------------------------------------

    fn draw_idle_edge(&self, canvas: &Canvas, breath_phase: f32) {
        let breath = AuraEasing::breathe(breath_phase / AuraTiming::BREATHE_CYCLE_SECS);
        let line_width = self.width * (0.15 + 0.10 * breath);
        let center_x = self.width / 2.0;
        let y = self.height - 2.0;

        let left = center_x - line_width / 2.0;
        let right = center_x + line_width / 2.0;

        let (cr, cg, cb) = AuraColors::GLOW_CYAN;

        // Glow behind the line
        let mut glow_paint = Paint::default();
        glow_paint.set_anti_alias(true);
        glow_paint.set_style(paint::Style::Stroke);
        glow_paint.set_stroke_width(6.0);
        glow_paint.set_color4f(Color4f::new(cr, cg, cb, 0.15 + 0.10 * breath), None);
        glow_paint.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 8.0, None));

        let gradient = Shader::linear_gradient(
            (Point::new(left, y), Point::new(right, y)),
            &[
                Color4f::new(cr, cg, cb, 0.0),
                Color4f::new(cr, cg, cb, 0.6 + 0.3 * breath),
                Color4f::new(cr, cg, cb, 0.6 + 0.3 * breath),
                Color4f::new(cr, cg, cb, 0.0),
            ][..],
            Some(&[0.0_f32, 0.2, 0.8, 1.0][..]),
            TileMode::Clamp,
            None,
            None,
        );

        // Hair-thin line
        let mut line_paint = Paint::default();
        line_paint.set_anti_alias(true);
        line_paint.set_style(paint::Style::Stroke);
        line_paint.set_stroke_width(1.0);
        if let Some(shader) = gradient {
            glow_paint.set_shader(shader.clone());
            line_paint.set_shader(shader);
        }

        canvas.draw_line(Point::new(left, y), Point::new(right, y), &glow_paint);
        canvas.draw_line(Point::new(left, y), Point::new(right, y), &line_paint);
    }

    // ------------------------------------------------------------------
    // Listening: organic multi-layer waveform + particles
    // ------------------------------------------------------------------

    /// Build the waveform path and return (path, peak_x, peak_y).
    fn build_wave_path(
        &self,
        audio_levels: &[f32],
        phase: f32,
        transition: f32,
    ) -> (Path, f32, f32) {
        let num_points: usize = 120;
        let base_y = self.height * 0.70;
        let max_amplitude = 60.0 * transition;

        let mut path = Path::new();
        let mut first = true;
        let mut peak_x = 0.0_f32;
        let mut peak_y = base_y;

        for i in 0..num_points {
            let t = i as f32 / (num_points - 1) as f32;
            let x = t * self.width;

            let edge_fade = (t * PI).sin().sqrt();

            let audio_idx = t * (audio_levels.len().max(1) - 1) as f32;
            let ai = audio_idx.floor() as usize;
            let frac = audio_idx - ai as f32;
            let level = if audio_levels.is_empty() {
                0.3
            } else {
                let a = audio_levels[ai.min(audio_levels.len() - 1)];
                let b = audio_levels[(ai + 1).min(audio_levels.len() - 1)];
                a + (b - a) * frac
            };

            let w1 = (t * 4.0 * PI + phase).sin();
            let w2 = (t * 7.0 * PI + phase * 1.3).sin() * 0.5;
            let w3 = (t * 13.0 * PI + phase * 0.7).sin() * 0.25;
            let wave = (w1 + w2 + w3) * edge_fade * level;

            let y = base_y - wave * max_amplitude;

            if first {
                path.move_to(Point::new(x, y));
                first = false;
            } else {
                path.line_to(Point::new(x, y));
            }

            if y < peak_y {
                peak_y = y;
                peak_x = x;
            }
        }

        (path, peak_x, peak_y)
    }

    /// Draw waveform strokes and fill from a pre-built path.
    fn draw_wave_strokes(&self, canvas: &Canvas, path: &Path, transition: f32) {
        let base_y = self.height * 0.70;
        let max_amplitude = 60.0 * transition;
        let (cr, cg, cb) = AuraColors::GLOW_CYAN;

        // Gradient fill: close path to screen bottom
        let mut fill_path = path.clone();
        fill_path.line_to(Point::new(self.width, self.height));
        fill_path.line_to(Point::new(0.0, self.height));
        fill_path.close();

        let fill_gradient = Shader::linear_gradient(
            (
                Point::new(self.width / 2.0, base_y - max_amplitude),
                Point::new(self.width / 2.0, self.height),
            ),
            &[
                Color4f::new(cr, cg, cb, 0.15 * transition),
                Color4f::new(cr, cg, cb, 0.0),
            ][..],
            None::<&[f32]>,
            TileMode::Clamp,
            None,
            None,
        );

        let mut fill_paint = Paint::default();
        fill_paint.set_anti_alias(true);
        fill_paint.set_style(paint::Style::Fill);
        if let Some(shader) = fill_gradient {
            fill_paint.set_shader(shader);
        }
        canvas.draw_path(&fill_path, &fill_paint);

        // Glowing stroke
        let mut glow = Paint::default();
        glow.set_anti_alias(true);
        glow.set_style(paint::Style::Stroke);
        glow.set_stroke_width(4.0);
        glow.set_color4f(Color4f::new(cr, cg, cb, 0.25 * transition), None);
        glow.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 6.0, None));
        canvas.draw_path(path, &glow);

        // Crisp stroke
        let mut stroke = Paint::default();
        stroke.set_anti_alias(true);
        stroke.set_style(paint::Style::Stroke);
        stroke.set_stroke_width(1.5);
        stroke.set_color4f(Color4f::new(cr, cg, cb, 0.8 * transition), None);
        canvas.draw_path(path, &stroke);
    }

    fn draw_listening_waveform(
        &mut self,
        canvas: &Canvas,
        audio_levels: &[f32],
        phase: f32,
        transition: f32,
    ) {
        let (path, peak_x, peak_y) = self.build_wave_path(audio_levels, phase, transition);
        self.draw_wave_strokes(canvas, &path, transition);

        // Spawn particles at wave peaks
        if transition > 0.5 {
            self.particles.spawn(peak_x, peak_y, transition);
        }
    }

    /// Static version for dissolving — no particle spawn, takes &self.
    fn draw_listening_waveform_static(
        &self,
        canvas: &Canvas,
        audio_levels: &[f32],
        phase: f32,
        transition: f32,
    ) {
        let (path, _, _) = self.build_wave_path(audio_levels, phase, transition);
        self.draw_wave_strokes(canvas, &path, transition);
    }

    // ------------------------------------------------------------------
    // Processing: amber/cyan radial gradient orb with orbital rings
    // ------------------------------------------------------------------

    fn draw_processing_orb(&self, canvas: &Canvas, phase: f32, transition: f32) {
        let center_x = self.width / 2.0;
        let center_y = self.height * 0.70;
        let base_radius = 30.0;
        let breath = AuraEasing::breathe(phase / 3.0);
        let radius = base_radius * (0.9 + 0.2 * breath) * transition;

        let (ar, ag, ab) = AuraColors::GLOW_AMBER;
        let (cr, cg, cb) = AuraColors::GLOW_CYAN;

        // Core radial gradient
        let core_gradient = Shader::radial_gradient(
            Point::new(center_x, center_y),
            radius,
            &[
                Color4f::new(ar, ag, ab, 0.9 * transition),
                Color4f::new(cr, cg, cb, 0.3 * transition),
                Color4f::new(cr, cg, cb, 0.0),
            ][..],
            Some(&[0.0_f32, 0.5, 1.0][..]),
            TileMode::Clamp,
            None,
            None,
        );

        let mut core_paint = Paint::default();
        core_paint.set_anti_alias(true);
        if let Some(shader) = core_gradient {
            core_paint.set_shader(shader);
        }
        canvas.draw_circle(Point::new(center_x, center_y), radius, &core_paint);

        // Outer glow
        let mut glow = Paint::default();
        glow.set_anti_alias(true);
        glow.set_color4f(Color4f::new(ar, ag, ab, 0.15 * transition), None);
        glow.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, radius * 0.8, None));
        canvas.draw_circle(Point::new(center_x, center_y), radius * 0.5, &glow);

        // Three orbital rings with traveling dots
        for i in 0..3 {
            let ring_phase = phase + i as f32 * (2.0 * PI / 3.0);
            let ring_radius = radius * (1.4 + i as f32 * 0.35);
            let ellipse_ratio = 0.4 + i as f32 * 0.1;

            // Ring stroke
            let mut ring_paint = Paint::default();
            ring_paint.set_anti_alias(true);
            ring_paint.set_style(paint::Style::Stroke);
            ring_paint.set_stroke_width(0.8);
            ring_paint.set_color4f(Color4f::new(cr, cg, cb, 0.2 * transition), None);

            let ring_rect = Rect::from_xywh(
                center_x - ring_radius,
                center_y - ring_radius * ellipse_ratio,
                ring_radius * 2.0,
                ring_radius * ellipse_ratio * 2.0,
            );
            let rrect = RRect::new_rect(ring_rect);
            canvas.draw_rrect(rrect, &ring_paint);

            // Traveling dot
            let dot_angle = ring_phase * (1.0 + i as f32 * 0.2);
            let dot_x = center_x + ring_radius * dot_angle.cos();
            let dot_y = center_y + ring_radius * ellipse_ratio * dot_angle.sin();

            let mut dot_paint = Paint::default();
            dot_paint.set_anti_alias(true);
            dot_paint.set_color4f(Color4f::new(cr, cg, cb, 0.8 * transition), None);
            canvas.draw_circle(Point::new(dot_x, dot_y), 2.5, &dot_paint);

            // Dot glow
            let mut dot_glow = Paint::default();
            dot_glow.set_anti_alias(true);
            dot_glow.set_color4f(Color4f::new(cr, cg, cb, 0.3 * transition), None);
            dot_glow.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 4.0, None));
            canvas.draw_circle(Point::new(dot_x, dot_y), 3.0, &dot_glow);
        }
    }

    // ------------------------------------------------------------------
    // Response: dark glass card with character-by-character text reveal
    // ------------------------------------------------------------------

    fn draw_response_card(
        &self,
        canvas: &Canvas,
        text: &str,
        chars_revealed: usize,
        card_opacity: f32,
    ) {
        let max_width = 420.0_f32;
        let padding = 24.0_f32;
        let corner_radius = 16.0_f32;
        let center_x = self.width / 2.0;
        let card_y = self.height * 0.35;

        let (cr, cg, cb) = AuraColors::GLOW_CYAN;
        let (tr, tg, tb) = AuraColors::TEXT_PRIMARY;
        let (gr, gg, gb, ga) = AuraColors::GLASS;

        // Card background
        let card_width = max_width.min(self.width - 40.0);
        let card_height = 160.0; // fixed height for now
        let card_rect =
            Rect::from_xywh(center_x - card_width / 2.0, card_y, card_width, card_height);
        let card_rrect = RRect::new_rect_xy(card_rect, corner_radius, corner_radius);

        // Glass fill
        let mut glass_paint = Paint::default();
        glass_paint.set_anti_alias(true);
        glass_paint.set_color4f(Color4f::new(gr, gg, gb, ga * card_opacity), None);
        canvas.draw_rrect(card_rrect, &glass_paint);

        // Luminous border
        let mut border_paint = Paint::default();
        border_paint.set_anti_alias(true);
        border_paint.set_style(paint::Style::Stroke);
        border_paint.set_stroke_width(1.0);
        border_paint.set_color4f(Color4f::new(cr, cg, cb, 0.3 * card_opacity), None);
        canvas.draw_rrect(card_rrect, &border_paint);

        // Outer glow on border
        let mut border_glow = Paint::default();
        border_glow.set_anti_alias(true);
        border_glow.set_style(paint::Style::Stroke);
        border_glow.set_stroke_width(2.0);
        border_glow.set_color4f(Color4f::new(cr, cg, cb, 0.1 * card_opacity), None);
        border_glow.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 4.0, None));
        canvas.draw_rrect(card_rrect, &border_glow);

        // Text rendering
        let font = self.make_font(AuraTypography::SIZE_RESPONSE);

        let revealed: String = text.chars().take(chars_revealed).collect();
        let text_x = center_x - card_width / 2.0 + padding;
        let text_y = card_y + padding + AuraTypography::SIZE_RESPONSE;

        // Draw revealed text
        let mut text_paint = Paint::default();
        text_paint.set_anti_alias(true);
        text_paint.set_color4f(Color4f::new(tr, tg, tb, card_opacity), None);
        canvas.draw_str(&revealed, Point::new(text_x, text_y), &font, &text_paint);

        // Glow on the latest revealed character
        if chars_revealed > 0 && chars_revealed <= text.chars().count() {
            let prefix: String = text.chars().take(chars_revealed - 1).collect();
            let (prefix_width, _) = font.measure_str(&prefix, None);
            let last_char: String = text.chars().nth(chars_revealed - 1).into_iter().collect();

            let mut glow_paint = Paint::default();
            glow_paint.set_anti_alias(true);
            glow_paint.set_color4f(Color4f::new(cr, cg, cb, 0.6 * card_opacity), None);
            glow_paint.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 3.0, None));
            canvas.draw_str(
                &last_char,
                Point::new(text_x + prefix_width, text_y),
                &font,
                &glow_paint,
            );
        }
    }

    // ------------------------------------------------------------------
    // Error: violet-tinted glass card with gentle pulse
    // ------------------------------------------------------------------

    fn draw_error_card(&self, canvas: &Canvas, message: &str, card_opacity: f32, pulse_phase: f32) {
        let max_width = 420.0_f32;
        let padding = 24.0_f32;
        let corner_radius = 16.0_f32;
        let center_x = self.width / 2.0;
        let card_y = self.height * 0.35;

        let (vr, vg, vb) = AuraColors::GLOW_VIOLET;
        let (tr, tg, tb) = AuraColors::TEXT_PRIMARY;
        let (gr, gg, gb, ga) = AuraColors::GLASS;

        let card_width = max_width.min(self.width - 40.0);
        let card_height = 120.0;
        let card_rect =
            Rect::from_xywh(center_x - card_width / 2.0, card_y, card_width, card_height);
        let card_rrect = RRect::new_rect_xy(card_rect, corner_radius, corner_radius);

        // Violet-tinted glass fill
        let mut glass_paint = Paint::default();
        glass_paint.set_anti_alias(true);
        glass_paint.set_color4f(
            Color4f::new(gr + 0.02, gg, gb + 0.02, ga * card_opacity),
            None,
        );
        canvas.draw_rrect(card_rrect, &glass_paint);

        // Pulsing violet border
        let pulse = AuraEasing::breathe(pulse_phase / AuraTiming::ERROR_PULSE_SECS);
        let border_alpha = (0.3 + 0.2 * pulse) * card_opacity;

        let mut border_paint = Paint::default();
        border_paint.set_anti_alias(true);
        border_paint.set_style(paint::Style::Stroke);
        border_paint.set_stroke_width(1.0);
        border_paint.set_color4f(Color4f::new(vr, vg, vb, border_alpha), None);
        canvas.draw_rrect(card_rrect, &border_paint);

        // Border glow
        let mut border_glow = Paint::default();
        border_glow.set_anti_alias(true);
        border_glow.set_style(paint::Style::Stroke);
        border_glow.set_stroke_width(2.0);
        border_glow.set_color4f(
            Color4f::new(vr, vg, vb, 0.1 * card_opacity * (1.0 + pulse)),
            None,
        );
        border_glow.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 4.0, None));
        canvas.draw_rrect(card_rrect, &border_glow);

        // Violet indicator dot
        let dot_x = center_x - card_width / 2.0 + padding;
        let dot_y = card_y + padding;

        let mut dot_paint = Paint::default();
        dot_paint.set_anti_alias(true);
        dot_paint.set_color4f(Color4f::new(vr, vg, vb, 0.8 * card_opacity), None);
        canvas.draw_circle(Point::new(dot_x, dot_y), 4.0, &dot_paint);

        let mut dot_glow = Paint::default();
        dot_glow.set_anti_alias(true);
        dot_glow.set_color4f(Color4f::new(vr, vg, vb, 0.3 * card_opacity), None);
        dot_glow.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 4.0, None));
        canvas.draw_circle(Point::new(dot_x, dot_y), 5.0, &dot_glow);

        // Error message text
        let font = self.make_font(AuraTypography::SIZE_RESPONSE);
        let text_x = dot_x + 16.0;
        let text_y = card_y + padding + AuraTypography::SIZE_RESPONSE * 0.3;

        let mut text_paint = Paint::default();
        text_paint.set_anti_alias(true);
        text_paint.set_color4f(Color4f::new(tr, tg, tb, card_opacity), None);
        canvas.draw_str(message, Point::new(text_x, text_y), &font, &text_paint);
    }

    // ------------------------------------------------------------------
    // Particles
    // ------------------------------------------------------------------

    fn draw_particles(&self, canvas: &Canvas) {
        let (cr, cg, cb) = AuraColors::GLOW_CYAN;

        for p in self.particles.particles() {
            let alpha = p.life.clamp(0.0, 1.0);
            let r = cr + p.hue_shift;
            let g = cg;
            let b = cb - p.hue_shift;

            // Glow
            let mut glow = Paint::default();
            glow.set_anti_alias(true);
            glow.set_color4f(Color4f::new(r, g, b, alpha * 0.3), None);
            glow.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, p.size * 1.5, None));
            canvas.draw_circle(Point::new(p.x, p.y), p.size, &glow);

            // Core
            let mut core = Paint::default();
            core.set_anti_alias(true);
            core.set_color4f(Color4f::new(r, g, b, alpha * 0.8), None);
            canvas.draw_circle(Point::new(p.x, p.y), p.size * 0.4, &core);
        }
    }

    // ------------------------------------------------------------------
    // Helpers
    // ------------------------------------------------------------------

    fn make_font(&self, size: f32) -> Font {
        if (size - AuraTypography::SIZE_RESPONSE).abs() < f32::EPSILON {
            return self.cached_font.clone();
        }
        Self::resolve_font(&self.font_mgr, size)
    }
}
