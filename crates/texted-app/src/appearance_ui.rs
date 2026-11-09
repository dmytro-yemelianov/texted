//! Appearance panel interaction and drawing, sharing native and compact framebuffer geometry.
use crate::{
    dropdown::{Dropdown, Popup},
    graphics::{draw_box, draw_rect, DrawList, FontEngine},
};
use std::{cell::RefCell, sync::Arc};
use texted_app::{
    appearance::{Appearance, FontFamily, MouseCursor, Palette, Theme},
    framebuffer::Framebuffer,
    input::KeyInput,
    visuals::{Effect, Look, Machine, Preset, Visuals},
};
use winit::keyboard::{Key, NamedKey};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Rect {
    pub x: usize,
    pub y: usize,
    pub w: usize,
    pub h: usize,
}
impl Rect {
    pub fn contains(self, x: f32, y: f32) -> bool {
        x >= self.x as f32
            && y >= self.y as f32
            && x < (self.x + self.w) as f32
            && y < (self.y + self.h) as f32
    }
}

/// Style fields: theme, screen, font, size, spacing and cursor.
const STYLE_FIELDS: usize = 6;
/// Visuals fields, as in ShaderWallpaper: machine preset, frame rate, time, then the effects.
const PRESET: usize = STYLE_FIELDS;
const FRAME_RATE: usize = STYLE_FIELDS + 1;
const TIME: usize = STYLE_FIELDS + 2;
const FIRST_EFFECT: usize = STYLE_FIELDS + 3;
/// Chrome: ShaderWallpaper's text and border shadow.
const SHADOW: usize = FIRST_EFFECT + Effect::ALL.len();
const SHADOW_X: usize = SHADOW + 1;
const SHADOW_Y: usize = SHADOW + 2;
/// Chrome: ShaderWallpaper's border tiles and panel inset.
const BORDER: usize = SHADOW + 3;
const CORNER: usize = BORDER + 1;
const INSET_X: usize = BORDER + 2;
const INSET_Y: usize = BORDER + 3;
/// The lace layer's rows.
const LACE: usize = INSET_Y + 1;
const LACE_DENSITY: usize = INSET_Y + 2;
const LACE_STRENGTH: usize = INSET_Y + 3;
const LACE_SPACING: usize = INSET_Y + 4;
const LACE_SERIFS: usize = INSET_Y + 5;
const LACE_SEED: usize = INSET_Y + 6;
const FIELDS: usize = LACE_SEED + 1;
/// The most rows shown at once; small screens show fewer, and pages scroll.
const ROWS: usize = 7;
struct Panel {
    rect: Rect,
    input: usize,
    row_top: usize,
    step: usize,
    row_h: usize,
    pad: usize,
    buttons: usize,
    /// Field rows that fit this screen, 1 to ROWS.
    rows: usize,
}
impl Panel {
    fn new(width: usize, height: usize, compact: bool) -> Self {
        // `tail` holds the buttons and the status line below the rows.
        let (limit, tail, margin, input, row_top, step, row_h, pad) = if compact {
            (400, 32, 8, 72, 32, 16, 16, 8)
        } else {
            (704, 80, 16, 152, 64, 32, 32, 16)
        };
        let rows = (height.saturating_sub(row_top + tail) / step).clamp(1, ROWS);
        let h = row_top + rows * step + tail;
        let w = width.saturating_sub(margin).min(limit) / 8 * 8;
        Self {
            rect: Rect {
                x: width.saturating_sub(w) / 2 / 8 * 8,
                y: height.saturating_sub(h) / 2 / if compact { 8 } else { 16 }
                    * if compact { 8 } else { 16 },
                w,
                h,
            },
            input,
            row_top,
            step,
            row_h,
            pad,
            buttons: row_top + rows * step + if compact { 0 } else { 16 },
            rows,
        }
    }
    fn placed(width: usize, height: usize, compact: bool, position: Option<(f32, f32)>) -> Self {
        let mut p = Self::new(width, height, compact);
        if let Some((x, y)) = position {
            p.rect.x = x.max(0.0).min(width.saturating_sub(p.rect.w) as f32) as usize / 8 * 8;
            p.rect.y = y.max(0.0).min(height.saturating_sub(p.rect.h) as f32) as usize
                / if compact { 8 } else { 16 }
                * if compact { 8 } else { 16 };
        }
        p
    }
    fn row(&self, row: usize) -> Rect {
        Rect {
            x: self.rect.x + self.input,
            y: self.rect.y + self.row_top + row * self.step,
            w: self.rect.w.saturating_sub(self.input + self.pad),
            h: self.row_h,
        }
    }
    fn done(&self) -> Rect {
        Rect {
            x: self.rect.x + self.rect.w.saturating_sub(4 * self.pad + 48),
            y: self.rect.y + self.buttons,
            w: 48 + 3 * self.pad,
            h: self.row_h,
        }
    }
    fn reset(&self) -> Rect {
        let x = self.rect.x + self.pad;
        // Narrow screens shrink Reset so it stops a pad short of Done.
        let room = self.done().x.saturating_sub(x + self.pad);
        Rect {
            x,
            y: self.rect.y + self.buttons,
            w: (if self.pad == 8 { 96 } else { 168 }).min(room),
            h: self.row_h,
        }
    }
    /// The Style and Visuals page tabs in the title row.
    fn tab(&self, visuals: bool) -> Rect {
        let compact = self.pad == 8;
        let (style_w, visuals_w, h) = if compact { (48, 64, 16) } else { (72, 96, 28) };
        Rect {
            x: self.rect.x + self.pad + if visuals { style_w } else { 0 },
            y: self.rect.y + if compact { 2 } else { 10 },
            w: if visuals { visuals_w } else { style_w },
            h,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hit {
    Close,
    Reset,
    /// A row slot; the page and its scroll decide which field it shows.
    Field(usize),
    Page {
        visuals: bool,
    },
}
#[cfg(test)]
fn hit(width: usize, height: usize, x: f32, y: f32, compact: bool) -> Option<Hit> {
    hit_panel(&Panel::new(width, height, compact), x, y)
}
fn hit_panel(p: &Panel, x: f32, y: f32) -> Option<Hit> {
    if !p.rect.contains(x, y)
        || (y < (p.rect.y + if p.pad == 8 { 24 } else { 36 }) as f32
            && x > (p.rect.x + p.rect.w.saturating_sub(28)) as f32)
    {
        return Some(Hit::Close);
    }
    for visuals in [false, true] {
        if p.tab(visuals).contains(x, y) {
            return Some(Hit::Page { visuals });
        }
    }
    for row in 0..p.rows {
        let r = p.row(row);
        if r.contains(x, y) {
            return Some(Hit::Field(row));
        }
    }
    if p.reset().contains(x, y) {
        Some(Hit::Reset)
    } else if p.done().contains(x, y) {
        Some(Hit::Close)
    } else {
        None
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Value {
    Theme(Theme),
    Screen(Framebuffer),
    Font(FontFamily),
    Size(u16),
    Spacing(u16),
    Cursor(MouseCursor),
    Preset(Option<Preset>),
    /// Effects that match no preset; no list entry equals it.
    Custom,
    Fps(u8),
    Time(Option<u32>),
    /// An effect by its Effect::ALL index, at a step of its range.
    Effect(usize, u16),
    Shadow(bool),
    /// A shadow offset in font pixels; `true` is horizontal.
    Offset(bool, u8),
    Border(texted_app::visuals::Border),
    Corner(u8),
    /// A panel inset in font pixels; `true` is horizontal.
    Inset(bool, u8),
    Lace(bool),
    /// Lace density in twentieths, 0 to 20.
    LaceDensity(u8),
    /// Lace strength in twentieths, 1 to 20.
    LaceStrength(u8),
    LaceSpacing(u8),
    LaceSerifs(bool),
    LaceSeed(u8),
}
struct Choice {
    label: String,
    value: Value,
    enabled: bool,
}
impl Choice {
    fn new(label: String, value: Value) -> Self {
        Self {
            label,
            value,
            enabled: true,
        }
    }
}
pub(crate) struct AppearanceUi {
    pub settings: Appearance,
    pub palette: Palette,
    pub open: bool,
    cell_backend: bool,
    terminal_graphics: bool,
    pub save_status: String,
    focus: usize,
    /// The first field of the current page shown in the top row.
    scroll: usize,
    /// Rows the panel showed last; small screens show fewer than ROWS.
    rows: std::cell::Cell<usize>,
    dropdown: Option<Dropdown>,
    font_query: String,
    origin: Option<Appearance>,
    accepted: bool,
    position: Option<(f32, f32)>,
    drag: Option<(f32, f32, f32, f32)>,
    previews: Vec<(FontFamily, FontEngine, u64)>,
    preview_clock: u64,
    font_choices: RefCell<Option<(String, Arc<Vec<Choice>>)>>,
    /// Why the current face cannot carry lace, if it cannot.
    lace_unavailable: Option<String>,
}
impl AppearanceUi {
    /// Tells the Lace row whether the face can carry lace; `Some(reason)` blocks turning it on.
    #[allow(
        dead_code,
        reason = "the lace worker reports the face's verdict from Task 7"
    )]
    pub fn set_lace_unavailable(&mut self, reason: Option<String>) {
        self.lace_unavailable = reason;
    }
    pub fn new(mut settings: Appearance) -> Self {
        settings.typography.normalize();
        settings.pixel_typography.normalize();
        let palette = settings
            .palette()
            .expect("Appearance was validated at load");
        Self {
            settings,
            palette,
            open: false,
            cell_backend: false,
            terminal_graphics: false,
            save_status: "Changes save automatically".into(),
            focus: 0,
            scroll: 0,
            rows: std::cell::Cell::new(ROWS),
            dropdown: None,
            font_query: String::new(),
            origin: None,
            accepted: false,
            position: None,
            drag: None,
            previews: Vec::new(),
            preview_clock: 0,
            font_choices: RefCell::new(None),
            lace_unavailable: None,
        }
    }
    pub fn set_terminal_graphics(&mut self, enabled: bool) {
        self.terminal_graphics = enabled;
    }
    pub fn set_cell_backend(&mut self, enabled: bool) {
        self.cell_backend = enabled;
        if enabled {
            self.settings.framebuffer = Framebuffer::Native;
            self.settings.typography = Default::default();
            self.settings.pixel_typography = Default::default();
        }
    }
    pub fn set_open(&mut self, open: bool) {
        self.cancel_preview();
        self.open = open;
        self.drag = None;
    }
    fn cancel_preview(&mut self) {
        if let Some(origin) = self.origin.take() {
            self.settings = origin;
            self.palette = self.settings.palette().unwrap();
        }
        self.dropdown = None;
    }
    fn accept_preview(&mut self) {
        self.origin = None;
        self.dropdown = None;
        self.accepted = true;
    }
    pub fn take_accepted(&mut self) -> bool {
        std::mem::take(&mut self.accepted)
    }
    pub fn previewing(&self) -> bool {
        self.origin.is_some()
    }
    pub fn accept_and_close(&mut self) {
        self.accept_preview();
        self.open = false;
        self.drag = None;
    }
    pub fn reset_defaults(&mut self) {
        let open = self.open;
        let status = self.save_status.clone();
        let cell_backend = self.cell_backend;
        let terminal_graphics = self.terminal_graphics;
        *self = Self::new(Appearance::default());
        self.set_cell_backend(cell_backend);
        self.set_terminal_graphics(terminal_graphics);
        self.open = open;
        self.save_status = status;
    }
    fn visuals_page(&self) -> bool {
        self.focus >= STYLE_FIELDS
    }
    fn page(&self) -> (usize, usize) {
        if self.visuals_page() {
            (STYLE_FIELDS, FIELDS)
        } else {
            (0, STYLE_FIELDS)
        }
    }
    /// The field a row slot shows on the current page.
    fn field_at(&self, slot: usize) -> Option<usize> {
        let (start, end) = self.page();
        let field = start + self.scroll + slot;
        (slot < self.rows.get() && field < end).then_some(field)
    }
    fn slot_of(&self, field: usize) -> Option<usize> {
        (0..self.rows.get()).find(|&slot| self.field_at(slot) == Some(field))
    }
    /// Focuses a field, turning to its page and scrolling it into view.
    fn set_focus(&mut self, field: usize) {
        let page = self.visuals_page();
        self.focus = field.min(FIELDS - 1);
        if page != self.visuals_page() {
            self.scroll = 0;
        }
        let rows = self.rows.get();
        let index = self.focus - self.page().0;
        if index < self.scroll {
            self.scroll = index;
        } else if index >= self.scroll + rows {
            self.scroll = index + 1 - rows;
        }
    }
    fn step_focus(&mut self, back: bool) {
        self.set_focus((self.focus + if back { FIELDS - 1 } else { 1 }) % FIELDS);
    }
    fn visuals_mut(&mut self) -> &mut Visuals {
        let current = self.settings.visuals();
        self.settings.visuals.get_or_insert(current)
    }
    /// On the Visuals page, the theme's visuals; elsewhere, every appearance default.
    pub fn reset(&mut self) {
        if self.visuals_page() {
            self.cancel_preview();
            let (time, lace) = {
                let v = self.settings.visuals();
                (v.time, v.lace)
            };
            self.settings.visuals = Some(Visuals {
                time,
                lace,
                ..Visuals::for_theme(self.settings.theme)
            });
        } else {
            self.reset_defaults();
        }
    }
    pub fn browse(&mut self, forward: bool, width: usize, height: usize, compact: bool) {
        if self.dropdown.is_none() {
            self.open_dropdown(self.focus, width, height, compact);
        }
        let key = Key::Named(if forward {
            NamedKey::ArrowDown
        } else {
            NamedKey::ArrowUp
        });
        let input = KeyInput {
            physical: winit::keyboard::PhysicalKey::Unidentified(
                winit::keyboard::NativeKeyCode::Unidentified,
            ),
            logical: &key,
            text: None,
            modifiers: winit::keyboard::ModifiersState::empty(),
        };
        self.key(&input, width, height, compact);
    }
    pub fn end_drag(&mut self) {
        self.drag = None;
    }
    pub fn dismiss_dropdown(&mut self) -> bool {
        let had = self.dropdown.is_some();
        self.cancel_preview();
        had
    }
    fn panel(&self, width: usize, height: usize, compact: bool) -> Panel {
        let p = Panel::placed(width, height, compact, self.position);
        self.rows.set(p.rows);
        p
    }
    pub fn cursor(
        &self,
        width: usize,
        height: usize,
        compact: bool,
        x: f32,
        y: f32,
    ) -> crate::cursor_ui::Shape {
        use crate::cursor_ui::Shape;
        if self.drag.is_some() {
            return Shape::Grabbing;
        }
        let p = self.panel(width, height, compact);
        if self.dropdown.is_some_and(|d| {
            self.popup(d, width, height, compact, &self.choices(d.field))
                .rect
                .contains(x, y)
        }) {
            return Shape::Pointer;
        }
        if y < (p.rect.y + if compact { 24 } else { 36 }) as f32
            && p.rect.contains(x, y)
            && x < (p.rect.x + p.rect.w.saturating_sub(28)) as f32
        {
            return Shape::Grab;
        }
        let in_popup = self.dropdown.is_some_and(|d| {
            self.popup(d, width, height, compact, &self.choices(d.field))
                .rect
                .contains(x, y)
        });
        if in_popup || (p.rect.contains(x, y) && hit_panel(&p, x, y).is_some()) {
            Shape::Pointer
        } else {
            Shape::Arrow
        }
    }
    fn selected(&self, row: usize) -> Value {
        let t = self.settings.editor_typography();
        match row {
            0 => Value::Theme(self.settings.theme),
            1 => Value::Screen(self.settings.framebuffer),
            2 => Value::Font(t.family),
            3 => Value::Size(t.scale_percent()),
            4 => Value::Spacing(t.line_spacing),
            5 => Value::Cursor(self.settings.mouse_cursor),
            PRESET => {
                let v = self.settings.visuals();
                match v.preset() {
                    Some(preset) => Value::Preset(Some(preset)),
                    None if v.effects.is_neutral() => Value::Preset(None),
                    None => Value::Custom,
                }
            }
            FRAME_RATE => Value::Fps(self.settings.visuals().fps),
            TIME => Value::Time(self.settings.visuals().time),
            SHADOW => Value::Shadow(self.settings.visuals().shadow),
            SHADOW_X => Value::Offset(true, self.settings.visuals().shadow_x),
            SHADOW_Y => Value::Offset(false, self.settings.visuals().shadow_y),
            BORDER => Value::Border(self.settings.visuals().border),
            CORNER => Value::Corner(self.settings.visuals().corner_inset),
            INSET_X => Value::Inset(true, self.settings.visuals().inset_x),
            INSET_Y => Value::Inset(false, self.settings.visuals().inset_y),
            LACE => Value::Lace(self.settings.visuals().lace.enabled),
            LACE_DENSITY => Value::LaceDensity(twentieths(self.settings.visuals().lace.density)),
            LACE_STRENGTH => {
                Value::LaceStrength(twentieths(self.settings.visuals().lace.strength).max(1))
            }
            LACE_SPACING => Value::LaceSpacing(self.settings.visuals().lace.spacing),
            LACE_SERIFS => Value::LaceSerifs(self.settings.visuals().lace.serifs),
            LACE_SEED => Value::LaceSeed(self.settings.visuals().lace.seed as u8),
            field => {
                let index = field - FIRST_EFFECT;
                let effect = Effect::ALL[index];
                let amount = effect.get(&self.settings.visuals().effects);
                Value::Effect(index, effect.step_of(amount))
            }
        }
    }
    /// A row's value text, including what terminals manage themselves.
    fn field_text(&self, field: usize) -> String {
        if field >= STYLE_FIELDS && (self.cell_backend || self.terminal_graphics) {
            return "Window only".into();
        }
        if self.cell_backend && field != 0 {
            return "Terminal setting".into();
        }
        if self.terminal_graphics && field == 5 {
            return "Terminal pointer".into();
        }
        let t = self.settings.editor_typography();
        match field {
            0 => self.settings.theme.name().into(),
            1 if self.settings.framebuffer == Framebuffer::Auto => self
                .settings
                .resolution()
                .map_or("Auto: Native".into(), |(w, h)| format!("Auto: {w}x{h}")),
            1 => self.settings.framebuffer.name().into(),
            2 => t.family.name().into(),
            3 => format!("{}%", t.scale_percent()),
            4 => format!("{}%", t.line_spacing),
            5 => self.settings.mouse_cursor.name().into(),
            field => self.value_text(field),
        }
    }
    /// A row's value as drawn in `cells` character cells: effect amounts get Pixel SSH's gauge
    /// in front when it fits.
    fn shown_text(&self, field: usize, cells: usize) -> String {
        const GAUGE: usize = 5;
        let text = self.field_text(field);
        if let Value::Effect(index, _) = self.selected(field) {
            let effect = Effect::ALL[index];
            if !self.cell_backend && !self.terminal_graphics && cells > GAUGE + text.chars().count()
            {
                if let Some(gauge) =
                    effect.meter(effect.get(&self.settings.visuals().effects), GAUGE)
                {
                    return format!("{gauge} {text}");
                }
            }
        }
        if let Value::LaceDensity(n) | Value::LaceStrength(n) = self.selected(field) {
            if !self.cell_backend && !self.terminal_graphics && cells > GAUGE + text.chars().count()
            {
                return format!(
                    "{} {text}",
                    texted_app::visuals::meter(f32::from(n) / 20.0, GAUGE)
                );
            }
        }
        text
    }
    /// What a Visuals row shows for its current value.
    fn value_text(&self, field: usize) -> String {
        let v = self.settings.visuals();
        match self.selected(field) {
            Value::Preset(preset) => preset.map_or("Off", Preset::name).into(),
            Value::Custom => "Custom".into(),
            Value::Fps(fps) => format!("{fps} FPS"),
            Value::Time(None) => "Live".into(),
            Value::Time(Some(t)) => format!("{:02}:{:02}:{:02}", t / 3600, t / 60 % 60, t % 60),
            Value::Effect(index, _) => {
                let effect = Effect::ALL[index];
                effect.label(effect.get(&v.effects))
            }
            Value::Shadow(on) => if on { "On" } else { "Off" }.into(),
            Value::Offset(_, pixels) | Value::Inset(_, pixels) | Value::Corner(pixels) => {
                pixels.to_string()
            }
            Value::Border(border) => border.name().into(),
            Value::Lace(_) => match &self.lace_unavailable {
                Some(reason) => format!("Unavailable: {reason}"),
                None => if v.lace.enabled { "On" } else { "Off" }.into(),
            },
            Value::LaceDensity(n) | Value::LaceStrength(n) => format!("{:.2}", f32::from(n) / 20.0),
            Value::LaceSpacing(n) => format!("{n} {}", if n == 1 { "cell" } else { "cells" }),
            Value::LaceSerifs(on) => if on { "Strokes" } else { "Ignored" }.into(),
            Value::LaceSeed(n) => n.to_string(),
            _ => String::new(),
        }
    }
    fn choices(&self, row: usize) -> Arc<Vec<Choice>> {
        // Terminals draw their own pointer, and their pictures skip the effect pass.
        if (self.cell_backend && row != 0) || (self.terminal_graphics && row >= 5) {
            return Arc::new(vec![Choice {
                label: if row >= STYLE_FIELDS {
                    "Window only".into()
                } else {
                    "Managed by the terminal emulator".into()
                },
                value: self.selected(row),
                enabled: false,
            }]);
        }
        if row == 2 {
            if let Some((query, choices)) = &*self.font_choices.borrow() {
                if query == &self.font_query {
                    return choices.clone();
                }
            }
        }
        let choices = match row {
            0 => Theme::ALL
                .into_iter()
                .map(|v| Choice::new(v.name().into(), Value::Theme(v)))
                .collect(),
            1 => Framebuffer::ALL
                .into_iter()
                .map(|v| Choice::new(v.name().into(), Value::Screen(v)))
                .collect(),
            2 => {
                let mut choices: Vec<Choice> = FontFamily::ALL
                    .into_iter()
                    .map(|v| Choice::new(v.name().into(), Value::Font(v)))
                    .chain(
                        texted_app::font_library::catalog()
                            .fonts
                            .iter()
                            .enumerate()
                            .map(|(i, f)| Choice {
                                label: format!(
                                    "{} · {}x{} · {}",
                                    f.name,
                                    f.width,
                                    f.height,
                                    f.disabled_reason
                                        .as_deref()
                                        .map(str::to_owned)
                                        .unwrap_or_else(|| format!("UA {}/66", f.ukrainian))
                                ),
                                value: Value::Font(FontFamily::Local(i as u16)),
                                enabled: f.enabled(),
                            }),
                    )
                    .collect();
                if !self.font_query.is_empty() {
                    let query = self.font_query.to_lowercase();
                    choices.retain(|c| c.label.to_lowercase().contains(&query));
                }
                if choices.is_empty() {
                    choices.push(Choice {
                        label: format!("No fonts matching: {}", self.font_query),
                        value: self.selected(2),
                        enabled: false,
                    });
                }
                let choices = Arc::new(choices);
                *self.font_choices.borrow_mut() = Some((self.font_query.clone(), choices.clone()));
                return choices;
            }
            5 => MouseCursor::ALL
                .into_iter()
                .map(|c| Choice::new(c.name().into(), Value::Cursor(c)))
                .collect(),
            PRESET => std::iter::once(Choice::new("Off".into(), Value::Preset(None)))
                .chain(
                    Machine::ALL
                        .into_iter()
                        .map(Preset::Machine)
                        .chain(Look::ALL.into_iter().map(Preset::Look))
                        .map(|p| Choice::new(p.name().into(), Value::Preset(Some(p)))),
                )
                .collect(),
            FRAME_RATE => (4..=30)
                .map(|fps| Choice::new(format!("{fps} FPS"), Value::Fps(fps)))
                .collect(),
            TIME => std::iter::once(Choice::new("Live".into(), Value::Time(None)))
                .chain((0..24).map(|hour| {
                    Choice::new(format!("{hour:02}:00"), Value::Time(Some(hour * 3600)))
                }))
                .collect(),
            SHADOW => [false, true]
                .into_iter()
                .map(|on| Choice::new(if on { "On" } else { "Off" }.into(), Value::Shadow(on)))
                .collect(),
            BORDER => texted_app::visuals::Border::ALL
                .into_iter()
                .map(|b| Choice::new(b.name().into(), Value::Border(b)))
                .collect(),
            CORNER => (0..=32)
                .map(|n| Choice::new(n.to_string(), Value::Corner(n)))
                .collect(),
            INSET_X | INSET_Y => (0..=8)
                .map(|n| Choice::new(n.to_string(), Value::Inset(row == INSET_X, n)))
                .collect(),
            LACE => [false, true]
                .into_iter()
                .map(|on| Choice::new(if on { "On" } else { "Off" }.into(), Value::Lace(on)))
                .collect(),
            LACE_DENSITY => (0..=20)
                .map(|n| Choice::new(format!("{:.2}", f32::from(n) / 20.0), Value::LaceDensity(n)))
                .collect(),
            LACE_STRENGTH => (1..=20)
                .map(|n| {
                    Choice::new(
                        format!("{:.2}", f32::from(n) / 20.0),
                        Value::LaceStrength(n),
                    )
                })
                .collect(),
            LACE_SPACING => (1..=3)
                .map(|n| {
                    let label = format!("{n} {}", if n == 1 { "cell" } else { "cells" });
                    Choice::new(label, Value::LaceSpacing(n))
                })
                .collect(),
            LACE_SERIFS => [false, true]
                .into_iter()
                .map(|on| {
                    let label = if on { "Strokes" } else { "Ignored" };
                    Choice::new(label.into(), Value::LaceSerifs(on))
                })
                .collect(),
            LACE_SEED => (1..=32)
                .map(|n| Choice::new(n.to_string(), Value::LaceSeed(n)))
                .collect(),
            SHADOW_X | SHADOW_Y => (0..=4)
                .map(|n| Choice::new(n.to_string(), Value::Offset(row == SHADOW_X, n)))
                .collect(),
            field if field >= FIRST_EFFECT => {
                let index = field - FIRST_EFFECT;
                let effect = Effect::ALL[index];
                (0..=effect.steps())
                    .map(|step| {
                        Choice::new(
                            effect.label(effect.value_at(step)),
                            Value::Effect(index, step),
                        )
                    })
                    .collect()
            }
            row => {
                let t = self.settings.editor_typography();
                let mut numbers: Vec<u16> = if row == 3 {
                    (10..=800).step_by(10).collect()
                } else {
                    (100..=180).step_by(10).collect()
                };
                let selected = if row == 3 {
                    t.scale_percent()
                } else {
                    t.line_spacing
                };
                if !numbers.contains(&selected) {
                    numbers.push(selected);
                    numbers.sort_unstable();
                }
                numbers
                    .into_iter()
                    .map(|v| {
                        if row == 3 {
                            Choice::new(format!("{v}%"), Value::Size(v))
                        } else {
                            Choice::new(format!("{v}%"), Value::Spacing(v))
                        }
                    })
                    .collect()
            }
        };
        Arc::new(choices)
    }
    fn choose(&mut self, choice: &Choice) {
        if !choice.enabled {
            return;
        }
        match choice.value {
            Value::Theme(theme) => {
                if self.settings.theme != theme {
                    self.settings.theme = theme;
                    self.settings.colors.clear();
                    // Like applying a machine in ShaderWallpaper, a theme brings its visuals.
                    // Lace is not part of a theme's look.
                    let (time, lace) = {
                        let v = self.settings.visuals();
                        (v.time, v.lace)
                    };
                    self.settings.visuals = Some(Visuals {
                        time,
                        lace,
                        ..Visuals::for_theme(theme)
                    });
                }
            }
            Value::Screen(screen) => self.settings.framebuffer = screen,
            Value::Preset(preset) => {
                let v = self.visuals_mut();
                *v = match preset {
                    // A machine brings its timing and chrome, as in ShaderWallpaper.
                    Some(Preset::Machine(machine)) => Visuals {
                        time: v.time,
                        lace: v.lace,
                        ..machine.visuals()
                    },
                    // Pixel SSH's presets only set the effects.
                    Some(Preset::Look(look)) => Visuals {
                        effects: look.effects(),
                        ..*v
                    },
                    None => Visuals {
                        effects: Default::default(),
                        ..*v
                    },
                };
            }
            Value::Custom => {}
            Value::Fps(fps) => self.visuals_mut().fps = fps,
            Value::Time(time) => self.visuals_mut().time = time,
            Value::Effect(index, step) => {
                let effect = Effect::ALL[index];
                effect.set(&mut self.visuals_mut().effects, effect.value_at(step));
            }
            Value::Shadow(on) => self.visuals_mut().shadow = on,
            Value::Offset(true, n) => self.visuals_mut().shadow_x = n,
            Value::Offset(false, n) => self.visuals_mut().shadow_y = n,
            Value::Border(border) => self.visuals_mut().border = border,
            Value::Corner(n) => self.visuals_mut().corner_inset = n,
            Value::Inset(true, n) => self.visuals_mut().inset_x = n,
            Value::Inset(false, n) => self.visuals_mut().inset_y = n,
            // A face that cannot carry lace keeps it off.
            Value::Lace(true) if self.lace_unavailable.is_some() => {}
            Value::Lace(on) => self.visuals_mut().lace.enabled = on,
            Value::LaceDensity(n) => self.visuals_mut().lace.density = f32::from(n) / 20.0,
            Value::LaceStrength(n) => self.visuals_mut().lace.strength = f32::from(n) / 20.0,
            Value::LaceSpacing(n) => self.visuals_mut().lace.spacing = n,
            Value::LaceSerifs(on) => self.visuals_mut().lace.serifs = on,
            Value::LaceSeed(n) => self.visuals_mut().lace.seed = u32::from(n),
            Value::Font(font) => {
                let t = self.settings.editor_typography_mut();
                let zoom = t.scale_percent();
                t.family = font;
                t.size_px = font.cell().1;
                t.zoom_percent = zoom;
            }
            Value::Size(size) => {
                let t = self.settings.editor_typography_mut();
                t.size_px = t.family.cell().1;
                t.zoom_percent = size;
            }
            Value::Spacing(spacing) => self.settings.editor_typography_mut().line_spacing = spacing,
            Value::Cursor(cursor) => self.settings.mouse_cursor = cursor,
        }
        self.settings.editor_typography_mut().normalize();
        self.palette = self.settings.palette().unwrap();
    }
    fn popup(
        &self,
        d: Dropdown,
        width: usize,
        height: usize,
        compact: bool,
        choices: &[Choice],
    ) -> Popup {
        let cell = if compact { 8 } else { 11 };
        let label_width = if d.field == 2 {
            width.saturating_sub(16)
        } else {
            choices
                .iter()
                .map(|c| c.label.chars().count() * cell)
                .max()
                .unwrap_or(0)
        };
        Popup::new(
            self.panel(width, height, compact)
                .row(self.slot_of(d.field).unwrap_or(0)),
            (width, height),
            compact,
            choices.len(),
            label_width,
        )
    }
    fn open_dropdown(&mut self, row: usize, width: usize, height: usize, compact: bool) {
        self.cancel_preview();
        self.origin = Some(self.settings.clone());
        self.font_query.clear();
        let choices = self.choices(row);
        let selected = choices
            .iter()
            .position(|c| c.value == self.selected(row))
            .unwrap_or(0);
        self.set_focus(row);
        let mut d = Dropdown::new(row, selected);
        let popup = self.popup(d, width, height, compact, &choices);
        d.reveal(popup.visible, choices.len());
        self.dropdown = Some(d);
    }
    pub fn hover(&mut self, width: usize, height: usize, x: f32, y: f32, compact: bool) -> bool {
        if let Some((sx, sy, px, py)) = self.drag {
            self.position = Some((px + x - sx, py + y - sy));
            let p = self.panel(width, height, compact);
            self.position = Some((p.rect.x as f32, p.rect.y as f32));
            return true;
        }

        let Some(mut d) = self.dropdown else {
            return false;
        };
        let choices = self.choices(d.field);
        let popup = self.popup(d, width, height, compact, &choices);
        if let Some(index) = popup.item_at(x, y, d.first, choices.len()) {
            if choices[index].enabled && d.highlighted != index {
                d.highlighted = index;
                self.dropdown = Some(d);
                return true;
            }
        }
        false
    }
    pub fn wheel(&mut self, rows: f64, width: usize, height: usize, compact: bool) -> bool {
        let Some(mut d) = self.dropdown else {
            let visible = self.panel(width, height, compact).rows;
            let (start, end) = self.page();
            if rows == 0.0 || end - start <= visible {
                return false;
            }
            let before = self.scroll;
            let most = end - start - visible;
            self.scroll = if rows > 0.0 {
                self.scroll.saturating_sub(1)
            } else {
                (self.scroll + 1).min(most)
            };
            // Keep the focus on a row that is still shown.
            if self.slot_of(self.focus).is_none() {
                let edge = if rows > 0.0 { visible - 1 } else { 0 };
                self.focus = self.field_at(edge).unwrap_or(self.focus);
            }
            return self.scroll != before;
        };
        let choices = self.choices(d.field);
        let popup = self.popup(d, width, height, compact, &choices);
        let before = d.first;
        let relative = d.highlighted.saturating_sub(d.first).min(popup.visible - 1);
        d.wheel(rows, popup.visible, choices.len());
        if d.first != before {
            d.highlighted = (d.first + relative).min(choices.len() - 1);
            if !choices[d.highlighted].enabled {
                let enabled: Vec<_> = choices.iter().map(|c| c.enabled).collect();
                d.navigate(if rows < 0.0 { -1 } else { 1 }, &enabled);
            }
        }
        self.dropdown = Some(d);
        d.first != before
    }
    pub fn zoom(&mut self, delta: i32) {
        self.cancel_preview();
        self.settings.editor_typography_mut().resize(delta);
    }
    pub fn reset_size(&mut self) {
        self.cancel_preview();
        let size = self.settings.editor_typography().family.cell().1;
        let t = self.settings.editor_typography_mut();
        t.size_px = size;
        t.zoom_percent = 100;
        t.normalize();
    }
    pub fn click(&mut self, width: usize, height: usize, x: f32, y: f32, compact: bool) {
        let panel = self.panel(width, height, compact);
        let over_popup = self.dropdown.is_some_and(|d| {
            self.popup(d, width, height, compact, &self.choices(d.field))
                .rect
                .contains(x, y)
        });
        if panel.done().contains(x, y) && !over_popup {
            self.accept_preview();
            self.open = false;
            return;
        }
        // A tab turns the page; the title row still drags the panel from anywhere.
        if !over_popup {
            for visuals in [false, true] {
                if panel.tab(visuals).contains(x, y) && visuals != self.visuals_page() {
                    self.cancel_preview();
                    self.set_focus(if visuals { PRESET } else { 0 });
                }
            }
        }
        if !over_popup
            && panel.rect.contains(x, y)
            && y < (panel.rect.y + if compact { 24 } else { 36 }) as f32
            && x < (panel.rect.x + panel.rect.w.saturating_sub(28)) as f32
        {
            self.drag = Some((x, y, panel.rect.x as f32, panel.rect.y as f32));
            return;
        }

        if let Some(d) = self.dropdown {
            let choices = self.choices(d.field);
            let popup = self.popup(d, width, height, compact, &choices);
            if let Some(index) = popup.item_at(x, y, d.first, choices.len()) {
                if choices[index].enabled {
                    self.choose(&choices[index]);
                    self.accept_preview();
                }
                return;
            }
            if popup.rect.contains(x, y) {
                if popup.scrollbar > 0 {
                    let direction = if y < (popup.rect.y + popup.rect.h / 2) as f32 {
                        -1.0
                    } else {
                        1.0
                    };
                    self.wheel(direction * popup.visible as f64, width, height, compact);
                }
                return;
            }
            self.cancel_preview();
            if let Some(Hit::Field(slot)) = hit_panel(&self.panel(width, height, compact), x, y) {
                if let Some(field) = self.field_at(slot).filter(|&f| f != d.field) {
                    self.open_dropdown(field, width, height, compact);
                }
            }
            return;
        }
        match hit_panel(&self.panel(width, height, compact), x, y) {
            Some(Hit::Close) => self.set_open(false),
            Some(Hit::Reset) => self.reset(),
            Some(Hit::Field(slot)) => {
                if let Some(field) = self.field_at(slot) {
                    self.open_dropdown(field, width, height, compact);
                }
            }
            Some(Hit::Page { .. }) | None => {}
        }
    }
    pub fn key(&mut self, input: &KeyInput<'_>, width: usize, height: usize, compact: bool) {
        self.panel(width, height, compact);
        let activate = matches!(input.logical, Key::Named(NamedKey::Enter))
            || matches!(input.logical,Key::Character(s) if s==" ");
        if let Some(mut d) = self.dropdown {
            let mut searched = false;
            if d.field == 2
                && !input.modifiers.super_key()
                && !input.modifiers.control_key()
                && !input.modifiers.alt_key()
            {
                match input.logical {
                    Key::Named(NamedKey::Backspace) => {
                        self.font_query.pop();
                        searched = true;
                    }
                    Key::Character(text)
                        if (text != " " || !self.font_query.is_empty())
                            && !text.chars().any(char::is_control) =>
                    {
                        let remaining = 64usize.saturating_sub(self.font_query.chars().count());
                        self.font_query.extend(text.chars().take(remaining));
                        searched = true;
                    }
                    _ => {}
                }
            }
            let choices = self.choices(d.field);
            if searched {
                let selected = choices
                    .iter()
                    .position(|c| c.enabled && c.value == self.selected(2))
                    .or_else(|| choices.iter().position(|c| c.enabled))
                    .unwrap_or(0);
                d = Dropdown::new(2, selected);
                let popup = self.popup(d, width, height, compact, &choices);
                d.reveal(popup.visible, choices.len());
                self.dropdown = Some(d);
                return;
            }
            d.highlighted = d.highlighted.min(choices.len() - 1);
            let enabled: Vec<_> = choices.iter().map(|c| c.enabled).collect();
            let popup = self.popup(d, width, height, compact, &choices);
            match input.logical {
                Key::Named(NamedKey::Escape) => {
                    self.cancel_preview();
                    return;
                }
                Key::Named(NamedKey::Tab) => {
                    self.cancel_preview();
                    self.step_focus(input.modifiers.shift_key());
                    return;
                }
                Key::Named(NamedKey::ArrowUp) => d.navigate(-1, &enabled),
                Key::Named(NamedKey::ArrowDown) => d.navigate(1, &enabled),
                Key::Named(NamedKey::Home) => {
                    d.highlighted = 0;
                    if !enabled[0] {
                        d.navigate(1, &enabled);
                    }
                }
                Key::Named(NamedKey::End) => {
                    d.highlighted = choices.len() - 1;
                    if !enabled[d.highlighted] {
                        d.navigate(-1, &enabled);
                    }
                }
                Key::Named(NamedKey::PageUp) => {
                    d.highlighted = d.highlighted.saturating_sub(popup.visible);
                    if !enabled[d.highlighted] {
                        d.navigate(-1, &enabled);
                    }
                }
                Key::Named(NamedKey::PageDown) => {
                    d.highlighted = (d.highlighted + popup.visible).min(choices.len() - 1);
                    if !enabled[d.highlighted] {
                        d.navigate(1, &enabled);
                    }
                }
                _ if activate => {
                    if choices[d.highlighted].enabled {
                        self.choose(&choices[d.highlighted]);
                        self.accept_preview();
                    }
                    return;
                }
                _ => return,
            }
            d.reveal(popup.visible, choices.len());
            self.choose(&choices[d.highlighted]);
            self.dropdown = Some(d);
            return;
        }
        match input.logical {
            Key::Named(NamedKey::Escape) => self.set_open(false),
            Key::Named(NamedKey::Tab) => self.step_focus(input.modifiers.shift_key()),
            Key::Named(
                NamedKey::ArrowUp
                | NamedKey::ArrowDown
                | NamedKey::ArrowLeft
                | NamedKey::ArrowRight,
            ) => self.open_dropdown(self.focus, width, height, compact),
            _ if activate => self.open_dropdown(self.focus, width, height, compact),
            _ => {}
        }
    }
    pub fn paint(
        &mut self,
        draws: &mut DrawList,
        font: &mut FontEngine,
        width: usize,
        height: usize,
        compact: bool,
    ) {
        if !self.open {
            return;
        }
        draws
            .cells
            .push(texted_app::cell_grid::CellCommand::HideCursor);
        let p = self.panel(width, height, compact);
        let r = p.rect;
        let c = self.palette;
        draw_box(
            draws,
            width,
            height,
            r.x,
            r.y,
            r.w,
            r.h,
            c.panel_bg,
            c.panel_border,
        );
        let top = if compact { 4 } else { 16 };
        for (visuals, name) in [(false, "Style"), (true, "Visuals")] {
            let tab = p.tab(visuals);
            let active = visuals == self.visuals_page();
            label(
                draws,
                font,
                (width, height),
                (tab.x, r.y + top),
                name,
                if active { c.text } else { c.status_text },
                tab.w,
            );
            if active {
                let under = r.y + top + if compact { 9 } else { 18 };
                let w = name.chars().count() * if compact { 8 } else { 9 };
                draw_rect(draws, width, height, tab.x, under, w, 1, c.status_accent);
            }
        }
        label(
            draws,
            font,
            (width, height),
            (r.x + r.w.saturating_sub(20), r.y + top),
            "X",
            c.text,
            16,
        );
        label(
            draws,
            font,
            (width, height),
            (r.x + p.pad, r.y + if compact { 20 } else { 48 }),
            &if self.dropdown.is_some_and(|d| d.field == 2) {
                format!(
                    "Find font: {}",
                    if self.font_query.is_empty() {
                        "type a name"
                    } else {
                        &self.font_query
                    }
                )
            } else if self.page().1 - self.page().0 > p.rows {
                let total = self.page().1 - self.page().0;
                let first = self.scroll + 1;
                let last = (self.scroll + p.rows).min(total);
                format!(
                    "Rows {first}-{last} of {total}{}",
                    if compact {
                        ""
                    } else {
                        "  Tab: field  Enter: list"
                    }
                )
            } else if compact {
                "Tab: field  Enter: list".into()
            } else {
                "Tab: field  Enter: list  Esc: close".into()
            },
            c.status_text,
            r.w.saturating_sub(2 * p.pad),
        );
        for slot in 0..p.rows {
            let Some(field) = self.field_at(slot) else {
                break;
            };
            let field_rect = p.row(slot);
            let text_y = field_rect.y + if compact { 4 } else { 5 };
            label(
                draws,
                font,
                (width, height),
                (r.x + p.pad, text_y),
                field_title(field, compact),
                c.text,
                p.input.saturating_sub(2 * p.pad),
            );
            draw_box(
                draws,
                width,
                height,
                field_rect.x,
                field_rect.y,
                field_rect.w,
                field_rect.h,
                c.input_bg,
                if self.focus == field {
                    c.input_active
                } else {
                    c.input_border
                },
            );
            let room = field_rect.w.saturating_sub(if compact { 20 } else { 38 });
            let cells = (room as f32 / font.char_width_px.max(1.0)) as usize;
            label(
                draws,
                font,
                (width, height),
                (field_rect.x + if compact { 4 } else { 10 }, text_y),
                &self.shown_text(field, cells),
                c.text,
                room,
            );
            label(
                draws,
                font,
                (width, height),
                (
                    field_rect.x + field_rect.w.saturating_sub(if compact { 12 } else { 24 }),
                    text_y,
                ),
                "v",
                c.status_accent,
                16,
            );
        }
        let reset = match (compact, self.visuals_page()) {
            (true, _) => "Reset",
            (false, false) => "Reset defaults",
            (false, true) => "Theme visuals",
        };
        for (rect, text) in [(p.reset(), reset), (p.done(), "Done")] {
            draw_box(
                draws,
                width,
                height,
                rect.x,
                rect.y,
                rect.w,
                rect.h,
                c.button_bg,
                c.button_border,
            );
            label(
                draws,
                font,
                (width, height),
                (rect.x + 5, rect.y + if compact { 4 } else { 5 }),
                text,
                c.text,
                rect.w.saturating_sub(10),
            );
        }
        label(
            draws,
            font,
            (width, height),
            (r.x + p.pad, r.y + p.buttons + p.row_h + 4),
            if self.previewing() {
                "Preview: Enter keeps, Esc restores"
            } else {
                &self.save_status
            },
            c.status_text,
            r.w.saturating_sub(2 * p.pad),
        );
        if let Some(d) = self.dropdown {
            self.paint_dropdown(draws, font, width, height, compact, d);
        }
    }
    fn preview_font(&mut self, family: FontFamily) -> &mut FontEngine {
        self.preview_clock += 1;
        let clock = self.preview_clock;
        let index = if let Some(index) = self.previews.iter().position(|(f, _, _)| *f == family) {
            index
        } else {
            if self.previews.len() >= 20 {
                let oldest = self
                    .previews
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, p)| p.2)
                    .unwrap()
                    .0;
                self.previews.swap_remove(oldest);
            }
            self.previews.push((
                family,
                FontEngine::with_typography(texted_app::appearance::Typography {
                    family,
                    size_px: family.cell().1,
                    line_spacing: 100,
                    zoom_percent: 100,
                }),
                clock,
            ));
            self.previews.len() - 1
        };
        self.previews[index].2 = clock;
        &mut self.previews[index].1
    }
    fn paint_dropdown(
        &mut self,
        draws: &mut DrawList,
        font: &mut FontEngine,
        width: usize,
        height: usize,
        compact: bool,
        d: Dropdown,
    ) {
        let choices = self.choices(d.field);
        let popup = self.popup(d, width, height, compact, &choices);
        let r = popup.rect;
        let c = self.palette;
        draw_box(
            draws,
            width,
            height,
            r.x,
            r.y,
            r.w,
            r.h,
            c.panel_bg,
            c.input_active,
        );
        let selected = self.selected(d.field);
        for (index, choice) in choices.iter().enumerate().skip(d.first).take(popup.visible) {
            let row = popup.row(index, d.first).unwrap();
            if index == d.highlighted && choice.enabled {
                draw_rect(
                    draws,
                    width,
                    height,
                    row.x,
                    row.y,
                    row.w,
                    row.h,
                    c.selection,
                );
            }
            let y = row.y + if compact { 3 } else { 4 };
            if choice.value == selected {
                label(
                    draws,
                    font,
                    (width, height),
                    (row.x + 2, y),
                    "*",
                    c.status_accent,
                    if compact { 10 } else { 16 },
                );
            }
            label(
                draws,
                font,
                (width, height),
                (row.x + if compact { 12 } else { 20 }, y),
                &choice.label,
                if choice.enabled {
                    c.text
                } else {
                    c.gutter_text
                },
                row.w.saturating_sub(if d.field == 2 && choice.enabled {
                    if compact {
                        84
                    } else {
                        180
                    }
                } else if compact {
                    16
                } else {
                    24
                }),
            );
            if d.field == 2 && choice.enabled {
                if let Value::Font(family) = choice.value {
                    let available = if compact { 64.0 } else { 150.0 };
                    self.preview_font(family).draw_preview(
                        draws,
                        [width as f32, height as f32],
                        [(row.x + row.w) as f32 - available - 8.0, row.y as f32 + 3.0],
                        [available, row.h.saturating_sub(6) as f32],
                        "AaЇїҐґ",
                        c.text,
                    );
                }
            }
        }
        if popup.scrollbar > 0 {
            let x = r.x + r.w - popup.scrollbar;
            let track = r.h.saturating_sub(2);
            draw_rect(
                draws,
                width,
                height,
                x,
                r.y + 1,
                popup.scrollbar.saturating_sub(1),
                track,
                c.input_bg,
            );
            let thumb = (track * popup.visible / choices.len()).max(4).min(track);
            let y = r.y
                + 1
                + (track - thumb) * d.first / choices.len().saturating_sub(popup.visible).max(1);
            draw_rect(
                draws,
                width,
                height,
                x + 1,
                y,
                popup.scrollbar.saturating_sub(3),
                thumb,
                c.gutter_text,
            );
        }
    }
}
/// Row titles; compact ones fit seven 8-pixel cells.
/// A 0 to 1 amount as whole twentieths, the step of the lace rows.
fn twentieths(amount: f32) -> u8 {
    (amount * 20.0).round().clamp(0.0, 20.0) as u8
}
fn field_title(field: usize, compact: bool) -> &'static str {
    if (FIRST_EFFECT..SHADOW).contains(&field) {
        let effect = Effect::ALL[field - FIRST_EFFECT];
        return if compact {
            effect.short()
        } else {
            effect.title()
        };
    }
    const COMPACT: [&str; FIELDS - Effect::ALL.len()] = [
        "Theme",
        "Screen",
        "Font",
        "Size",
        "Spacing",
        "Cursor",
        "Preset",
        "FPS",
        "Time",
        "Shadow",
        "Shad X",
        "Shad Y",
        "Border",
        "Corner",
        "Inset X",
        "Inset Y",
        "Lace",
        "Lace dens",
        "Lace str",
        "Lace gap",
        "Lace serif",
        "Lace seed",
    ];
    const FULL: [&str; FIELDS - Effect::ALL.len()] = [
        "Theme",
        "Resolution",
        "Editor font",
        "Text scale",
        "Line height",
        "Mouse cursor",
        "Preset",
        "Frame rate",
        "Time of day",
        "Shadow",
        "Shadow X",
        "Shadow Y",
        "Border",
        "Corner inset",
        "Inset X",
        "Inset Y",
        "Lace",
        "Lace density",
        "Lace strength",
        "Lace spacing",
        "Lace serifs",
        "Lace pattern",
    ];
    let index = if field >= SHADOW {
        field - Effect::ALL.len()
    } else {
        field
    };
    if compact {
        COMPACT[index]
    } else {
        FULL[index]
    }
}
fn label(
    draws: &mut DrawList,
    font: &mut FontEngine,
    size: (usize, usize),
    at: (usize, usize),
    text: &str,
    color: u32,
    available: usize,
) {
    let limit = size.0.min(at.0.saturating_add(available));
    font.draw_string(draws, limit, size.1, at.0, at.1, text, color);
}

