//! Versioned appearance values. Document contents and editing state are independent of these settings.
use crate::framebuffer::Framebuffer;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    #[default]
    ModernDark,
    ModernLight,
    Vga,
    Ega,
    Atari,
    Commander,
    C64,
    ZxSpectrum,
    Amber,
    GreenCrt,
    /// ShaderWallpaper's Mac System 2.0 and Game Boy machines.
    MacSystem2,
    GameBoy,
}
impl Theme {
    pub const ALL: [Self; 12] = [
        Self::ModernDark,
        Self::ModernLight,
        Self::Vga,
        Self::Ega,
        Self::Atari,
        Self::Commander,
        Self::C64,
        Self::ZxSpectrum,
        Self::Amber,
        Self::GreenCrt,
        Self::MacSystem2,
        Self::GameBoy,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Self::ModernDark => "Modern Dark",
            Self::ModernLight => "Modern Light",
            Self::Vga => "VGA",
            Self::Ega => "EGA",
            Self::Atari => "Atari",
            Self::Commander => "Commander",
            Self::C64 => "C64",
            Self::ZxSpectrum => "ZX Spectrum",
            Self::Amber => "Amber",
            Self::GreenCrt => "Green CRT",
            Self::MacSystem2 => "Mac System 2.0",
            Self::GameBoy => "Game Boy",
        }
    }
    pub fn step(self, delta: i32) -> Self {
        let i = Self::ALL.iter().position(|v| *v == self).unwrap();
        Self::ALL[(i as i32 + delta).rem_euclid(Self::ALL.len() as i32) as usize]
    }
    pub fn palette(self) -> Palette {
        if self == Self::ModernDark {
            return Palette::default();
        }
        let (bg, fg, panel, muted, accent, selection, found, active) = match self {
            Self::ModernLight => (
                0xfafafa, 0x202020, 0xededed, 0x555555, 0x005a9e, 0xb6d6f2, 0xffedb0, 0xffd36b,
            ),
            Self::Vga => (
                0x101018, 0xdddddd, 0x202030, 0xaaaaaa, 0x55ffff, 0x004488, 0x504000, 0x806000,
            ),
            Self::Ega => (
                0x000000, 0xaaaaaa, 0x000055, 0xaaaaaa, 0x55ffff, 0x0000aa, 0x555500, 0x887700,
            ),
            Self::Atari => (
                0x101820, 0xd8d0a0, 0x202c38, 0x98a8b0, 0xffd880, 0x3b5060, 0x50502b, 0x807030,
            ),
            Self::Commander => (
                0x000080, 0xeeeeee, 0x0000aa, 0xaaaaff, 0x55ffff, 0x005555, 0x555500, 0x887700,
            ),
            Self::C64 => (
                0x352879, 0xb8abea, 0x40318d, 0x9b8fce, 0xe8dcff, 0x6454a0, 0x5a4b60, 0x806050,
            ),
            Self::ZxSpectrum => (
                0x000000, 0xd7d7d7, 0x171717, 0xaaaaaa, 0x00ffff, 0x004050, 0x504000, 0x806000,
            ),
            Self::Amber => (
                0x150e03, 0xffca70, 0x241805, 0xc29a55, 0xffe3a0, 0x5d3b0b, 0x62450e, 0x805b13,
            ),
            Self::GreenCrt => (
                0x061108, 0x90e69c, 0x0d2012, 0x68ac74, 0xc0ffd0, 0x234f2c, 0x375126, 0x536b2b,
            ),
            // Background, clock text, details and accent from ShaderWallpaper's palettes.
            Self::MacSystem2 => (
                0xe3e0d1, 0x141414, 0xd4d1c2, 0x2e2e2e, 0x050505, 0xb8b5a8, 0xcfccbf, 0xa3a094,
            ),
            Self::GameBoy => (
                0x9cad75, 0x2e3d1f, 0x8e9f68, 0x47592e, 0x1a2412, 0x7d8e57, 0xb5c48f, 0x6e7f4a,
            ),
            Self::ModernDark => unreachable!(),
        };
        Palette {
            bg,
            text: fg,
            gutter_bg: panel,
            gutter_border: muted,
            gutter_text: muted,
            caret: accent,
            selection,
            block_selection: selection,
            search_match: found,
            search_active: active,
            panel_bg: panel,
            panel_border: muted,
            input_bg: bg,
            input_border: muted,
            input_active: accent,
            button_bg: bg,
            button_border: muted,
            status_bg: panel,
            status_border: muted,
            status_text: fg,
            status_accent: accent,
            // ShaderWallpaper's machines shadow with black; a light screen needs grey.
            shadow: match self {
                Self::ModernLight => 0xb0b0b0,
                Self::MacSystem2 => 0x1f1f1f,
                Self::GameBoy => 0x2b361f,
                _ => 0x000000,
            },
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    pub bg: u32,
    pub text: u32,
    pub gutter_bg: u32,
    pub gutter_border: u32,
    pub gutter_text: u32,
    pub caret: u32,
    pub selection: u32,
    pub block_selection: u32,
    pub search_match: u32,
    pub search_active: u32,
    pub panel_bg: u32,
    pub panel_border: u32,
    pub input_bg: u32,
    pub input_border: u32,
    pub input_active: u32,
    pub button_bg: u32,
    pub button_border: u32,
    pub status_bg: u32,
    pub status_border: u32,
    pub status_text: u32,
    pub status_accent: u32,
    /// Behind shadowed text and borders, as ShaderWallpaper's palette `shadow`.
    pub shadow: u32,
}
impl Default for Palette {
    fn default() -> Self {
        Self {
            bg: 0x1e1e1e,
            gutter_bg: 0x181818,
            gutter_border: 0x2b2b2b,
            gutter_text: 0x6e6e6e,
            text: 0xd4d4d4,
            caret: 0xffffff,
            selection: 0x264f78,
            block_selection: 0x1e4068,
            search_match: 0x52441c,
            search_active: 0x8f6400,
            panel_bg: 0x252526,
            panel_border: 0x3f3f46,
            input_bg: 0x1e1e1e,
            input_border: 0x3c3c3c,
            input_active: 0x0e639c,
            button_bg: 0x333333,
            button_border: 0x4a4a4a,
            status_bg: 0x141414,
            status_border: 0x282828,
            status_text: 0x888888,
            status_accent: 0x569cd6,
            shadow: 0,
        }
    }
}
impl Palette {
    fn set(&mut self, key: &str, value: u32) -> Result<(), String> {
        let slot = match key {
            "bg" => &mut self.bg,
            "text" => &mut self.text,
            "gutter_bg" => &mut self.gutter_bg,
            "gutter_border" => &mut self.gutter_border,
            "gutter_text" => &mut self.gutter_text,
            "caret" => &mut self.caret,
            "selection" => &mut self.selection,
            "block_selection" => &mut self.block_selection,
            "search_match" => &mut self.search_match,
            "search_active" => &mut self.search_active,
            "panel_bg" => &mut self.panel_bg,
            "panel_border" => &mut self.panel_border,
            "input_bg" => &mut self.input_bg,
            "input_border" => &mut self.input_border,
            "input_active" => &mut self.input_active,
            "button_bg" => &mut self.button_bg,
            "button_border" => &mut self.button_border,
            "status_bg" => &mut self.status_bg,
            "status_border" => &mut self.status_border,
            "status_text" => &mut self.status_text,
            "status_accent" => &mut self.status_accent,
            "shadow" => &mut self.shadow,
            _ => return Err(format!("Unknown appearance color: {key}")),
        };
        *slot = value;
        Ok(())
    }
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum FontFamily {
    #[default]
    Vga,
    Bios,
    Ega,
    SpleenSmall,
    Spleen,
    Local(u16),
}
impl FontFamily {
    pub const ALL: [Self; 5] = [
        Self::Bios,
        Self::Ega,
        Self::Vga,
        Self::SpleenSmall,
        Self::Spleen,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Self::Bios => "IBM BIOS 8x8",
            Self::Ega => "IBM EGA 8x14",
            Self::Vga => "IBM VGA 8x16",
            Self::SpleenSmall => "Spleen 6x12",
            Self::Spleen => "Spleen 8x16",
            Self::Local(index) => {
                crate::font_library::get(index).map_or("Unavailable font", |f| f.name.as_str())
            }
        }
    }
    pub fn maximum_size(self) -> u16 {
        let (width, height) = self.cell();
        let scale = (64 / height)
            .max(1)
            .min(1022 / width)
            .min(1022 / height)
            .max(1);
        height * scale
    }
    pub fn cell(self) -> (u16, u16) {
        match self {
            Self::Bios => (8, 8),
            Self::Ega => (8, 14),
            Self::Vga | Self::Spleen => (8, 16),
            Self::SpleenSmall => (6, 12),
            Self::Local(index) => crate::font_library::get(index)
                .filter(|f| f.enabled())
                .map_or((8, 16), |f| (f.width, f.height)),
        }
    }
}
impl Serialize for FontFamily {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let id = match *self {
            Self::Vga => "vga_8x16",
            Self::Bios => "bios_8x8",
            Self::Ega => "ega_8x14",
            Self::SpleenSmall => "spleen_6x12",
            Self::Spleen => "spleen_8x16",
            Self::Local(index) => {
                return serializer.serialize_str(&format!(
                    "verba:{}",
                    crate::font_library::get(index)
                        .ok_or_else(|| serde::ser::Error::custom("Unknown local font"))?
                        .id
                ))
            }
        };
        serializer.serialize_str(id)
    }
}
impl<'de> Deserialize<'de> for FontFamily {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let id = String::deserialize(deserializer)?;
        Ok(match id.as_str() {
            "vga_8x16" | "courier" => Self::Vga,
            "bios_8x8" | "pixel" => Self::Bios,
            "ega_8x14" => Self::Ega,
            "spleen_6x12" | "monaco" => Self::SpleenSmall,
            "spleen_8x16" | "menlo" => Self::Spleen,
            name if name.starts_with("verba:") => crate::font_library::find(&name[6..])
                .map(Self::Local)
                .unwrap_or(Self::Vga),
            _ => return Err(serde::de::Error::custom("Unknown bitmap font family")),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Typography {
    pub family: FontFamily,
    pub size_px: u16,
    pub line_spacing: u16,
    pub zoom_percent: u16,
}
impl Default for Typography {
    fn default() -> Self {
        Self {
            family: FontFamily::Vga,
            size_px: 16,
            line_spacing: 100,
            zoom_percent: 100,
        }
    }
}
impl Typography {
    pub fn normalize(&mut self) {
        let height = self.family.cell().1;
        let maximum = self.family.maximum_size();
        self.size_px = ((self.size_px.clamp(height, maximum) + height / 2) / height * height)
            .clamp(height, maximum);
        self.line_spacing = self.line_spacing.clamp(100, 180);
        self.zoom_percent = self.zoom_percent.clamp(10, 800);
    }
    pub fn scale_percent(self) -> u16 {
        ((u32::from(self.size_px) * u32::from(self.zoom_percent)) / u32::from(self.family.cell().1))
            .min(800) as u16
    }
    pub fn resize(&mut self, delta: i32) {
        let scale = (i32::from(self.scale_percent()) + delta * 10).clamp(10, 800);
        self.size_px = self.family.cell().1;
        self.zoom_percent = scale as u16;
        self.normalize();
    }
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MouseCursor {
    #[default]
    System,
    Pixel,
    LargePixel,
    SmallPixel,
    DarkPixel,
    AmberPixel,
    GreenPixel,
    CyanPixel,
    Classic,
    Crosshair,
    Ring,
}
impl MouseCursor {
    pub const ALL: [Self; 11] = [
        Self::System,
        Self::SmallPixel,
        Self::Pixel,
        Self::LargePixel,
        Self::DarkPixel,
        Self::AmberPixel,
        Self::GreenPixel,
        Self::CyanPixel,
        Self::Classic,
        Self::Crosshair,
        Self::Ring,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Self::System => "System",
            Self::Pixel => "Pixel",
            Self::LargePixel => "Large pixel",
            Self::SmallPixel => "Small pixel",
            Self::DarkPixel => "Dark pixel",
            Self::AmberPixel => "Amber pixel",
            Self::GreenPixel => "Green pixel",
            Self::CyanPixel => "Cyan pixel",
            Self::Classic => "Classic arrow",
            Self::Crosshair => "Crosshair",
            Self::Ring => "Ring",
        }
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Appearance {
    pub version: u32,
    pub theme: Theme,
    pub mouse_cursor: MouseCursor,
    pub typography: Typography,
    pub framebuffer: Framebuffer,
    /// The Visuals page; files without it have no screen effects.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visuals: Option<crate::visuals::Visuals>,
    /// Read from files written before the Visuals page, then folded into `visuals`.
    #[serde(skip_serializing)]
    pub crt: Option<crate::visuals::CrtEffect>,
    #[serde(skip_serializing)]
    pub effects: Option<crate::visuals::VisualEffects>,
    pub pixel_typography: Typography,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub colors: BTreeMap<String, String>,
}
impl Default for Appearance {
    fn default() -> Self {
        Self {
            version: 1,
            theme: Theme::default(),
            mouse_cursor: MouseCursor::System,
            typography: Typography::default(),
            framebuffer: Framebuffer::default(),
            visuals: None,
            crt: None,
            effects: None,
            pixel_typography: Typography {
                family: FontFamily::Bios,
                size_px: 8,
                line_spacing: 100,
                zoom_percent: 100,
            },
            colors: BTreeMap::new(),
        }
    }
}
impl Appearance {
    /// The Visuals page settings, including what an older `crt` key meant.
    pub fn visuals(&self) -> crate::visuals::Visuals {
        self.visuals
            .or_else(|| {
                self.crt
                    .map(|crt| crt.visuals(self.effects.unwrap_or_default()))
            })
            .unwrap_or_default()
    }
    pub fn resolution(&self) -> Option<(u32, u32)> {
        self.framebuffer.resolve(self.theme)
    }
    pub fn editor_typography(&self) -> Typography {
        if self.resolution().is_some() {
            self.pixel_typography
        } else {
            self.typography
        }
    }
    pub fn editor_typography_mut(&mut self) -> &mut Typography {
        if self.resolution().is_some() {
            &mut self.pixel_typography
        } else {
            &mut self.typography
        }
    }

    pub fn normalize(&mut self) -> Result<(), String> {
        if self.version != 1 {
            return Err(format!("Unsupported appearance version {}", self.version));
        }
        if self.visuals.is_some() || self.crt.is_some() {
            self.visuals = Some(self.visuals().normalized());
        }
        (self.crt, self.effects) = (None, None);
        self.typography.normalize();
        self.pixel_typography.normalize();
        self.palette()?;
        Ok(())
    }
    pub fn palette(&self) -> Result<Palette, String> {
        let mut palette = self.theme.palette();
        for (key, color) in &self.colors {
            let digits = color
                .strip_prefix('#')
                .filter(|s| s.len() == 6 && s.bytes().all(|b| b.is_ascii_hexdigit()))
                .ok_or_else(|| format!("Color {key} must be #RRGGBB"))?;
            let rgb = u32::from_str_radix(digits, 16)
                .map_err(|_| format!("Color {key} must be #RRGGBB"))?;
            palette.set(key, rgb)?;
        }
        Ok(palette)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn zoom_changes_by_ten_percent_without_changing_native_masks() {
        for family in FontFamily::ALL {
            let mut t = Typography {
                family,
                size_px: family.cell().1,
                ..Default::default()
            };
            for expected in [110, 120, 130] {
                t.resize(1);
                assert_eq!(t.scale_percent(), expected);
                assert_eq!(t.size_px, family.cell().1);
            }
            t.resize(-1);
            assert_eq!(t.scale_percent(), 120);
            t.resize(-1000);
            assert_eq!(t.scale_percent(), 10);
            t.resize(1000);
            assert_eq!(t.scale_percent(), 800);
        }
    }
    #[test]
    fn legacy_outline_preferences_migrate_to_bundled_faces_and_valid_native_sizes() {
        for (old, family) in [
            ("courier", FontFamily::Vga),
            ("menlo", FontFamily::Spleen),
            ("monaco", FontFamily::SpleenSmall),
            ("pixel", FontFamily::Bios),
        ] {
            let mut t: Typography =
                serde_json::from_value(serde_json::json!({"family":old,"size_px":18})).unwrap();
            t.normalize();
            assert_eq!(t.family, family);
            assert_eq!(t.size_px % family.cell().1, 0);
            assert_ne!(serde_json::to_value(t).unwrap()["family"], old);
        }
        for family in FontFamily::ALL {
            for size_px in 0..=u16::MAX {
                let mut t = Typography {
                    family,
                    size_px,
                    line_spacing: 100,
                    zoom_percent: 100,
                };
                t.normalize();
                assert!((family.cell().1..=64).contains(&t.size_px));
                assert_eq!(t.size_px % family.cell().1, 0);
                let before = t;
                t.normalize();
                assert_eq!(before, t);
            }
        }
    }
    #[test]
    fn existing_preferences_get_auto_resolution_and_isolated_pixel_typography() {
        let mut a: Appearance = serde_json::from_str(
            r#"{"version":1,"theme":"zx_spectrum","typography":{"family":"menlo","size_px":24}}"#,
        )
        .unwrap();
        a.normalize().unwrap();
        let native = a.typography;
        assert_eq!(a.resolution(), Some((256, 192)));
        assert_eq!(a.editor_typography().size_px, 8);
        assert_eq!(a.editor_typography().family, FontFamily::Bios);
        a.editor_typography_mut().resize(1);
        a.framebuffer = Framebuffer::Native;
        assert_eq!(a.editor_typography(), native);
        a.framebuffer = Framebuffer::Auto;
        assert_eq!(a.editor_typography().scale_percent(), 110);
        let copy: Appearance = serde_json::from_str(&serde_json::to_string(&a).unwrap()).unwrap();
        assert_eq!(copy, a);
        assert!(serde_json::from_str::<Appearance>(r#"{"framebuffer":"4k"}"#).is_err());
    }
    #[test]
    fn settings_round_trip_preserves_custom_colors_and_normalizes_pixel_metrics() {
        let mut a = Appearance {
            theme: Theme::C64,
            typography: Typography {
                family: FontFamily::Bios,
                size_px: 19,
                ..Default::default()
            },
            ..Default::default()
        };
        a.colors.insert("text".into(), "#ABCDEF".into());
        a.normalize().unwrap();
        assert_eq!(a.typography.size_px, 16);
        assert_eq!(a.palette().unwrap().text, 0xabcdef);
        assert_eq!(
            serde_json::from_str::<Appearance>(&serde_json::to_string(&a).unwrap()).unwrap(),
            a
        );
    }
    #[test]
    fn shaderwallpaper_machines_are_themes_with_their_palettes() {
        for (theme, id, name, bg, text, shadow) in [
            (
                Theme::MacSystem2,
                "mac_system2",
                "Mac System 2.0",
                0xe3e0d1,
                0x141414,
                0x1f1f1f,
            ),
            (
                Theme::GameBoy,
                "game_boy",
                "Game Boy",
                0x9cad75,
                0x2e3d1f,
                0x2b361f,
            ),
        ] {
            let p = theme.palette();
            assert_eq!((p.bg, p.text, p.shadow), (bg, text, shadow), "{name}");
            assert_eq!(theme.name(), name);
            assert_eq!(serde_json::to_string(&theme).unwrap(), format!("\"{id}\""));
            assert!(Theme::ALL.contains(&theme));
        }
    }
    #[test]
    fn every_theme_has_a_shadow_color_and_it_can_be_overridden() {
        for theme in Theme::ALL {
            let expected = match theme {
                Theme::ModernLight => 0xb0b0b0,
                Theme::MacSystem2 => 0x1f1f1f,
                Theme::GameBoy => 0x2b361f,
                _ => 0x000000,
            };
            assert_eq!(theme.palette().shadow, expected, "{theme:?}");
        }
        let mut a = Appearance::default();
        a.colors.insert("shadow".into(), "#203040".into());
        assert_eq!(a.palette().unwrap().shadow, 0x203040);
    }
    #[test]
    fn visuals_are_saved_and_older_crt_files_migrate() {
        use crate::visuals::Machine;
        let old: Appearance =
            serde_json::from_str(r#"{"theme":"green_crt","crt":"wallpaper"}"#).unwrap();
        assert_eq!(old.visuals(), Machine::AtariSt.visuals());
        let mut migrated = old.clone();
        migrated.normalize().unwrap();
        let json = serde_json::to_string(&migrated).unwrap();
        assert!(
            json.contains(r#""visuals""#) && !json.contains(r#""crt""#),
            "{json}"
        );
        let again: Appearance = serde_json::from_str(&json).unwrap();
        assert_eq!(again.visuals(), Machine::AtariSt.visuals());
        let plain: Appearance = serde_json::from_str(r#"{"theme":"zx_spectrum"}"#).unwrap();
        assert!(plain.visuals().effects.is_neutral());
    }
    #[test]
    fn future_versions_unknown_colors_and_invalid_rgb_are_rejected() {
        let mut a = Appearance {
            version: 2,
            ..Default::default()
        };
        assert!(a.normalize().is_err());
        a.version = 1;
        a.colors.insert("bg".into(), "#GG0000".into());
        assert!(a.normalize().is_err());
        a.colors.insert("bg".into(), "#+12345".into());
        assert!(a.normalize().is_err());
        a.colors.clear();
        a.colors.insert("unknown".into(), "#ffffff".into());
        assert!(a.normalize().is_err());
    }
    #[test]
    fn theme_changes_do_not_select_a_font_and_every_preset_has_distinct_overlays() {
        let typography = Typography::default();
        for theme in Theme::ALL {
            let a = Appearance {
                theme,
                typography,
                ..Default::default()
            };
            let p = a.palette().unwrap();
            assert_ne!(p.text, p.bg);
            assert_ne!(p.selection, p.bg);
            assert_ne!(p.search_match, p.search_active);
            assert_eq!(a.typography, typography);
        }
    }
}
