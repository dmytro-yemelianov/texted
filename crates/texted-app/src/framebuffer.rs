//! Fixed reference canvases and the shared presentation/input transform. No window or GPU state.
use crate::appearance::Theme;
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
}
impl Framebuffer {
    pub const ALL: [Self; 9] = [
        Self::Auto,
        Self::Native,
        Self::Vga,
        Self::Ega,
        Self::AtariSt,
        Self::DosText,
        Self::C64,
        Self::ZxSpectrum,
        Self::MonoText,
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
            },
            Self::Native => None,
            Self::Vga => Some((640, 480)),
            Self::Ega => Some((640, 350)),
            Self::AtariSt | Self::C64 => Some((320, 200)),
            Self::DosText => Some((720, 400)),
            Self::ZxSpectrum => Some((256, 192)),
            Self::MonoText => Some((720, 350)),
        }
    }
}
/// Screen effects ported from ShaderWallpaper's shader, with its ranges. Neutral values
/// (brightness 1, everything else 0) draw the exact picture.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct VisualEffects {
    pub brightness: f32,
    pub curvature: f32,
    pub chromatic_aberration: f32,
    pub afterglow: f32,
    pub noise: f32,
    pub flicker: f32,
    pub scanlines: f32,
}
impl Default for VisualEffects {
    fn default() -> Self {
        Self::NEUTRAL
    }
}
impl VisualEffects {
    pub const NEUTRAL: Self = Self {
        brightness: 1.0,
        curvature: 0.0,
        chromatic_aberration: 0.0,
        afterglow: 0.0,
        noise: 0.0,
        flicker: 0.0,
        scanlines: 0.0,
    };
    /// The values tuned in ShaderWallpaper's Visuals tab.
    pub const WALLPAPER: Self = Self {
        brightness: 0.96,
        curvature: 0.014,
        chromatic_aberration: 0.18,
        afterglow: 0.12,
        noise: 0.55,
        flicker: 0.45,
        scanlines: 0.42,
    };
    /// Pulls every amount into ShaderWallpaper's range; a non-number takes the neutral value.
    pub fn normalized(self) -> Self {
        let fit = |value: f32, low: f32, high: f32, neutral: f32| {
            if value.is_finite() {
                value.clamp(low, high)
            } else {
                neutral
            }
        };
        Self {
            brightness: fit(self.brightness, 0.5, 1.6, 1.0),
            curvature: fit(self.curvature, 0.0, 0.08, 0.0),
            chromatic_aberration: fit(self.chromatic_aberration, 0.0, 1.5, 0.0),
            afterglow: fit(self.afterglow, 0.0, 1.2, 0.0),
            noise: fit(self.noise, 0.0, 2.0, 0.0),
            flicker: fit(self.flicker, 0.0, 2.0, 0.0),
            scanlines: fit(self.scanlines, 0.0, 1.25, 0.0),
        }
    }
    pub fn is_neutral(&self) -> bool {
        *self == Self::NEUTRAL
    }
    /// Noise and flicker change every frame; without them the picture stays still.
    pub fn animated(&self) -> bool {
        self.noise > 0.0 || self.flicker > 0.0
    }
    /// The shader's pillow distortion with its slow drift at rest (1 + 0.4 x 0.5).
    fn pillow(&self) -> f32 {
        self.curvature * 1.2
    }
    /// The picture point shown at screen point `(u, v)`, 0..1 each, as presentation.wgsl bends it.
    fn bend(&self, (u, v): (f32, f32)) -> (f32, f32) {
        let (x, y) = (u * 2.0 - 1.0, v * 2.0 - 1.0);
        let f = 1.0 + self.pillow() * (x * x + y * y);
        (x * f * 0.5 + 0.5, y * f * 0.5 + 0.5)
    }
    /// The inverse of `bend` by fixed-point iteration, which converges fast for small curvature.
    fn unbend(&self, (u, v): (f32, f32)) -> (f32, f32) {
        let (tx, ty) = (u * 2.0 - 1.0, v * 2.0 - 1.0);
        let (mut x, mut y) = (tx, ty);
        for _ in 0..8 {
            let f = 1.0 + self.pillow() * (x * x + y * y);
            (x, y) = (tx / f, ty / f);
        }
        (x * 0.5 + 0.5, y * 0.5 + 0.5)
    }
}
/// The CRT effect row: presets, or the `effects` block of appearance.json for Custom.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CrtEffect {
    #[default]
    Off,
    Subtle,
    Strong,
    Wallpaper,
    Custom,
}
impl CrtEffect {
    pub const ALL: [Self; 5] = [
        Self::Off,
        Self::Subtle,
        Self::Strong,
        Self::Wallpaper,
        Self::Custom,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Subtle => "Subtle",
            Self::Strong => "Strong",
            Self::Wallpaper => "Wallpaper",
            Self::Custom => "Custom",
        }
    }
    /// Subtle and Strong stay still; Wallpaper moves like the desktop it came from.
    pub fn effects(self, custom: VisualEffects) -> VisualEffects {
        let neutral = VisualEffects::NEUTRAL;
        match self {
            Self::Off => neutral,
            Self::Subtle => VisualEffects {
                curvature: 0.012,
                chromatic_aberration: 0.1,
                afterglow: 0.1,
                scanlines: 0.3,
                ..neutral
            },
            Self::Strong => VisualEffects {
                brightness: 1.05,
                curvature: 0.04,
                chromatic_aberration: 0.5,
                afterglow: 0.4,
                scanlines: 0.8,
                ..neutral
            },
            Self::Wallpaper => VisualEffects::WALLPAPER,
            Self::Custom => custom.normalized(),
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
        }
    }
    /// Effects for the window backend; native and limited-resolution screens both get them.
    pub fn with_effects(mut self, effects: VisualEffects, time: f32, pixel: f32) -> Self {
        self.effects = effects;
        self.effect_time = time;
        self.effect_pixel = pixel.max(1.0);
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
            let (u, v) = self.effects.bend(self.screen_fraction((x, y)));
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
            let (u, v) = self.effects.bend(self.screen_fraction((x, y)));
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
            let (u, v) = self
                .effects
                .unbend((x / self.source.0 as f32, y / self.source.1 as f32));
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
            let off = flat.with_effects(VisualEffects::NEUTRAL, 0.0, 1.0);
            assert_eq!(off.to_source((40.0, 40.0)), Some((fx, fy)));
        }
    }
    #[test]
    fn presets_and_custom_values_stay_inside_shaderwallpaper_ranges() {
        let wallpaper = CrtEffect::Wallpaper.effects(VisualEffects::NEUTRAL);
        assert_eq!(
            wallpaper,
            VisualEffects {
                brightness: 0.96,
                curvature: 0.014,
                chromatic_aberration: 0.18,
                afterglow: 0.12,
                noise: 0.55,
                flicker: 0.45,
                scanlines: 0.42,
            }
        );
        assert!(wallpaper.animated());
        assert!(!CrtEffect::Strong.effects(wallpaper).animated());
        assert!(CrtEffect::Off.effects(wallpaper).is_neutral());
        let wild = VisualEffects {
            brightness: 9.0,
            curvature: -1.0,
            noise: f32::NAN,
            ..VisualEffects::NEUTRAL
        };
        let custom = CrtEffect::Custom.effects(wild);
        assert_eq!(
            (custom.brightness, custom.curvature, custom.noise),
            (1.6, 0.0, 0.0)
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