#[cfg(test)]
mod tests {
    use super::*;
    use texted_app::appearance::Theme;
    use winit::keyboard::{KeyCode, ModifiersState, PhysicalKey};
    fn press(ui: &mut AppearanceUi, key: NamedKey, compact: bool) {
        let logical = Key::Named(key);
        let (w, h) = if compact { (256, 192) } else { (980, 700) };
        ui.key(
            &KeyInput {
                physical: PhysicalKey::Code(KeyCode::Enter),
                logical: &logical,
                text: None,
                modifiers: ModifiersState::empty(),
            },
            w,
            h,
            compact,
        );
    }
    #[test]
    #[ignore = "CPU profile; run explicitly on the target machine"]
    fn profile_font_picker() {
        use std::{hint::black_box, time::Instant};
        let started = Instant::now();
        let mut ui = AppearanceUi::new(Appearance::default());
        ui.set_open(true);
        ui.open_dropdown(2, 980, 700, false);
        let entries = ui.choices(2).len();
        let cold = started.elapsed();
        let started = Instant::now();
        for _ in 0..10_000 {
            let choices = ui.choices(2);
            black_box(ui.popup(ui.dropdown.unwrap(), 980, 700, false, &choices));
        }
        let hot = started.elapsed();
        let mut font = FontEngine::new(16.0);
        let mut draws = DrawList::default();
        ui.paint(&mut draws, &mut font, 980, 700, false);
        let started = Instant::now();
        for _ in 0..1_000 {
            draws.reset(0);
            ui.paint(&mut draws, &mut font, 980, 700, false);
            black_box(&draws);
        }
        println!("font picker: {entries} entries; cold {cold:?}; cached choices+geometry {:?}/call; drawing preparation {:?}/frame; cached faces {}",hot/10_000,started.elapsed()/1_000,ui.previews.len());
    }
    #[test]
    fn font_catalog_choices_are_reused_until_the_search_changes() {
        let mut ui = AppearanceUi::new(Appearance::default());
        let first = ui.choices(2);
        assert!(Arc::ptr_eq(&first, &ui.choices(2)));
        ui.font_query = "Arial".into();
        let filtered = ui.choices(2);
        assert!(!Arc::ptr_eq(&first, &filtered));
        assert!(Arc::ptr_eq(&filtered, &ui.choices(2)));
    }
    #[test]
    fn cursor_popup_items_take_priority_over_the_header_and_done_button() {
        for index in 0..MouseCursor::ALL.len() {
            let mut ui = AppearanceUi::new(Appearance::default());
            ui.set_open(true);
            ui.open_dropdown(5, 980, 700, false);
            let d = ui.dropdown.unwrap();
            let choices = ui.choices(5);
            let popup = ui.popup(d, 980, 700, false, &choices);
            if let Some(row) = popup.row(index, d.first) {
                let (x, y) = ((row.x + row.w / 2) as f32, (row.y + row.h / 2) as f32);
                assert_eq!(
                    ui.cursor(980, 700, false, x, y),
                    crate::cursor_ui::Shape::Pointer
                );
                ui.click(980, 700, x, y, false);
                assert!(ui.settings.mouse_cursor == MouseCursor::ALL[index]);
                assert!(ui.drag.is_none());
            }
        }
    }
    #[test]
    fn dragging_keeps_values_and_moves_the_panel_and_its_hit_regions() {
        let mut ui = AppearanceUi::new(Appearance::default());
        ui.set_open(true);
        let before = ui.settings.clone();
        let p = ui.panel(980, 700, false);
        ui.click(
            980,
            700,
            (p.rect.x + 30) as f32,
            (p.rect.y + 15) as f32,
            false,
        );
        ui.hover(
            980,
            700,
            (p.rect.x + 110) as f32,
            (p.rect.y + 75) as f32,
            false,
        );
        ui.end_drag();
        let moved = ui.panel(980, 700, false);
        assert_eq!(moved.rect.x, p.rect.x + 80);
        assert_eq!(moved.rect.y, (p.rect.y + 60) / 16 * 16);
        let row = moved.row(2);
        ui.click(980, 700, (row.x + 3) as f32, (row.y + 3) as f32, false);
        assert_eq!(ui.dropdown.unwrap().field, 2);
        assert_eq!(ui.settings, before);
    }
    #[test]
    fn font_search_filters_without_changing_settings_and_enter_commits() {
        let mut ui = AppearanceUi::new(Appearance::default());
        ui.open_dropdown(2, 980, 700, false);
        let before = ui.settings.clone();
        let logical = Key::Character("ega".into());
        ui.key(
            &KeyInput {
                physical: PhysicalKey::Code(KeyCode::KeyA),
                logical: &logical,
                text: Some("ega"),
                modifiers: ModifiersState::empty(),
            },
            980,
            700,
            false,
        );
        assert_eq!(ui.settings, before);
        assert!(ui
            .choices(2)
            .iter()
            .all(|c| c.label.to_lowercase().contains("ega")));
        press(&mut ui, NamedKey::Enter, false);
        assert_eq!(ui.settings.typography.family, FontFamily::Ega);
    }
    #[test]
    fn empty_font_search_is_disabled_and_backspace_recovers_the_list() {
        let mut ui = AppearanceUi::new(Appearance::default());
        ui.open_dropdown(2, 980, 700, false);
        ui.font_query = "no_such_font_4fbea7".into();
        let before = ui.settings.clone();
        assert!(ui.choices(2).iter().all(|c| !c.enabled));
        press(&mut ui, NamedKey::Enter, false);
        assert_eq!(ui.settings, before);
        ui.open_dropdown(2, 980, 700, false);
        ui.font_query = "q".into();
        press(&mut ui, NamedKey::Backspace, false);
        assert!(ui.font_query.is_empty());
        assert!(ui.choices(2).iter().any(|c| c.enabled));
    }
    #[test]
    fn browsing_and_cancelling_a_list_preserves_values_and_custom_colors() {
        let mut settings = Appearance::default();
        settings.colors.insert("text".into(), "#ABCDEF".into());
        let mut ui = AppearanceUi::new(settings);
        ui.set_open(true);
        let original = ui.settings.clone();
        let r = Panel::new(980, 700, false).row(0);
        ui.click(980, 700, (r.x + 20) as f32, (r.y + 10) as f32, false);
        assert!(ui.dropdown.is_some());
        assert_eq!(ui.settings, original);
        press(&mut ui, NamedKey::End, false);
        assert_eq!(ui.dropdown.unwrap().highlighted, Theme::ALL.len() - 1);
        assert_ne!(ui.settings, original); // Arrow navigation previews immediately.
        press(&mut ui, NamedKey::Escape, false);
        assert!(ui.open && ui.dropdown.is_none());
        assert_eq!(ui.settings, original);
        press(&mut ui, NamedKey::Enter, false);
        press(&mut ui, NamedKey::Enter, false);
        assert_eq!(ui.settings, original); // Accepting the same palette keeps custom colors.
        press(&mut ui, NamedKey::Enter, false);
        press(&mut ui, NamedKey::End, false);
        press(&mut ui, NamedKey::Enter, false);
        assert_eq!(Some(&ui.settings.theme), Theme::ALL.last());
        assert!(ui.settings.colors.is_empty());
        assert!(ui.dropdown.is_none());
    }
    #[test]
    fn every_available_face_is_selectable_and_zoom_has_ten_percent_steps() {
        let mut ui = AppearanceUi::new(Appearance::default());
        let choices = ui.choices(2);
        assert_eq!(
            choices.len(),
            5 + texted_app::font_library::catalog().fonts.len()
        );
        for choice in choices.iter() {
            if !choice.enabled {
                let before = ui.settings.clone();
                ui.choose(choice);
                assert_eq!(ui.settings, before);
                continue;
            }
            ui.choose(choice);
            let sizes = ui.choices(3);
            assert!(sizes.iter().all(
                |c| matches!(c.value,Value::Size(v) if v % 10 == 0 && (10..=800).contains(&v))
            ));
        }
    }
    fn key_input(key: NamedKey, shift: bool) -> KeyInput<'static> {
        let logical: &'static Key = Box::leak(Box::new(Key::Named(key)));
        KeyInput {
            physical: winit::keyboard::PhysicalKey::Unidentified(
                winit::keyboard::NativeKeyCode::Unidentified,
            ),
            logical,
            text: None,
            modifiers: if shift {
                winit::keyboard::ModifiersState::SHIFT
            } else {
                winit::keyboard::ModifiersState::empty()
            },
        }
    }
    fn pick(ui: &mut AppearanceUi, field: usize, label: &str) {
        let (w, h) = (320, 200);
        ui.open_dropdown(field, w, h, true);
        let d = ui.dropdown.unwrap();
        let choices = ui.choices(field);
        let index = choices.iter().position(|c| c.label == label).unwrap();
        let mut d = d;
        d.highlighted = index;
        let popup = ui.popup(d, w, h, true, &choices);
        d.reveal(popup.visible, choices.len());
        ui.dropdown = Some(d);
        let row = popup.row(index, d.first).unwrap();
        ui.click(w, h, (row.x + 10) as f32, (row.y + 4) as f32, true);
        assert!(ui.dropdown.is_none(), "{label} was not chosen");
    }
    #[test]
    fn tab_walks_from_the_style_page_into_the_scrolling_visuals_page_and_back() {
        let mut ui = AppearanceUi::new(Appearance::default());
        ui.set_open(true);
        ui.set_focus(5);
        assert!(!ui.visuals_page());
        ui.key(&key_input(NamedKey::Tab, false), 320, 200, true);
        assert_eq!((ui.focus, ui.visuals_page()), (PRESET, true));
        assert_eq!(ui.field_at(0), Some(PRESET));
        for _ in PRESET..FIELDS - 1 {
            ui.key(&key_input(NamedKey::Tab, false), 320, 200, true);
            assert!(
                ui.slot_of(ui.focus).is_some(),
                "field {} is off screen",
                ui.focus
            );
        }
        assert_eq!(ui.field_at(ROWS - 1), Some(FIELDS - 1));
        ui.key(&key_input(NamedKey::Tab, false), 320, 200, true);
        assert_eq!((ui.focus, ui.visuals_page()), (0, false));
        ui.key(&key_input(NamedKey::Tab, true), 320, 200, true);
        assert_eq!(ui.focus, FIELDS - 1);
    }
    #[test]
    fn the_visuals_tab_turns_the_page_with_the_mouse() {
        let mut ui = AppearanceUi::new(Appearance::default());
        ui.set_open(true);
        let tab = Panel::new(320, 200, true).tab(true);
        ui.click(320, 200, (tab.x + 4) as f32, (tab.y + 4) as f32, true);
        assert!(ui.visuals_page());
        ui.end_drag();
        let style = Panel::new(320, 200, true).tab(false);
        ui.click(320, 200, (style.x + 4) as f32, (style.y + 4) as f32, true);
        assert!(!ui.visuals_page());
    }
    #[test]
    fn a_machine_preset_fills_frame_rate_and_every_effect_and_rows_set_exact_steps() {
        let mut ui = AppearanceUi::new(Appearance::default());
        ui.set_open(true);
        pick(&mut ui, PRESET, "Atari ST");
        assert_eq!(ui.settings.visuals(), Machine::AtariSt.visuals());
        assert_eq!(ui.field_text(PRESET), "Atari ST");
        pick(&mut ui, FIRST_EFFECT, "0.95");
        assert_eq!(
            ui.settings.visuals().effects.brightness,
            Effect::Brightness.value_at(9)
        );
        assert_eq!(ui.field_text(PRESET), "Custom");
        pick(&mut ui, FRAME_RATE, "30 FPS");
        pick(&mut ui, TIME, "13:00");
        assert_eq!(
            (ui.settings.visuals().fps, ui.settings.visuals().time),
            (30, Some(13 * 3600))
        );
        assert_eq!(ui.field_text(TIME), "13:00:00");
    }
    #[test]
    fn pixel_ssh_looks_set_only_the_effects_and_show_in_the_preset_row() {
        use texted_app::visuals::Look;
        let mut ui = AppearanceUi::new(Appearance::default());
        ui.set_open(true);
        let names: Vec<String> = ui.choices(PRESET).iter().map(|c| c.label.clone()).collect();
        assert_eq!(
            names,
            [
                "Off",
                "ZX Spectrum",
                "Atari ST",
                "Mac System 2.0",
                "Game Boy",
                "Pixel SSH",
                "CRT",
                "Arcade",
                "Bloom",
                "Glitch"
            ]
        );
        pick(&mut ui, PRESET, "Atari ST");
        pick(&mut ui, TIME, "13:00");
        pick(&mut ui, PRESET, "Arcade");
        let v = ui.settings.visuals();
        assert_eq!(v.effects, Look::Arcade.effects());
        assert_eq!(
            (v.fps, v.time, v.shadow, v.inset_x),
            (14, Some(13 * 3600), true, 2),
            "timing and chrome stay"
        );
        assert_eq!(ui.field_text(PRESET), "Arcade");
        pick(&mut ui, PRESET, "Off");
        assert!(ui.settings.visuals().effects.is_neutral());
        assert_eq!(ui.field_text(PRESET), "Off");
    }
    #[test]
    fn separation_rows_offer_dithergl_modes_and_matrix_settings() {
        use texted_app::visuals::{MatrixBackground, Separation};
        let mut ui = AppearanceUi::new(Appearance::default());
        ui.set_open(true);
        let separation = FIRST_EFFECT + Effect::Separation as usize;
        let names: Vec<String> = ui
            .choices(separation)
            .iter()
            .map(|c| c.label.clone())
            .collect();
        assert_eq!(names, Separation::ALL.map(|m| m.name().to_string()));
        pick(&mut ui, separation, "LED Matrix");
        pick(
            &mut ui,
            FIRST_EFFECT + Effect::MatrixSpacing as usize,
            "0.250",
        );
        pick(&mut ui, FIRST_EFFECT + Effect::InnerSquare as usize, "Off");
        pick(
            &mut ui,
            FIRST_EFFECT + Effect::MatrixBackground as usize,
            "Theme",
        );
        let e = ui.settings.visuals().effects;
        assert_eq!(
            (
                e.separation,
                e.matrix_spacing,
                e.inner_square,
                e.matrix_background
            ),
            (Separation::LedMatrix, 0.25, 0.0, MatrixBackground::Theme)
        );
        assert_eq!(ui.field_text(separation), "LED Matrix");
    }
    #[test]
    fn effect_rows_draw_a_gauge_where_it_fits() {
        let mut ui = AppearanceUi::new(Appearance::default());
        ui.set_open(true);
        let scan = FIRST_EFFECT + Effect::Scanlines as usize;
        pick(&mut ui, scan, "0.35");
        assert_eq!(ui.field_text(scan), "0.35");
        assert_eq!(ui.shown_text(scan, 30), "█▓░░░ 0.35");
        // A Game Boy row has room for the number only.
        assert_eq!(ui.shown_text(scan, 6), "0.35");
        let separation = FIRST_EFFECT + Effect::Separation as usize;
        assert_eq!(ui.shown_text(separation, 30), "Off");
        assert_eq!(ui.shown_text(PRESET, 30), "Custom");
    }
    #[test]
    fn a_theme_brings_its_visuals_and_reset_restores_them_without_touching_style() {
        let mut ui = AppearanceUi::new(Appearance::default());
        ui.set_open(true);
        pick(&mut ui, 0, Theme::ZxSpectrum.name());
        assert_eq!(ui.settings.visuals(), Machine::ZxSpectrum.visuals());
        pick(&mut ui, FIRST_EFFECT + Effect::Noise as usize, "0.10");
        ui.set_focus(FIRST_EFFECT + Effect::Noise as usize);
        ui.reset();
        assert_eq!(ui.settings.visuals(), Machine::ZxSpectrum.visuals());
        assert_eq!(ui.settings.theme, Theme::ZxSpectrum);
    }
    #[test]
    fn shadow_rows_switch_it_and_set_its_offsets() {
        let mut ui = AppearanceUi::new(Appearance::default());
        ui.set_open(true);
        pick(&mut ui, SHADOW, "On");
        pick(&mut ui, SHADOW_X, "2");
        pick(&mut ui, SHADOW_Y, "0");
        let v = ui.settings.visuals();
        assert_eq!((v.shadow, v.shadow_x, v.shadow_y), (true, 2, 0));
        assert_eq!(ui.field_text(SHADOW), "On");
        assert_eq!(ui.field_text(SHADOW_X), "2");
    }
    #[test]
    fn border_and_inset_rows_set_the_chrome() {
        use texted_app::visuals::Border;
        let mut ui = AppearanceUi::new(Appearance::default());
        ui.set_open(true);
        pick(&mut ui, BORDER, Border::Double.name());
        pick(&mut ui, CORNER, "24");
        pick(&mut ui, INSET_X, "3");
        pick(&mut ui, INSET_Y, "1");
        let v = ui.settings.visuals();
        assert_eq!(
            (v.border, v.corner_inset, v.inset_x, v.inset_y),
            (Border::Double, 24, 3, 1)
        );
        assert_eq!(ui.field_text(BORDER), "Double ═║");
    }
    #[test]
    fn visuals_page_lists_the_lace_rows() {
        let titles: Vec<&str> = (INSET_Y..FIELDS).map(|f| field_title(f, false)).collect();
        assert_eq!(
            titles,
            [
                "Inset Y",
                "Lace",
                "Lace density",
                "Lace strength",
                "Lace spacing",
                "Lace serifs",
                "Lace pattern"
            ]
        );
        assert_eq!(FIELDS, INSET_Y + 7);
        let mut ui = AppearanceUi::new(Appearance::default());
        ui.set_open(true);
        assert_eq!(ui.field_text(LACE), "Off");
        pick(&mut ui, LACE, "On");
        pick(&mut ui, LACE_DENSITY, "0.85");
        pick(&mut ui, LACE_STRENGTH, "0.50");
        pick(&mut ui, LACE_SPACING, "2 cells");
        pick(&mut ui, LACE_SERIFS, "Strokes");
        pick(&mut ui, LACE_SEED, "17");
        let l = ui.settings.visuals().lace;
        assert!(l.enabled && l.serifs);
        assert_eq!((l.spacing, l.seed), (2, 17));
        assert!((l.density - 0.85).abs() < 1e-6 && (l.strength - 0.5).abs() < 1e-6);
        assert_eq!(ui.field_text(LACE), "On");
        assert_eq!(ui.field_text(LACE_DENSITY), "0.85");
        assert_eq!(ui.field_text(LACE_SPACING), "2 cells");
        assert_eq!(ui.field_text(LACE_SERIFS), "Strokes");
        assert_eq!(ui.choices(LACE_DENSITY).len(), 21);
        assert_eq!(ui.choices(LACE_STRENGTH).len(), 20);
        assert_eq!(ui.choices(LACE_SEED).len(), 32);
        assert!(ui.shown_text(LACE_DENSITY, 40).starts_with("████"));
    }
    #[test]
    fn an_unavailable_face_blocks_the_lace_row() {
        let reason = "cell 4×6 is outside 6–32 × 8–64";
        let mut ui = AppearanceUi::new(Appearance::default());
        ui.set_open(true);
        ui.set_lace_unavailable(Some(reason.into()));
        assert_eq!(ui.field_text(LACE), format!("Unavailable: {reason}"));
        let on = Choice::new("On".into(), Value::Lace(true));
        ui.choose(&on);
        assert!(!ui.settings.visuals().lace.enabled);
        ui.set_lace_unavailable(None);
        assert_eq!(ui.field_text(LACE), "Off");
        ui.choose(&on);
        assert!(ui.settings.visuals().lace.enabled);
    }
    #[test]
    fn theme_and_machine_changes_keep_the_lace_settings() {
        use crate::visuals::Machine;
        let mut ui = AppearanceUi::new(Appearance::default());
        ui.set_open(true);
        pick(&mut ui, LACE, "On");
        pick(&mut ui, LACE_DENSITY, "0.85");
        pick(&mut ui, LACE_SPACING, "3 cells");
        pick(&mut ui, LACE_SEED, "9");
        let lace = ui.settings.visuals().lace;
        assert!(lace.enabled);
        let other = Theme::ALL
            .into_iter()
            .find(|t| *t != ui.settings.theme)
            .unwrap();
        pick(&mut ui, 0, other.name());
        assert_eq!(ui.settings.visuals().lace, lace);
        pick(&mut ui, PRESET, Preset::Machine(Machine::AtariSt).name());
        assert_eq!(ui.settings.visuals().lace, lace);
        ui.reset();
        assert_eq!(ui.settings.visuals().lace, lace);
    }
    #[test]
    fn terminals_show_visuals_as_window_only() {
        let mut ui = AppearanceUi::new(Appearance::default());
        ui.set_cell_backend(true);
        assert!(ui.choices(PRESET).iter().all(|c| !c.enabled));
        assert_eq!(ui.field_text(FIRST_EFFECT), "Window only");
    }
    #[test]
    fn compact_lists_commit_clicked_values_and_outside_clicks_dismiss_only_the_list() {
        let mut ui = AppearanceUi::new(Appearance {
            theme: Theme::C64,
            ..Default::default()
        });
        ui.set_open(true);
        ui.open_dropdown(1, 256, 192, true);
        let d = ui.dropdown.unwrap();
        let choices = ui.choices(1);
        let popup = ui.popup(d, 256, 192, true, &choices);
        let row = popup.row(7, d.first).unwrap();
        ui.click(256, 192, (row.x + 10) as f32, (row.y + 4) as f32, true);
        assert_eq!(ui.settings.framebuffer, Framebuffer::ZxSpectrum);
        assert!(ui.dropdown.is_none());
        ui.open_dropdown(0, 256, 192, true);
        let before = ui.settings.clone();
        ui.click(256, 192, 0.0, 0.0, true);
        assert!(ui.open && ui.dropdown.is_none());
        assert_eq!(ui.settings, before);
        ui.set_open(false);
        ui.set_open(true);
        assert!(ui.dropdown.is_none());
    }
    #[test]
    fn font_size_list_commits_a_valid_native_scale() {
        let mut ui = AppearanceUi::new(Appearance {
            theme: Theme::C64,
            ..Default::default()
        });
        ui.open_dropdown(3, 256, 192, true);
        press(&mut ui, NamedKey::End, true);
        press(&mut ui, NamedKey::Enter, true);
        assert_eq!(ui.settings.pixel_typography.scale_percent(), 800);
        assert!(ui.dropdown.is_none());
    }
    #[test]
    fn the_panel_fits_a_game_boy_screen_and_scrolls_every_page() {
        let (w, h) = (160, 144);
        let p = Panel::new(w, h, true);
        assert!(p.rect.y + p.rect.h <= h && p.rect.x + p.rect.w <= w);
        assert!(p.rows >= 1 && p.rows < ROWS);
        assert!(p.buttons + p.row_h + 4 + 8 <= p.rect.h);
        assert!(
            p.reset().x + p.reset().w < p.done().x,
            "Reset runs into Done"
        );
        let mut ui = AppearanceUi::new(Appearance::default());
        ui.set_open(true);
        for _ in 0..FIELDS {
            ui.key(&key_input(NamedKey::Tab, false), w, h, true);
            let slot = ui.slot_of(ui.focus).expect("focused field is shown");
            assert!(p.row(slot).y + p.row(slot).h <= p.rect.y + p.buttons);
        }
    }
    #[test]
    fn the_status_line_stays_inside_the_panel_below_seven_rows() {
        for (w, h, compact, text_h) in [(256, 176, true, 8), (980, 700, false, 16)] {
            let p = Panel::new(w, h, compact);
            let status_bottom = p.buttons + p.row_h + 4 + text_h;
            assert!(status_bottom <= p.rect.h, "{status_bottom} > {}", p.rect.h);
            assert!(p.rect.y + p.rect.h <= h);
        }
    }
    #[test]
    fn compact_panel_controls_fit_the_smallest_canvas_and_share_hit_geometry() {
        for (w, h) in [(256, 176), (256, 192), (320, 200), (640, 350), (720, 400)] {
            let p = Panel::new(w, h, true);
            assert!(p.rect.x + p.rect.w <= w && p.rect.y + p.rect.h <= h);
            for row in 0..ROWS {
                let r = p.row(row);
                assert!(r.x + r.w <= w && r.y + r.h <= h);
                assert_eq!(
                    hit(w, h, (r.x + 4) as f32, (r.y + 4) as f32, true),
                    Some(Hit::Field(row))
                );
                assert_eq!(
                    hit(w, h, (r.x + r.w - 4) as f32, (r.y + 4) as f32, true),
                    Some(Hit::Field(row))
                );
            }
            for (r, action) in [(p.done(), Hit::Close), (p.reset(), Hit::Reset)] {
                assert!(r.x + r.w <= w && r.y + r.h <= h);
                assert_eq!(
                    hit(w, h, (r.x + 4) as f32, (r.y + 4) as f32, true),
                    Some(action)
                );
            }
        }
    }
    #[test]
    fn palette_and_typography_controls_are_independent_and_pixel_sizes_are_integral() {
        let mut ui = AppearanceUi::new(Appearance::default());
        let typography = ui.settings.typography;
        ui.choose(&Choice::new("C64".into(), Value::Theme(Theme::C64)));
        assert_eq!(ui.settings.theme, Theme::C64);
        assert_eq!(ui.settings.typography, typography);
        assert_eq!(ui.settings.editor_typography().family, FontFamily::Bios);
        assert_eq!(ui.settings.editor_typography().size_px, 8);
        ui.zoom(1);
        assert_eq!(ui.settings.editor_typography().scale_percent(), 110);
        assert_eq!(ui.settings.typography, typography);
        assert_eq!(ui.settings.theme, Theme::C64);
    }
    #[test]
    fn keyboard_panel_consumes_text_without_touching_settings() {
        let mut ui = AppearanceUi::new(Appearance::default());
        let expected = ui
            .choices(2)
            .iter()
            .rev()
            .find(|c| c.enabled)
            .unwrap()
            .value;
        ui.open_dropdown(2, 980, 700, false);
        press(&mut ui, NamedKey::End, false);
        press(&mut ui, NamedKey::Enter, false);
        assert!(Value::Font(ui.settings.typography.family) == expected);
        let old = ui.settings.clone();
        ui.key(
            &KeyInput {
                physical: PhysicalKey::Code(KeyCode::KeyA),
                logical: &Key::Character("a".into()),
                text: Some("a"),
                modifiers: ModifiersState::empty(),
            },
            980,
            700,
            false,
        );
        assert_eq!(ui.settings, old);
        assert_eq!(hit(980, 700, 600.0, 700.0, false), Some(Hit::Close));
    }
}
