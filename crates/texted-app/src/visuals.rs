//! ShaderWallpaper's Visuals tab: vintage machines, effect amounts with their ranges and
//! steps, and animation timing. Window rendering reads it; terminals ignore it.
use crate::appearance::Theme;
use serde::{Deserialize, Serialize};
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
    pub(crate) fn bend(&self, (u, v): (f32, f32)) -> (f32, f32) {
        let (x, y) = (u * 2.0 - 1.0, v * 2.0 - 1.0);
        let f = 1.0 + self.pillow() * (x * x + y * y);
        (x * f * 0.5 + 0.5, y * f * 0.5 + 0.5)
    }
    /// The inverse of `bend` by fixed-point iteration, which converges fast for small curvature.
    pub(crate) fn unbend(&self, (u, v): (f32, f32)) -> (f32, f32) {
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
            Self::Wallpaper => Machine::AtariSt.visuals().effects,
            Self::Custom => custom.normalized(),
        }
    }
}
/// Seconds since local midnight, the clock ShaderWallpaper animates its effects with.
pub fn local_seconds_since_midnight() -> f64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    #[cfg(unix)]
    {
        let secs = now.as_secs() as libc::time_t;
        // SAFETY: localtime_r only writes the zeroed `tm` it is given.
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        if !unsafe { libc::localtime_r(&secs, &mut tm) }.is_null() {
            let whole = tm.tm_hour * 3600 + tm.tm_min * 60 + tm.tm_sec;
            return f64::from(whole) + f64::from(now.subsec_nanos()) * 1e-9;
        }
    }
    (now.as_secs() % 86_400) as f64 + f64::from(now.subsec_nanos()) * 1e-9
}
/// One effect row of ShaderWallpaper's Visuals tab, with its range and step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    Brightness,
    Curvature,
    ChromaticAberration,
    Afterglow,
    Noise,
    Flicker,
    Scanlines,
}
impl Effect {
    pub const ALL: [Self; 7] = [
        Self::Brightness,
        Self::Curvature,
        Self::ChromaticAberration,
        Self::Afterglow,
        Self::Noise,
        Self::Flicker,
        Self::Scanlines,
    ];
    pub fn title(self) -> &'static str {
        match self {
            Self::Brightness => "Brightness",
            Self::Curvature => "Curvature",
            Self::ChromaticAberration => "Chromatic aberration",
            Self::Afterglow => "Afterglow",
            Self::Noise => "Noise",
            Self::Flicker => "Flicker",
            Self::Scanlines => "Scanlines",
        }
    }
    /// Low, high and step, as WallpaperVisualEffectRegistry defines them.
    fn range(self) -> (f64, f64, f64) {
        match self {
            Self::Brightness => (0.5, 1.6, 0.05),
            Self::Curvature => (0.0, 0.08, 0.0025),
            Self::ChromaticAberration => (0.0, 1.5, 0.05),
            Self::Afterglow => (0.0, 1.2, 0.05),
            Self::Noise | Self::Flicker => (0.0, 2.0, 0.05),
            Self::Scanlines => (0.0, 1.25, 0.05),
        }
    }
    /// Values a row offers: the range divided into the wallpaper's steps.
    pub fn steps(self) -> u16 {
        let (low, high, step) = self.range();
        ((high - low) / step).round() as u16
    }
    pub fn value_at(self, step: u16) -> f32 {
        let (low, _, size) = self.range();
        (low + f64::from(step.min(self.steps())) * size) as f32
    }
    pub fn step_of(self, value: f32) -> u16 {
        let (low, _, size) = self.range();
        ((f64::from(value) - low) / size)
            .round()
            .clamp(0.0, f64::from(self.steps())) as u16
    }
    pub fn get(self, e: &VisualEffects) -> f32 {
        match self {
            Self::Brightness => e.brightness,
            Self::Curvature => e.curvature,
            Self::ChromaticAberration => e.chromatic_aberration,
            Self::Afterglow => e.afterglow,
            Self::Noise => e.noise,
            Self::Flicker => e.flicker,
            Self::Scanlines => e.scanlines,
        }
    }
    pub fn set(self, e: &mut VisualEffects, value: f32) {
        let field = match self {
            Self::Brightness => &mut e.brightness,
            Self::Curvature => &mut e.curvature,
            Self::ChromaticAberration => &mut e.chromatic_aberration,
            Self::Afterglow => &mut e.afterglow,
            Self::Noise => &mut e.noise,
            Self::Flicker => &mut e.flicker,
            Self::Scanlines => &mut e.scanlines,
        };
        *field = value;
    }
    /// Curvature steps in quarter hundredths, so it needs four decimals.
    pub fn label(self, value: f32) -> String {
        if self == Self::Curvature {
            format!("{value:.4}")
        } else {
            format!("{value:.2}")
        }
    }
}
/// ShaderWallpaper's machine presets and their render defaults.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Machine {
    ZxSpectrum,
    AtariSt,
    MacSystem2,
    GameBoy,
}
impl Machine {
    pub const ALL: [Self; 4] = [
        Self::ZxSpectrum,
        Self::AtariSt,
        Self::MacSystem2,
        Self::GameBoy,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Self::ZxSpectrum => "ZX Spectrum",
            Self::AtariSt => "Atari ST",
            Self::MacSystem2 => "Mac System 2.0",
            Self::GameBoy => "Game Boy",
        }
    }
    /// Frame rate and the seven amounts, in Effect::ALL order, from ShaderWallpaper.
    pub fn visuals(self) -> Visuals {
        let (
            fps,
            [brightness, curvature, chromatic_aberration, afterglow, noise, flicker, scanlines],
        ) = match self {
            Self::ZxSpectrum => (12, [1.0, 0.025, 0.35, 0.2, 1.0, 1.0, 1.0]),
            Self::AtariSt => (14, [0.96, 0.014, 0.18, 0.12, 0.55, 0.45, 0.42]),
            Self::MacSystem2 => (10, [0.95, 0.003, 0.0, 0.04, 0.08, 0.06, 0.0]),
            Self::GameBoy => (8, [0.92, 0.0, 0.0, 0.28, 0.18, 0.12, 0.0]),
        };
        Visuals {
            fps,
            effects: VisualEffects {
                brightness,
                curvature,
                chromatic_aberration,
                afterglow,
                noise,
                flicker,
                scanlines,
            },
            // WallpaperChromeEffectRegistry defaults: shadow 1,1, single tiles, inset 2,0.
            shadow: true,
            border: Border::Single,
            inset_x: 2,
            ..Visuals::default()
        }
    }
}
/// How panels are framed: Texted's thin lines, or ShaderWallpaper's single or double tiles.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Border {
    #[default]
    Lines,
    Single,
    Double,
}
impl Border {
    pub const ALL: [Self; 3] = [Self::Lines, Self::Single, Self::Double];
    pub fn name(self) -> &'static str {
        match self {
            Self::Lines => "Lines",
            Self::Single => "Single ─│",
            Self::Double => "Double ═║",
        }
    }
}
/// Everything the Visuals page sets; saved in appearance.json.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Visuals {
    /// Frames per second for moving effects, 4 to 30.
    pub fps: u8,
    /// A fixed time of day in seconds for every moving effect; `None` follows the clock.
    pub time: Option<u32>,
    pub effects: VisualEffects,
    /// Chrome shadow: text drawn again behind itself, offset in font pixels (0 to 4).
    pub shadow: bool,
    pub shadow_x: u8,
    pub shadow_y: u8,
    /// Chrome border: the frame style, and how far edge tiles start from corners (0 to 32).
    pub border: Border,
    pub corner_inset: u8,
    /// Chrome panel inset: room around the document text in font pixels (0 to 8).
    pub inset_x: u8,
    pub inset_y: u8,
}
impl Default for Visuals {
    fn default() -> Self {
        Self {
            fps: 12,
            time: None,
            effects: VisualEffects::NEUTRAL,
            shadow: false,
            shadow_x: 1,
            shadow_y: 1,
            border: Border::Lines,
            corner_inset: 16,
            inset_x: 0,
            inset_y: 0,
        }
    }
}
impl Visuals {
    /// The defaults a theme brings, as applying a machine does in ShaderWallpaper.
    pub fn for_theme(theme: Theme) -> Self {
        match theme {
            Theme::ZxSpectrum => Machine::ZxSpectrum.visuals(),
            Theme::Atari => Machine::AtariSt.visuals(),
            _ => Self::default(),
        }
    }
    pub fn normalized(self) -> Self {
        Self {
            fps: self.fps.clamp(4, 30),
            time: self.time.map(|t| t.min(86_399)),
            effects: self.effects.normalized(),
            shadow_x: self.shadow_x.min(4),
            shadow_y: self.shadow_y.min(4),
            corner_inset: self.corner_inset.min(32),
            inset_x: self.inset_x.min(8),
            inset_y: self.inset_y.min(8),
            ..self
        }
    }
    /// The machine whose effect amounts these are, if any.
    pub fn machine(&self) -> Option<Machine> {
        Machine::ALL
            .into_iter()
            .find(|m| m.visuals().effects == self.effects)
    }
    /// Moving effects with a live clock need a new frame at `fps`; a fixed time is still.
    pub fn animated(&self) -> bool {
        self.time.is_none() && self.effects.animated()
    }
    /// The shader's time: seconds since midnight held at whole frames, or the fixed time.
    pub fn effect_time(&self, seconds_since_midnight: f64) -> f32 {
        match self.time {
            Some(fixed) => fixed as f32,
            None => {
                let fps = f64::from(self.fps.max(1));
                (seconds_since_midnight * fps).floor() as f32 / fps as f32
            }
        }
    }
}
impl CrtEffect {
    /// What an appearance.json from before the Visuals page meant.
    pub fn visuals(self, custom: VisualEffects) -> Visuals {
        match self {
            Self::Wallpaper => Machine::AtariSt.visuals(),
            other => Visuals {
                effects: other.effects(custom),
                ..Visuals::default()
            },
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn machines_bring_shaderwallpaper_render_defaults() {
        let table = [
            (
                Machine::ZxSpectrum,
                12,
                [1.0, 0.025, 0.35, 0.2, 1.0, 1.0, 1.0],
            ),
            (
                Machine::AtariSt,
                14,
                [0.96, 0.014, 0.18, 0.12, 0.55, 0.45, 0.42],
            ),
            (
                Machine::MacSystem2,
                10,
                [0.95, 0.003, 0.0, 0.04, 0.08, 0.06, 0.0],
            ),
            (Machine::GameBoy, 8, [0.92, 0.0, 0.0, 0.28, 0.18, 0.12, 0.0]),
        ];
        for (machine, fps, values) in table {
            let v = machine.visuals();
            assert_eq!(v.fps, fps, "{machine:?}");
            let actual: Vec<f32> = Effect::ALL.iter().map(|e| e.get(&v.effects)).collect();
            assert_eq!(actual, values, "{machine:?}");
            assert_eq!(v.machine(), Some(machine));
        }
        assert_eq!(
            Visuals::for_theme(Theme::ZxSpectrum),
            Machine::ZxSpectrum.visuals()
        );
        assert_eq!(Visuals::for_theme(Theme::Atari), Machine::AtariSt.visuals());
        assert!(Visuals::for_theme(Theme::GreenCrt).effects.is_neutral());
    }
    #[test]
    fn effect_rows_use_the_wallpaper_ranges_and_steps() {
        let rows = [
            (Effect::Brightness, "Brightness", 0.5, 1.6, 22),
            (Effect::Curvature, "Curvature", 0.0, 0.08, 32),
            (
                Effect::ChromaticAberration,
                "Chromatic aberration",
                0.0,
                1.5,
                30,
            ),
            (Effect::Afterglow, "Afterglow", 0.0, 1.2, 24),
            (Effect::Noise, "Noise", 0.0, 2.0, 40),
            (Effect::Flicker, "Flicker", 0.0, 2.0, 40),
            (Effect::Scanlines, "Scanlines", 0.0, 1.25, 25),
        ];
        for (effect, title, low, high, steps) in rows {
            assert_eq!(effect.title(), title);
            assert_eq!(effect.steps(), steps, "{title}");
            assert!((effect.value_at(0) - low).abs() < 1e-6, "{title}");
            assert!((effect.value_at(steps) - high).abs() < 1e-6, "{title}");
            for step in 0..=steps {
                assert_eq!(
                    effect.step_of(effect.value_at(step)),
                    step,
                    "{title} {step}"
                );
            }
        }
        assert!((Effect::Brightness.value_at(9) - 0.95).abs() < 1e-6);
        assert_eq!(Effect::Brightness.label(0.95), "0.95");
        assert_eq!(Effect::Curvature.label(0.0125), "0.0125");
        let mut e = VisualEffects::NEUTRAL;
        Effect::Noise.set(&mut e, 0.55);
        assert_eq!(e.noise, 0.55);
    }
    #[test]
    fn time_follows_the_clock_in_whole_frames_or_stays_fixed() {
        let live = Machine::AtariSt.visuals();
        assert!(live.animated());
        assert_eq!(
            live.effect_time(100.123),
            (100.123f64 * 14.0).floor() as f32 / 14.0
        );
        let fixed = Visuals {
            time: Some(3600),
            ..live
        };
        assert!(!fixed.animated());
        assert_eq!(fixed.effect_time(100.123), 3600.0);
        assert!(!Machine::MacSystem2.visuals().effects.is_neutral());
        assert!(!Visuals::default().animated());
    }
    #[test]
    fn machines_turn_on_the_wallpaper_shadow_and_other_themes_leave_it_off() {
        for machine in Machine::ALL {
            let v = machine.visuals();
            assert_eq!(
                (v.shadow, v.shadow_x, v.shadow_y),
                (true, 1, 1),
                "{machine:?}"
            );
        }
        assert!(!Visuals::for_theme(Theme::GreenCrt).shadow);
        let far = Visuals {
            shadow_x: 9,
            shadow_y: 7,
            ..Visuals::default()
        }
        .normalized();
        assert_eq!((far.shadow_x, far.shadow_y), (4, 4));
    }
    #[test]
    fn machines_frame_panels_with_single_tiles_and_inset_the_text() {
        for machine in Machine::ALL {
            let v = machine.visuals();
            assert_eq!(
                (v.border, v.corner_inset, v.inset_x, v.inset_y),
                (Border::Single, 16, 2, 0),
                "{machine:?}"
            );
        }
        let plain = Visuals::for_theme(Theme::ModernDark);
        assert_eq!(
            (plain.border, plain.inset_x, plain.inset_y),
            (Border::Lines, 0, 0)
        );
        let wide = Visuals {
            corner_inset: 99,
            inset_x: 20,
            inset_y: 9,
            ..Visuals::default()
        }
        .normalized();
        assert_eq!((wide.corner_inset, wide.inset_x, wide.inset_y), (32, 8, 8));
    }
    #[test]
    fn stored_values_are_pulled_into_range() {
        let v = Visuals {
            fps: 99,
            time: Some(999_999),
            ..Visuals::default()
        }
        .normalized();
        assert_eq!((v.fps, v.time), (30, Some(86_399)));
        assert_eq!(Visuals { fps: 1, ..v }.normalized().fps, 4);
    }
    #[test]
    fn files_from_before_the_visuals_page_keep_their_effect() {
        assert_eq!(
            CrtEffect::Wallpaper.visuals(VisualEffects::NEUTRAL),
            Machine::AtariSt.visuals()
        );
        assert!(CrtEffect::Off
            .visuals(VisualEffects::NEUTRAL)
            .effects
            .is_neutral());
        let custom = VisualEffects {
            noise: 0.3,
            ..VisualEffects::NEUTRAL
        };
        assert_eq!(CrtEffect::Custom.visuals(custom).effects, custom);
    }
}
