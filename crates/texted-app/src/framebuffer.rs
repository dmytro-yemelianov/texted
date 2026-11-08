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
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScreenMapping {
    pub source: (u32, u32),
    pub destination: (u32, u32, u32, u32),
    pub pixelated: bool,
}
impl ScreenMapping {
    pub fn new(window: (u32, u32), resolution: Option<(u32, u32)>) -> Self {
        let (width, height) = (window.0.max(1), window.1.max(1));
        let Some((w, h)) = resolution else {
            return Self {
                source: (width, height),
                destination: (0, 0, width, height),
                pixelated: false,
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
        }
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
        Some(self.to_source_clamped((x, y)))
    }
    pub fn to_source_clamped(self, (x, y): (f32, f32)) -> (f32, f32) {
        let (sx, sy) = self.scale();
        (
            ((x - self.destination.0 as f32) / sx).clamp(0.0, self.source.0 as f32 - 0.001),
            ((y - self.destination.1 as f32) / sy).clamp(0.0, self.source.1 as f32 - 0.001),
        )
    }
    pub fn to_window(self, (x, y): (f32, f32)) -> (f32, f32) {
        let (sx, sy) = self.scale();
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
