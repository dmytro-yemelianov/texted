//! Fixed reference canvases and the shared presentation/input transform. No window or GPU state.
use crate::appearance::Theme;
pub use crate::visuals::{CrtEffect, Phosphor, VisualEffects};
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Framebuffer {
    #[default]
    Auto,
    Native,
    Vga,
    Ega,
    AtariSt,
    DosText,
    C64,
    ZxSpectrum,
    MonoText,
    MacSystem2,
    GameBoy,
}
impl Framebuffer {
    pub const ALL: [Self; 11] = [
        Self::Auto,
        Self::Native,
        Self::Vga,
        Self::Ega,
        Self::AtariSt,
        Self::DosText,
        Self::C64,
        Self::ZxSpectrum,
        Self::MonoText,
        Self::MacSystem2,
        Self::GameBoy,
    ];
    pub fn step(self, delta: i32) -> Self {
        let i = Self::ALL.iter().position(|v| *v == self).unwrap() as i32;
        Self::ALL[(i + delta).rem_euclid(Self::ALL.len() as i32) as usize]
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Auto => "Auto (theme)",
            Self::Native => "Native",
            Self::Vga => "VGA 640x480",
            Self::Ega => "EGA 640x350",
            Self::AtariSt => "ST 320x200",
            Self::DosText => "DOS 720x400",
            Self::C64 => "C64 320x200",
            Self::ZxSpectrum => "ZX 256x192",
            Self::MonoText => "Mono 720x350",
            Self::MacSystem2 => "Mac 512x342",
            Self::GameBoy => "Game Boy 160x144",
        }
    }
    pub fn resolve(self, theme: Theme) -> Option<(u32, u32)> {
        match self {
            Self::Auto => match theme {
                Theme::ModernDark | Theme::ModernLight => None,
                Theme::Vga => Self::Vga.resolve(theme),
                Theme::Ega => Self::Ega.resolve(theme),
                Theme::Atari => Self::AtariSt.resolve(theme),
                Theme::Commander => Self::DosText.resolve(theme),
                Theme::C64 => Self::C64.resolve(theme),
                Theme::ZxSpectrum => Self::ZxSpectrum.resolve(theme),
                Theme::Amber | Theme::GreenCrt => Self::MonoText.resolve(theme),
                Theme::MacSystem2 => Self::MacSystem2.resolve(theme),
                Theme::GameBoy => Self::GameBoy.resolve(theme),
            },
            Self::Native => None,
            Self::Vga => Some((640, 480)),
            Self::Ega => Some((640, 350)),
            Self::AtariSt | Self::C64 => Some((320, 200)),
            Self::DosText => Some((720, 400)),
            Self::ZxSpectrum => Some((256, 192)),
            Self::MonoText => Some((720, 350)),
            Self::MacSystem2 => Some((512, 342)),
            Self::GameBoy => Some((160, 144)),
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScreenMapping {
    pub source: (u32, u32),
    pub destination: (u32, u32, u32, u32),
    pub pixelated: bool,
    pub effects: VisualEffects,
    /// Seconds for animated effects, and one font pixel in window pixels for their size.
    pub effect_time: f32,
    pub effect_pixel: f32,
    /// The theme's tube and background colour, which monochrome effects and matrix gaps use.
    pub phosphor: Phosphor,
    pub background: u32,
}
impl ScreenMapping {
    pub fn new(window: (u32, u32), resolution: Option<(u32, u32)>) -> Self {
        let (width, height) = (window.0.max(1), window.1.max(1));
        let Some((w, h)) = resolution else {
            return Self {
                source: (width, height),
                destination: (0, 0, width, height),
                pixelated: false,
                effects: VisualEffects::NEUTRAL,
                effect_time: 0.0,
                effect_pixel: 1.0,
                phosphor: Phosphor::Color,
                background: 0,
            };
        };
        // Fill one window dimension, preserving the source aspect to physical-pixel precision.
        // Integer cross-products avoid a floating-point roundoff leaving a gap on the fitted edge.
        let (dw, dh) = if u64::from(width) * u64::from(h) <= u64::from(height) * u64::from(w) {
            (
                width,
                (u64::from(width) * u64::from(h) / u64::from(w)).max(1) as u32,
            )
        } else {
            (
                (u64::from(height) * u64::from(w) / u64::from(h)).max(1) as u32,
                height,
            )
        };
        Self {
            source: (w, h),
            destination: ((width - dw) / 2, (height - dh) / 2, dw, dh),
            pixelated: true,
            effects: VisualEffects::NEUTRAL,
            effect_time: 0.0,
            effect_pixel: 1.0,
            phosphor: Phosphor::Color,
            background: 0,
        }
    }
    /// Effects for the window backend; native and limited-resolution screens both get them.
    pub fn with_effects(mut self, effects: VisualEffects, time: f32, pixel: f32) -> Self {
        self.effects = effects;
        self.effect_time = time.rem_euclid(crate::visuals::EFFECT_PERIOD);
        self.effect_pixel = pixel.max(1.0);
        self
    }
    /// The theme's phosphor and background for the effect pass.
    pub fn with_glass(mut self, phosphor: Phosphor, background: u32) -> Self {
        self.phosphor = phosphor;
        self.background = background;
        self
    }
    fn bent(self) -> bool {
        self.effects.curvature > 0.0
    }
    pub fn scale(self) -> (f32, f32) {
        (
            self.destination.2 as f32 / self.source.0 as f32,
            self.destination.3 as f32 / self.source.1 as f32,
        )
    }
    pub fn to_source(self, (x, y): (f32, f32)) -> Option<(f32, f32)> {
        let (dx, dy, w, h) = self.destination;
        if x < dx as f32 || y < dy as f32 || x >= (dx + w) as f32 || y >= (dy + h) as f32 {
            return None;
        }
        if self.bent() {
            let (u, v) = self
                .effects
                .bend(self.screen_fraction((x, y)), self.effect_time);
            if !(0.0..1.0).contains(&u) || !(0.0..1.0).contains(&v) {
                return None; // Black glass beyond the curved picture.
            }
        }
        Some(self.to_source_clamped((x, y)))
    }
    fn screen_fraction(self, (x, y): (f32, f32)) -> (f32, f32) {
        let (dx, dy, w, h) = self.destination;
        ((x - dx as f32) / w as f32, (y - dy as f32) / h as f32)
    }
    pub fn to_source_clamped(self, (x, y): (f32, f32)) -> (f32, f32) {
        let (sx, sy) = self.scale();
        let (mut px, mut py) = (
            (x - self.destination.0 as f32) / sx,
            (y - self.destination.1 as f32) / sy,
        );
        if self.bent() {
            let (u, v) = self
                .effects
                .bend(self.screen_fraction((x, y)), self.effect_time);
            (px, py) = (u * self.source.0 as f32, v * self.source.1 as f32);
        }
        (
            px.clamp(0.0, self.source.0 as f32 - 0.001),
            py.clamp(0.0, self.source.1 as f32 - 0.001),
        )
    }
    pub fn to_window(self, (x, y): (f32, f32)) -> (f32, f32) {
        let (sx, sy) = self.scale();
        if self.bent() {
            let (u, v) = self.effects.unbend(
                (x / self.source.0 as f32, y / self.source.1 as f32),
                self.effect_time,
            );
            let (dx, dy, w, h) = self.destination;
            return (dx as f32 + u * w as f32, dy as f32 + v * h as f32);
        }
        (
            self.destination.0 as f32 + x * sx,
            self.destination.1 as f32 + y * sy,
        )
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retro_profiles_are_bounded_and_native_is_a_real_bypass() {
        for theme in Theme::ALL {
            let r = Framebuffer::Auto.resolve(theme);
            assert_eq!(
                r.is_none(),
                matches!(theme, Theme::ModernDark | Theme::ModernLight)
            );
            if let Some((w, h)) = r {
                assert!(w <= 720 && h <= 480);
            }
            assert!(Framebuffer::Native.resolve(theme).is_none());
        }
        assert_eq!(
            Framebuffer::Auto.resolve(Theme::ZxSpectrum),
            Some((256, 192))
        );
        assert_eq!(Framebuffer::Auto.resolve(Theme::C64), Some((320, 200)));
    }
    #[test]
    fn the_wallpaper_machines_bring_their_screens() {
        assert_eq!(
            Framebuffer::Auto.resolve(Theme::MacSystem2),
            Some((512, 342))
        );
        assert_eq!(Framebuffer::Auto.resolve(Theme::GameBoy), Some((160, 144)));
        assert_eq!(Framebuffer::MacSystem2.name(), "Mac 512x342");
        assert_eq!(Framebuffer::GameBoy.name(), "Game Boy 160x144");
    }
    #[test]
    fn presentation_and_input_share_aspect_fit_scaling_and_letterbox_rejection() {
        for window in [(1960, 1400), (980, 700), (256, 192), (100, 70), (1, 1)] {
            let m = ScreenMapping::new(window, Some((256, 192)));
            let (x, y, w, h) = m.destination;
            assert!(x + w <= window.0 && y + h <= window.1);
            let p = (42.5, 60.5);
            let q = m.to_source(m.to_window(p)).unwrap();
            assert!((p.0 - q.0).abs() < 0.001 && (p.1 - q.1).abs() < 0.001);
            assert!(m.to_source(((x + w) as f32, y as f32)).is_none());
            if x > 0 {
                assert!(m.to_source((x as f32 - 1.0, y as f32)).is_none());
            }
            assert!(w == window.0 || h == window.1);
            // The non-fitted edge differs from the exact aspect ratio by less than one physical pixel.
            if w == window.0 {
                assert!((f64::from(w) * 192.0 / 256.0 - f64::from(h)).abs() < 1.0);
            } else {
                assert!((f64::from(h) * 256.0 / 192.0 - f64::from(w)).abs() < 1.0);
            }
        }
    }
    #[test]
    fn curvature_maps_clicks_where_the_bent_picture_shows_them() {
        let strong = CrtEffect::Strong.effects(VisualEffects::NEUTRAL);
        // Limited-resolution and Native screens bend the same way.
        for flat in [
            ScreenMapping::new((640, 480), Some((640, 480))),
            ScreenMapping::new((640, 480), None),
        ] {
            let bent = flat.with_effects(strong, 0.0, 1.0);
            assert_eq!(bent.to_source((320.0, 240.0)), Some((320.0, 240.0)));
            // Near a corner the picture is pushed outward, so a click there lands nearer the edge.
            let (fx, fy) = flat.to_source((40.0, 40.0)).unwrap();
            let (bx, by) = bent.to_source((40.0, 40.0)).unwrap();
            assert!(bx < fx && by < fy, "{bx},{by} vs {fx},{fy}");
            // The very corner shows black glass, not the picture.
            assert!(bent.to_source((1.0, 1.0)).is_none());
            // A keyboard-placed popup goes to the window and back to the same scene point.
            let p = (100.0, 80.0);
            let q = bent.to_source_clamped(bent.to_window(p));
            assert!(
                (p.0 - q.0).abs() < 0.01 && (p.1 - q.1).abs() < 0.01,
                "{q:?}"
            );
            // Later frames breathe; clicks follow the frame on screen and still round-trip.
            let later = flat.with_effects(strong, 1.0, 1.0);
            assert_ne!(later.to_source((40.0, 40.0)), bent.to_source((40.0, 40.0)));
            let q = later.to_source_clamped(later.to_window(p));
            assert!(
                (p.0 - q.0).abs() < 0.01 && (p.1 - q.1).abs() < 0.01,
                "{q:?}"
            );
            let off = flat.with_effects(VisualEffects::NEUTRAL, 0.0, 1.0);
            assert_eq!(off.to_source((40.0, 40.0)), Some((fx, fy)));
        }
    }
    #[test]
    fn effect_time_wraps_so_shader_hashes_keep_their_precision() {
        let e = VisualEffects {
            curvature: 1.0,
            ..VisualEffects::NEUTRAL
        };
        let m = ScreenMapping::new((640, 480), None).with_effects(e, 86_399.5, 1.0);
        assert!((m.effect_time - 86_399.5 % crate::visuals::EFFECT_PERIOD).abs() < 0.01);
        assert!(m.effect_time < crate::visuals::EFFECT_PERIOD);
        let p = (100.0, 80.0);
        let q = m.to_source_clamped(m.to_window(p));
        assert!(
            (p.0 - q.0).abs() < 0.01 && (p.1 - q.1).abs() < 0.01,
            "{q:?}"
        );
    }
    #[test]
    fn the_tube_carries_its_phosphor_and_theme_background_to_the_effect_pass() {
        let m = ScreenMapping::new((640, 480), None);
        assert_eq!((m.phosphor, m.background), (Phosphor::Color, 0));
        let green = m.with_glass(Phosphor::Green, 0x102030);
        assert_eq!(
            (green.phosphor, green.background),
            (Phosphor::Green, 0x102030)
        );
        assert_eq!(
            green
                .with_effects(VisualEffects::NEUTRAL, 1.0, 2.0)
                .phosphor,
            Phosphor::Green
        );
    }
    #[test]
    fn low_resolution_canvases_use_the_full_available_window_without_whole_number_steps() {
        assert_eq!(
            ScreenMapping::new((980, 700), Some((640, 480))).destination,
            (23, 0, 933, 700)
        );
        assert_eq!(
            ScreenMapping::new((980, 700), Some((256, 192))).destination,
            (23, 0, 933, 700)
        );
        assert_eq!(
            ScreenMapping::new((980, 700), Some((320, 200))).destination,
            (0, 44, 980, 612)
        );
        assert_eq!(
            ScreenMapping::new((1920, 1080), Some((320, 200))).destination,
            (96, 0, 1728, 1080)
        );
        assert_eq!(
            ScreenMapping::new((980, 700), None).destination,
            (0, 0, 980, 700)
        );
    }
}
