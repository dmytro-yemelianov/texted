//! The Visuals page: Pixel SSH's CRT pass, ditherGL's pixel separation, vignette and bloom, and
//! ShaderWallpaper's brightness, flicker, machines, timing and chrome. Window rendering reads it;
//! terminals ignore it.
use crate::appearance::Theme;
use serde::{Deserialize, Serialize};
/// ditherGL's pixel separation modes: how each font pixel is cut out of the glass.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Separation {
    #[default]
    Off,
    LcdGrid,
    CrtDots,
    RetroSquare,
    TetrisMatrix,
    LedMatrix,
    GameboyMatrix,
}
impl Separation {
    pub const ALL: [Self; 7] = [
        Self::Off,
        Self::LcdGrid,
        Self::CrtDots,
        Self::RetroSquare,
        Self::TetrisMatrix,
        Self::LedMatrix,
        Self::GameboyMatrix,
    ];
    /// ditherGL's PIXEL_SEPARATION_MODES.
    pub fn name(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::LcdGrid => "LCD Grid",
            Self::CrtDots => "CRT Dots",
            Self::RetroSquare => "Retro Square",
            Self::TetrisMatrix => "Tetris Matrix",
            Self::LedMatrix => "LED Matrix",
            Self::GameboyMatrix => "Gameboy Matrix",
        }
    }
    /// The shader's mode number, ditherGL's `u_pixel_separation_mode`.
    pub fn index(self) -> usize {
        Self::ALL.iter().position(|m| *m == self).unwrap()
    }
}
/// What shows between matrix pixels: ditherGL's black, or the theme's background.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatrixBackground {
    #[default]
    Black,
    Theme,
}
impl MatrixBackground {
    pub const ALL: [Self; 2] = [Self::Black, Self::Theme];
    pub fn name(self) -> &'static str {
        match self {
            Self::Black => "Black",
            Self::Theme => "Theme",
        }
    }
}
/// The phosphor a theme's tube glows with. Pixel SSH's monochrome tubes have no colour triads,
/// so they skip the aberration split, use a dot-pitch mask and tint their snow.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Phosphor {
    #[default]
    Color,
    Amber,
    Green,
    White,
}
impl Phosphor {
    pub fn for_theme(theme: Theme) -> Self {
        match theme {
            Theme::Amber => Self::Amber,
            Theme::GreenCrt => Self::Green,
            Theme::MacSystem2 | Theme::GameBoy => Self::White,
            _ => Self::Color,
        }
    }
    /// The shader's index: 0 colour, 1 amber, 2 green, 3 white.
    pub fn index(self) -> u8 {
        match self {
            Self::Color => 0,
            Self::Amber => 1,
            Self::Green => 2,
            Self::White => 3,
        }
    }
}
/// Screen effect amounts. Neutral values (brightness 1, every amount 0, separation off) draw the
/// exact picture.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct VisualEffects {
    pub brightness: f32,
    pub curvature: f32,
    pub chromatic_aberration: f32,
    pub afterglow: f32,
    pub scanlines: f32,
    pub pixel_grid: f32,
    pub jitter: f32,
    pub antenna_hum: f32,
    pub noise: f32,
    pub flicker: f32,
    pub vignette: f32,
    pub bloom: f32,
    pub separation: Separation,
    pub separation_strength: f32,
    pub matrix_spacing: f32,
    /// ditherGL's inner square size; 0 turns the inner square off.
    pub inner_square: f32,
    pub matrix_background: MatrixBackground,
}
impl Default for VisualEffects {
    fn default() -> Self {
        Self::NEUTRAL
    }
}
impl VisualEffects {
    /// Every amount off; the separation settings hold ditherGL's defaults for when a mode is on.
    pub const NEUTRAL: Self = Self {
        brightness: 1.0,
        curvature: 0.0,
        chromatic_aberration: 0.0,
        afterglow: 0.0,
        scanlines: 0.0,
        pixel_grid: 0.0,
        jitter: 0.0,
        antenna_hum: 0.0,
        noise: 0.0,
        flicker: 0.0,
        vignette: 0.0,
        bloom: 0.0,
        separation: Separation::Off,
        separation_strength: 0.3,
        matrix_spacing: 0.1,
        inner_square: 0.7,
        matrix_background: MatrixBackground::Black,
    };
    /// Pulls every amount into its row's range; a non-number takes the neutral value.
    pub fn normalized(self) -> Self {
        let mut out = self;
        for effect in Effect::ALL {
            let (low, high, _) = effect.range();
            let value = effect.get(&self);
            let fitted = if value.is_finite() {
                value.clamp(low as f32, high as f32)
            } else {
                effect.get(&Self::NEUTRAL)
            };
            effect.set(&mut out, fitted);
        }
        out
    }
    /// Nothing would show: matrix settings only matter once a separation mode is on.
    pub fn is_neutral(&self) -> bool {
        let n = Self::NEUTRAL;
        Self {
            separation_strength: n.separation_strength,
            matrix_spacing: n.matrix_spacing,
            inner_square: n.inner_square,
            matrix_background: n.matrix_background,
            ..*self
        } == n
    }
    /// Snow, flicker, jitter and hum change every frame; without them the picture stays still.
    pub fn animated(&self) -> bool {
        self.noise > 0.0 || self.flicker > 0.0 || self.jitter > 0.0 || self.antenna_hum > 0.0
    }
    /// Pixel SSH's flyback anode breathing, which scales the bent picture slightly over time.
    fn breath(time: f32) -> f32 {
        1.0 + 0.0035 * ((time * 1.7).sin() * (time * 0.43).cos())
    }
    /// The picture point shown at screen point `(u, v)`, 0..1 each, as presentation.wgsl bends it.
    pub(crate) fn bend(&self, (u, v): (f32, f32), time: f32) -> (f32, f32) {
        let b = Self::breath(time);
        let (x, y) = ((u * 2.0 - 1.0) * b, (v * 2.0 - 1.0) * b);
        let k = self.curvature * 0.28;
        let (bx, by) = (x + x * (y / 3.0).powi(2) * k, y + y * (x / 3.0).powi(2) * k);
        ((bx + 1.0) * 0.5, (by + 1.0) * 0.5)
    }
    /// The inverse of `bend` by fixed-point iteration, which converges fast for this gentle bend.
    pub(crate) fn unbend(&self, (u, v): (f32, f32), time: f32) -> (f32, f32) {
        let (tx, ty) = (u * 2.0 - 1.0, v * 2.0 - 1.0);
        let k = self.curvature * 0.28;
        let (mut x, mut y) = (tx, ty);
        for _ in 0..12 {
            (x, y) = (
                tx / (1.0 + k * (y / 3.0).powi(2)),
                ty / (1.0 + k * (x / 3.0).powi(2)),
            );
        }
        let b = Self::breath(time);
        (x / b * 0.5 + 0.5, y / b * 0.5 + 0.5)
    }
    /// What an amount saved by the ShaderWallpaper build means now: its machines become the
    /// retuned machines, and other amounts scale from the wallpaper's ranges to these.
    pub fn from_wallpaper(self) -> Self {
        if let Some(machine) = Machine::ALL
            .into_iter()
            .find(|m| m.wallpaper_effects() == self)
        {
            return machine.visuals().effects;
        }
        Self {
            brightness: self.brightness,
            curvature: self.curvature / 0.08,
            chromatic_aberration: self.chromatic_aberration / 1.5,
            afterglow: self.afterglow / 1.2,
            noise: self.noise / 2.0,
            flicker: self.flicker,
            scanlines: self.scanlines / 1.25,
            ..Self::NEUTRAL
        }
    }
}
/// The CRT effect row of older files: presets, or the `effects` block of appearance.json for Custom.
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
    /// Subtle and Strong stay still; Wallpaper moves like the desktop it came from.
    pub fn effects(self, custom: VisualEffects) -> VisualEffects {
        let neutral = VisualEffects::NEUTRAL;
        match self {
            Self::Off => neutral,
            Self::Subtle => VisualEffects {
                curvature: 0.15,
                chromatic_aberration: 0.1,
                afterglow: 0.1,
                scanlines: 0.25,
                pixel_grid: 0.15,
                ..neutral
            },
            Self::Strong => VisualEffects {
                brightness: 1.05,
                curvature: 0.5,
                chromatic_aberration: 0.35,
                afterglow: 0.35,
                scanlines: 0.6,
                pixel_grid: 0.35,
                ..neutral
            },
            Self::Wallpaper => Machine::AtariSt.visuals().effects,
            Self::Custom => custom.from_wallpaper().normalized(),
        }
    }
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
/// The effect clock repeats every ten minutes: Pixel SSH's hashes take frame numbers that lose
/// their low bits in f32 once a whole day of seconds is multiplied in.
pub const EFFECT_PERIOD: f32 = 600.0;
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
/// Pixel SSH's character gauge: whole cells of █, a ░▒▓ quarter, then a ░ track.
pub fn meter(fraction: f32, cells: usize) -> String {
    let quarters =
        ((fraction.clamp(0.0, 1.0) * (cells * 4) as f32).round() as usize).min(cells * 4);
    let (full, rest) = (quarters / 4, quarters % 4);
    let partial = ["", "░", "▒", "▓"][rest];
    let used = full + usize::from(rest > 0);
    format!("{}{partial}{}", "█".repeat(full), "░".repeat(cells - used))
}
/// One effect row of the Visuals page, with its range and step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    Brightness,
    Curvature,
    ChromaticAberration,
    Afterglow,
    Scanlines,
    PixelGrid,
    Jitter,
    AntennaHum,
    Noise,
    Flicker,
    Vignette,
    Bloom,
    Separation,
    SeparationStrength,
    MatrixSpacing,
    InnerSquare,
    MatrixBackground,
}
impl Effect {
    pub const ALL: [Self; 17] = [
        Self::Brightness,
        Self::Curvature,
        Self::ChromaticAberration,
        Self::Afterglow,
        Self::Scanlines,
        Self::PixelGrid,
        Self::Jitter,
        Self::AntennaHum,
        Self::Noise,
        Self::Flicker,
        Self::Vignette,
        Self::Bloom,
        Self::Separation,
        Self::SeparationStrength,
        Self::MatrixSpacing,
        Self::InnerSquare,
        Self::MatrixBackground,
    ];
    pub fn title(self) -> &'static str {
        match self {
            Self::Brightness => "Brightness",
            Self::Curvature => "Curvature",
            Self::ChromaticAberration => "Aberration",
            Self::Afterglow => "Afterglow",
            Self::Scanlines => "Scanlines",
            Self::PixelGrid => "Pixel grid",
            Self::Jitter => "Signal jitter",
            Self::AntennaHum => "Antenna hum",
            Self::Noise => "Noise snow",
            Self::Flicker => "Flicker",
            Self::Vignette => "Vignette",
            Self::Bloom => "Bloom",
            Self::Separation => "Separation",
            Self::SeparationStrength => "Sep. strength",
            Self::MatrixSpacing => "Matrix spacing",
            Self::InnerSquare => "Inner square",
            Self::MatrixBackground => "Matrix gap",
        }
    }
    /// Titles for compact screens, seven cells at most.
    pub fn short(self) -> &'static str {
        match self {
            Self::Brightness => "Bright",
            Self::Curvature => "Curve",
            Self::ChromaticAberration => "Aberr",
            Self::Afterglow => "Glow",
            Self::Scanlines => "Scan",
            Self::PixelGrid => "Grid",
            Self::Jitter => "Jitter",
            Self::AntennaHum => "Hum",
            Self::Noise => "Snow",
            Self::Flicker => "Flicker",
            Self::Vignette => "Vignet",
            Self::Bloom => "Bloom",
            Self::Separation => "Sep",
            Self::SeparationStrength => "Sep str",
            Self::MatrixSpacing => "Spacing",
            Self::InnerSquare => "Inner",
            Self::MatrixBackground => "Gap",
        }
    }
    /// Low, high and step: Pixel SSH's 0..1 in twentieths, ditherGL's ranges, the wallpaper's
    /// brightness and flicker, and list rows stepping through their entries.
    fn range(self) -> (f64, f64, f64) {
        match self {
            Self::Brightness => (0.5, 1.6, 0.05),
            Self::Flicker | Self::Bloom => (0.0, 2.0, 0.05),
            Self::Vignette => (0.0, 3.0, 0.05),
            Self::Separation => (0.0, 6.0, 1.0),
            Self::MatrixSpacing => (0.0, 0.5, 0.025),
            Self::MatrixBackground => (0.0, 1.0, 1.0),
            _ => (0.0, 1.0, 0.05),
        }
    }
    /// Values a row offers: the range divided into its steps.
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
    /// The row's amount; list rows give their entry's position.
    pub fn get(self, e: &VisualEffects) -> f32 {
        match self {
            Self::Brightness => e.brightness,
            Self::Curvature => e.curvature,
            Self::ChromaticAberration => e.chromatic_aberration,
            Self::Afterglow => e.afterglow,
            Self::Scanlines => e.scanlines,
            Self::PixelGrid => e.pixel_grid,
            Self::Jitter => e.jitter,
            Self::AntennaHum => e.antenna_hum,
            Self::Noise => e.noise,
            Self::Flicker => e.flicker,
            Self::Vignette => e.vignette,
            Self::Bloom => e.bloom,
            Self::Separation => e.separation.index() as f32,
            Self::SeparationStrength => e.separation_strength,
            Self::MatrixSpacing => e.matrix_spacing,
            Self::InnerSquare => e.inner_square,
            Self::MatrixBackground => {
                f32::from(u8::from(e.matrix_background == MatrixBackground::Theme))
            }
        }
    }
    pub fn set(self, e: &mut VisualEffects, value: f32) {
        let entry = |count: usize| (value.max(0.0).round() as usize).min(count - 1);
        let field = match self {
            Self::Separation => {
                e.separation = Separation::ALL[entry(Separation::ALL.len())];
                return;
            }
            Self::MatrixBackground => {
                e.matrix_background = MatrixBackground::ALL[entry(MatrixBackground::ALL.len())];
                return;
            }
            Self::Brightness => &mut e.brightness,
            Self::Curvature => &mut e.curvature,
            Self::ChromaticAberration => &mut e.chromatic_aberration,
            Self::Afterglow => &mut e.afterglow,
            Self::Scanlines => &mut e.scanlines,
            Self::PixelGrid => &mut e.pixel_grid,
            Self::Jitter => &mut e.jitter,
            Self::AntennaHum => &mut e.antenna_hum,
            Self::Noise => &mut e.noise,
            Self::Flicker => &mut e.flicker,
            Self::Vignette => &mut e.vignette,
            Self::Bloom => &mut e.bloom,
            Self::SeparationStrength => &mut e.separation_strength,
            Self::MatrixSpacing => &mut e.matrix_spacing,
            Self::InnerSquare => &mut e.inner_square,
        };
        *field = value;
    }
    pub fn label(self, value: f32) -> String {
        let mut e = VisualEffects::NEUTRAL;
        match self {
            Self::Separation => {
                self.set(&mut e, value);
                e.separation.name().into()
            }
            Self::MatrixBackground => {
                self.set(&mut e, value);
                e.matrix_background.name().into()
            }
            Self::InnerSquare if value <= 0.0 => "Off".into(),
            Self::MatrixSpacing => format!("{value:.3}"),
            _ => format!("{value:.2}"),
        }
    }
    /// A gauge of `cells` for numeric rows; list rows have none.
    pub fn meter(self, value: f32, cells: usize) -> Option<String> {
        if matches!(self, Self::Separation | Self::MatrixBackground) {
            return None;
        }
        let (low, high, _) = self.range();
        Some(meter(
            ((f64::from(value) - low) / (high - low)) as f32,
            cells,
        ))
    }
}
/// ShaderWallpaper's machine presets with their render defaults, retuned for these effects.
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
    /// The amounts ShaderWallpaper's own shader used, as the previous build saved them.
    fn wallpaper_effects(self) -> VisualEffects {
        let [brightness, curvature, chromatic_aberration, afterglow, noise, flicker, scanlines] =
            match self {
                Self::ZxSpectrum => [1.0, 0.025, 0.35, 0.2, 1.0, 1.0, 1.0],
                Self::AtariSt => [0.96, 0.014, 0.18, 0.12, 0.55, 0.45, 0.42],
                Self::MacSystem2 => [0.95, 0.003, 0.0, 0.04, 0.08, 0.06, 0.0],
                Self::GameBoy => [0.92, 0.0, 0.0, 0.28, 0.18, 0.12, 0.0],
            };
        VisualEffects {
            brightness,
            curvature,
            chromatic_aberration,
            afterglow,
            noise,
            flicker,
            scanlines,
            ..VisualEffects::NEUTRAL
        }
    }
    /// ShaderWallpaper's frame rate and chrome, with each machine's look rebuilt from Pixel SSH's
    /// and ditherGL's effects: colour tubes get the grille, the Mac a mono tube, the Game Boy an LCD.
    pub fn visuals(self) -> Visuals {
        let n = VisualEffects::NEUTRAL;
        let (fps, effects) = match self {
            Self::ZxSpectrum => (
                12,
                VisualEffects {
                    curvature: 0.35,
                    chromatic_aberration: 0.35,
                    afterglow: 0.2,
                    scanlines: 0.6,
                    pixel_grid: 0.3,
                    jitter: 0.1,
                    antenna_hum: 0.15,
                    noise: 0.25,
                    flicker: 1.0,
                    ..n
                },
            ),
            Self::AtariSt => (
                14,
                VisualEffects {
                    brightness: 0.96,
                    curvature: 0.2,
                    chromatic_aberration: 0.18,
                    afterglow: 0.12,
                    scanlines: 0.35,
                    pixel_grid: 0.2,
                    jitter: 0.05,
                    noise: 0.1,
                    flicker: 0.45,
                    ..n
                },
            ),
            Self::MacSystem2 => (
                10,
                VisualEffects {
                    brightness: 0.95,
                    curvature: 0.05,
                    afterglow: 0.05,
                    pixel_grid: 0.1,
                    noise: 0.03,
                    flicker: 0.06,
                    ..n
                },
            ),
            Self::GameBoy => (
                8,
                VisualEffects {
                    brightness: 0.92,
                    afterglow: 0.28,
                    flicker: 0.12,
                    separation: Separation::GameboyMatrix,
                    matrix_background: MatrixBackground::Theme,
                    ..n
                },
            ),
        };
        Visuals {
            fps,
            effects,
            // WallpaperChromeEffectRegistry defaults: shadow 1,1, single tiles, inset 2,0.
            shadow: true,
            border: Border::Single,
            inset_x: 2,
            ..Visuals::default()
        }
    }
}
/// Pixel SSH's Visuals presets; they set the effects and leave timing and chrome alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Look {
    PixelSsh,
    Crt,
    Arcade,
    Bloom,
    Glitch,
}
impl Look {
    pub const ALL: [Self; 5] = [
        Self::PixelSsh,
        Self::Crt,
        Self::Arcade,
        Self::Bloom,
        Self::Glitch,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Self::PixelSsh => "Pixel SSH",
            Self::Crt => "CRT",
            Self::Arcade => "Arcade",
            Self::Bloom => "Bloom",
            Self::Glitch => "Glitch",
        }
    }
    /// Scanlines, grid, aberration, afterglow, curvature and jitter from pixel-ssh's view crate.
    pub fn effects(self) -> VisualEffects {
        let [scanlines, pixel_grid, chromatic_aberration, afterglow, curvature, jitter] = match self
        {
            Self::PixelSsh => [0.25, 0.16, 0.0, 0.15, 0.0, 0.0],
            Self::Crt => [0.35, 0.20, 0.15, 0.25, 0.20, 0.0],
            Self::Arcade => [0.55, 0.35, 0.45, 0.30, 0.35, 0.15],
            Self::Bloom => [0.65, 0.40, 0.0, 0.75, 0.20, 0.05],
            Self::Glitch => [0.75, 0.50, 0.90, 0.40, 0.40, 0.50],
        };
        VisualEffects {
            scanlines,
            pixel_grid,
            chromatic_aberration,
            afterglow,
            curvature,
            jitter,
            ..VisualEffects::NEUTRAL
        }
    }
}
/// A Preset row entry: a machine with its timing and chrome, or a look that only sets effects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preset {
    Machine(Machine),
    Look(Look),
}
impl Preset {
    pub fn name(self) -> &'static str {
        match self {
            Self::Machine(m) => m.name(),
            Self::Look(l) => l.name(),
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
/// The effect amounts' meaning: 1 is ShaderWallpaper's shader, 2 this one.
const EFFECTS_MODEL: u8 = 2;
fn wallpaper_model() -> u8 {
    1
}
/// Everything the Visuals page sets; saved in appearance.json.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Visuals {
    /// Which shader the effect amounts were set for; files without it predate this one.
    #[serde(default = "wallpaper_model")]
    pub effects_model: u8,
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
            effects_model: EFFECTS_MODEL,
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
            Theme::MacSystem2 => Machine::MacSystem2.visuals(),
            Theme::GameBoy => Machine::GameBoy.visuals(),
            _ => Self::default(),
        }
    }
    /// These settings with amounts saved for ShaderWallpaper's shader moved to this one.
    pub fn current(self) -> Self {
        if self.effects_model >= EFFECTS_MODEL {
            return self;
        }
        Self {
            effects_model: EFFECTS_MODEL,
            effects: self.effects.from_wallpaper(),
            ..self
        }
    }
    pub fn normalized(self) -> Self {
        let v = self.current();
        Self {
            fps: v.fps.clamp(4, 30),
            time: v.time.map(|t| t.min(86_399)),
            effects: v.effects.normalized(),
            shadow_x: v.shadow_x.min(4),
            shadow_y: v.shadow_y.min(4),
            corner_inset: v.corner_inset.min(32),
            inset_x: v.inset_x.min(8),
            inset_y: v.inset_y.min(8),
            ..v
        }
    }
    /// The machine whose effect amounts these are, if any.
    pub fn machine(&self) -> Option<Machine> {
        Machine::ALL
            .into_iter()
            .find(|m| m.visuals().effects == self.effects)
    }
    /// The Preset row entry these amounts match: a machine first, then a look.
    pub fn preset(&self) -> Option<Preset> {
        self.machine().map(Preset::Machine).or_else(|| {
            Look::ALL
                .into_iter()
                .find(|l| l.effects() == self.effects)
                .map(Preset::Look)
        })
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
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn effect_rows_use_pixel_ssh_dithergl_and_wallpaper_ranges() {
        let rows = [
            (Effect::Brightness, "Brightness", 0.5, 1.6, 22),
            (Effect::Curvature, "Curvature", 0.0, 1.0, 20),
            (Effect::ChromaticAberration, "Aberration", 0.0, 1.0, 20),
            (Effect::Afterglow, "Afterglow", 0.0, 1.0, 20),
            (Effect::Scanlines, "Scanlines", 0.0, 1.0, 20),
            (Effect::PixelGrid, "Pixel grid", 0.0, 1.0, 20),
            (Effect::Jitter, "Signal jitter", 0.0, 1.0, 20),
            (Effect::AntennaHum, "Antenna hum", 0.0, 1.0, 20),
            (Effect::Noise, "Noise snow", 0.0, 1.0, 20),
            (Effect::Flicker, "Flicker", 0.0, 2.0, 40),
            (Effect::Vignette, "Vignette", 0.0, 3.0, 60),
            (Effect::Bloom, "Bloom", 0.0, 2.0, 40),
            (Effect::Separation, "Separation", 0.0, 6.0, 6),
            (Effect::SeparationStrength, "Sep. strength", 0.0, 1.0, 20),
            (Effect::MatrixSpacing, "Matrix spacing", 0.0, 0.5, 20),
            (Effect::InnerSquare, "Inner square", 0.0, 1.0, 20),
            (Effect::MatrixBackground, "Matrix gap", 0.0, 1.0, 1),
        ];
        assert_eq!(Effect::ALL.len(), rows.len());
        for ((effect, title, low, high, steps), listed) in rows.into_iter().zip(Effect::ALL) {
            assert_eq!(effect, listed);
            assert_eq!(effect.title(), title);
            assert!(effect.short().chars().count() <= 7, "{title}");
            assert!(effect.title().chars().count() <= 14, "{title}");
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
        assert_eq!(Effect::Brightness.label(0.95), "0.95");
        assert_eq!(Effect::MatrixSpacing.label(0.125), "0.125");
        assert_eq!(Effect::InnerSquare.label(0.0), "Off");
        assert_eq!(Effect::InnerSquare.label(0.7), "0.70");
        let names: Vec<String> = (0..=6)
            .map(|s| Effect::Separation.label(s as f32))
            .collect();
        assert_eq!(
            names,
            [
                "Off",
                "LCD Grid",
                "CRT Dots",
                "Retro Square",
                "Tetris Matrix",
                "LED Matrix",
                "Gameboy Matrix"
            ]
        );
        assert_eq!(Effect::MatrixBackground.label(1.0), "Theme");
        let mut e = VisualEffects::NEUTRAL;
        Effect::Separation.set(&mut e, 6.0);
        Effect::MatrixBackground.set(&mut e, 1.0);
        Effect::Jitter.set(&mut e, 0.4);
        assert_eq!(
            (e.separation, e.matrix_background, e.jitter),
            (Separation::GameboyMatrix, MatrixBackground::Theme, 0.4)
        );
        assert_eq!(Effect::Separation.get(&e), 6.0);
    }
    #[test]
    fn numeric_rows_draw_pixel_ssh_gauges() {
        assert_eq!(meter(0.0, 5), "░░░░░");
        assert_eq!(meter(0.35, 5), "█▓░░░");
        assert_eq!(meter(0.5, 4), "██░░");
        assert_eq!(meter(1.0, 5), "█████");
        assert_eq!(meter(7.0, 3), "███");
        assert_eq!(Effect::Brightness.meter(1.6, 5).as_deref(), Some("█████"));
        assert_eq!(Effect::Flicker.meter(1.0, 4).as_deref(), Some("██░░"));
        assert_eq!(Effect::Separation.meter(2.0, 5), None);
        assert_eq!(Effect::MatrixBackground.meter(1.0, 5), None);
    }
    #[test]
    fn looks_are_pixel_ssh_presets() {
        // Scanlines, grid, aberration, afterglow, curvature, jitter, as pixel-ssh's view crate sets them.
        let table = [
            (
                Look::PixelSsh,
                "Pixel SSH",
                [0.25, 0.16, 0.0, 0.15, 0.0, 0.0],
            ),
            (Look::Crt, "CRT", [0.35, 0.20, 0.15, 0.25, 0.20, 0.0]),
            (Look::Arcade, "Arcade", [0.55, 0.35, 0.45, 0.30, 0.35, 0.15]),
            (Look::Bloom, "Bloom", [0.65, 0.40, 0.0, 0.75, 0.20, 0.05]),
            (Look::Glitch, "Glitch", [0.75, 0.50, 0.90, 0.40, 0.40, 0.50]),
        ];
        assert_eq!(Look::ALL.len(), table.len());
        for (look, name, [scan, grid, aberration, glow, curve, jitter]) in table {
            let e = look.effects();
            assert_eq!(look.name(), name);
            assert_eq!(
                VisualEffects {
                    scanlines: scan,
                    pixel_grid: grid,
                    chromatic_aberration: aberration,
                    afterglow: glow,
                    curvature: curve,
                    jitter,
                    ..VisualEffects::NEUTRAL
                },
                e,
                "{name}"
            );
            let v = Visuals {
                effects: e,
                ..Visuals::default()
            };
            assert_eq!(v.preset(), Some(Preset::Look(look)));
        }
        assert!(Look::Glitch.effects().animated());
        assert!(!Look::Crt.effects().animated());
    }
    #[test]
    fn machines_keep_wallpaper_render_defaults_with_retuned_effects() {
        for (machine, fps) in [
            (Machine::ZxSpectrum, 12),
            (Machine::AtariSt, 14),
            (Machine::MacSystem2, 10),
            (Machine::GameBoy, 8),
        ] {
            let v = machine.visuals();
            assert_eq!(v.fps, fps, "{machine:?}");
            assert_eq!(v.machine(), Some(machine));
            assert_eq!(v.preset(), Some(Preset::Machine(machine)));
            assert_eq!(
                v.effects,
                v.effects.normalized(),
                "{machine:?} stays in range"
            );
            assert_eq!(
                (
                    v.shadow,
                    v.shadow_x,
                    v.shadow_y,
                    v.border,
                    v.corner_inset,
                    v.inset_x,
                    v.inset_y
                ),
                (true, 1, 1, Border::Single, 16, 2, 0),
                "{machine:?}"
            );
        }
        // Colour tubes get Pixel SSH's grille; the Game Boy's LCD gets ditherGL's matrix.
        assert!(Machine::ZxSpectrum.visuals().effects.pixel_grid > 0.0);
        assert!(Machine::AtariSt.visuals().effects.chromatic_aberration > 0.0);
        let lcd = Machine::GameBoy.visuals().effects;
        assert_eq!(
            (lcd.separation, lcd.matrix_background, lcd.scanlines),
            (Separation::GameboyMatrix, MatrixBackground::Theme, 0.0)
        );
        assert_eq!(
            Visuals::for_theme(Theme::ZxSpectrum),
            Machine::ZxSpectrum.visuals()
        );
        assert_eq!(Visuals::for_theme(Theme::Atari), Machine::AtariSt.visuals());
        assert_eq!(
            Visuals::for_theme(Theme::MacSystem2),
            Machine::MacSystem2.visuals()
        );
        assert_eq!(
            Visuals::for_theme(Theme::GameBoy),
            Machine::GameBoy.visuals()
        );
        let plain = Visuals::for_theme(Theme::GreenCrt);
        assert!(plain.effects.is_neutral() && !plain.shadow);
        assert_eq!((plain.border, plain.inset_x), (Border::Lines, 0));
        assert_eq!(plain.preset(), None);
    }
    #[test]
    fn monochrome_tubes_glow_in_their_phosphor() {
        assert_eq!(Phosphor::for_theme(Theme::Amber), Phosphor::Amber);
        assert_eq!(Phosphor::for_theme(Theme::GreenCrt), Phosphor::Green);
        assert_eq!(Phosphor::for_theme(Theme::MacSystem2), Phosphor::White);
        assert_eq!(Phosphor::for_theme(Theme::GameBoy), Phosphor::White);
        assert_eq!(Phosphor::for_theme(Theme::Atari), Phosphor::Color);
        let indices: Vec<u8> = [
            Phosphor::Color,
            Phosphor::Amber,
            Phosphor::Green,
            Phosphor::White,
        ]
        .map(Phosphor::index)
        .into();
        assert_eq!(indices, [0, 1, 2, 3]);
    }
    #[test]
    fn neutral_means_nothing_visible_and_moving_effects_animate() {
        assert!(VisualEffects::NEUTRAL.is_neutral());
        // Matrix settings only matter once a separation mode is on.
        let idle = VisualEffects {
            matrix_spacing: 0.3,
            inner_square: 0.0,
            separation_strength: 1.0,
            matrix_background: MatrixBackground::Theme,
            ..VisualEffects::NEUTRAL
        };
        assert!(idle.is_neutral());
        assert!(!VisualEffects {
            separation: Separation::CrtDots,
            ..idle
        }
        .is_neutral());
        for moving in [
            VisualEffects {
                noise: 0.1,
                ..VisualEffects::NEUTRAL
            },
            VisualEffects {
                flicker: 0.1,
                ..VisualEffects::NEUTRAL
            },
            VisualEffects {
                jitter: 0.1,
                ..VisualEffects::NEUTRAL
            },
            VisualEffects {
                antenna_hum: 0.1,
                ..VisualEffects::NEUTRAL
            },
        ] {
            assert!(moving.animated(), "{moving:?}");
        }
        assert!(!VisualEffects {
            bloom: 1.0,
            vignette: 1.0,
            pixel_grid: 1.0,
            ..VisualEffects::NEUTRAL
        }
        .animated());
    }
    #[test]
    fn curvature_bends_like_pixel_ssh_and_unbends_back() {
        let e = VisualEffects {
            curvature: 1.0,
            ..VisualEffects::NEUTRAL
        };
        assert_eq!(e.bend((0.5, 0.5), 0.0), (0.5, 0.5));
        // Pixel SSH: uv = cc + cc * (cc.yx / 3)^2 * curvature * 0.28.
        let (u, v) = e.bend((0.0, 0.0), 0.0);
        let edge = -1.0 - 0.28 / 9.0;
        assert!((u - (edge + 1.0) * 0.5).abs() < 1e-6 && (v - (edge + 1.0) * 0.5).abs() < 1e-6);
        for time in [0.0, 1.0, 1234.5] {
            for p in [(0.1, 0.2), (0.9, 0.05), (0.5, 0.97)] {
                let q = e.bend(e.unbend(p, time), time);
                assert!(
                    (p.0 - q.0).abs() < 1e-5 && (p.1 - q.1).abs() < 1e-5,
                    "{p:?} {time}"
                );
            }
        }
        // The anode breathes with time, so the mapping follows the frame it was drawn at.
        assert_ne!(e.bend((0.1, 0.1), 1.0), e.bend((0.1, 0.1), 0.0));
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
        assert!(!Visuals::default().animated());
    }
    #[test]
    fn stored_values_are_pulled_into_range() {
        let v = Visuals {
            fps: 99,
            time: Some(999_999),
            shadow_x: 9,
            shadow_y: 7,
            corner_inset: 99,
            inset_x: 20,
            inset_y: 9,
            effects: VisualEffects {
                brightness: 9.0,
                curvature: -1.0,
                noise: f32::NAN,
                vignette: 7.0,
                matrix_spacing: 2.0,
                ..VisualEffects::NEUTRAL
            },
            ..Visuals::default()
        }
        .normalized();
        assert_eq!((v.fps, v.time), (30, Some(86_399)));
        assert_eq!(
            (v.shadow_x, v.shadow_y, v.corner_inset, v.inset_x, v.inset_y),
            (4, 4, 32, 8, 8)
        );
        let e = v.effects;
        assert_eq!(
            (
                e.brightness,
                e.curvature,
                e.noise,
                e.vignette,
                e.matrix_spacing
            ),
            (1.6, 0.0, 0.0, 3.0, 0.5)
        );
        assert_eq!(Visuals { fps: 1, ..v }.normalized().fps, 4);
    }
    #[test]
    fn wallpaper_amounts_from_older_files_move_to_the_new_scales() {
        // Saved by the previous build: no model marker, ShaderWallpaper's Atari ST amounts.
        let atari: Visuals = serde_json::from_str(
            r#"{"fps":14,"effects":{"brightness":0.96,"curvature":0.014,"chromatic_aberration":0.18,
            "afterglow":0.12,"noise":0.55,"flicker":0.45,"scanlines":0.42},"shadow":true,
            "border":"single","inset_x":2}"#,
        )
        .unwrap();
        assert_eq!(atari.current().effects, Machine::AtariSt.visuals().effects);
        assert_eq!(
            atari.normalized().preset(),
            Some(Preset::Machine(Machine::AtariSt))
        );
        let custom: Visuals = serde_json::from_str(
            r#"{"effects":{"brightness":1.1,"curvature":0.04,"chromatic_aberration":0.75,
            "afterglow":0.6,"noise":1.0,"flicker":0.3,"scanlines":0.625}}"#,
        )
        .unwrap();
        let e = custom.normalized().effects;
        assert_eq!(
            (
                e.brightness,
                e.curvature,
                e.chromatic_aberration,
                e.afterglow,
                e.noise,
                e.flicker,
                e.scanlines
            ),
            (1.1, 0.5, 0.5, 0.5, 0.5, 0.3, 0.5)
        );
        // Files from this build round-trip unchanged.
        let saved = serde_json::to_string(&Machine::GameBoy.visuals()).unwrap();
        let back: Visuals = serde_json::from_str(&saved).unwrap();
        assert_eq!(back.current(), Machine::GameBoy.visuals());
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
        for crt in [CrtEffect::Subtle, CrtEffect::Strong] {
            let e = crt.effects(VisualEffects::NEUTRAL);
            assert!(
                !e.is_neutral() && !e.animated() && e == e.normalized(),
                "{crt:?}"
            );
        }
        // The old top-level `effects` block holds wallpaper amounts.
        let custom = VisualEffects {
            noise: 0.3,
            ..VisualEffects::NEUTRAL
        };
        assert_eq!(CrtEffect::Custom.visuals(custom).effects.noise, 0.15);
    }
}
