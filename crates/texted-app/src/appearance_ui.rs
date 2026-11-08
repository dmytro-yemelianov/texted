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

const ROWS: usize = 7;
struct Panel {
    rect: Rect,
    input: usize,
    row_top: usize,
    step: usize,
    row_h: usize,
    pad: usize,
    buttons: usize,
}
impl Panel {
    fn new(width: usize, height: usize, compact: bool) -> Self {
        let (limit, h, margin, input, row_top, step, row_h, pad) = if compact {
            (400, 168, 8, 72, 32, 16, 16, 8)
        } else {
            (704, 352, 16, 152, 64, 32, 32, 16)
        };
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
            buttons: row_top + ROWS * step + if compact { 0 } else { 16 },
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
        Rect {
            x: self.rect.x + self.pad,
            y: self.rect.y + self.buttons,
            w: if self.pad == 8 { 96 } else { 168 },
            h: self.row_h,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hit {
    Close,
    Reset,
    Field(usize),
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
    for row in 0..ROWS {
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
    Crt(texted_app::framebuffer::CrtEffect),
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
    dropdown: Option<Dropdown>,
    font_query: String,
    origin: Option<Appearance>,
    accepted: bool,
    position: Option<(f32, f32)>,
    drag: Option<(f32, f32, f32, f32)>,
    previews: Vec<(FontFamily, FontEngine, u64)>,
    preview_clock: u64,
    font_choices: RefCell<Option<(String, Arc<Vec<Choice>>)>>,
}
impl AppearanceUi {
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
            dropdown: None,
            font_query: String::new(),
            origin: None,
            accepted: false,
            position: None,
            drag: None,
            previews: Vec::new(),
            preview_clock: 0,
            font_choices: RefCell::new(None),
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
        Panel::placed(width, height, compact, self.position)
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
            _ => Value::Crt(self.settings.crt),
        }
    }
    fn choices(&self, row: usize) -> Arc<Vec<Choice>> {
        // Terminals draw their own pointer, and their pictures skip the CRT pass.
        if (self.cell_backend && row != 0) || (self.terminal_graphics && row >= 5) {
            return Arc::new(vec![Choice {
                label: "Managed by the terminal emulator".into(),
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
            6 => texted_app::framebuffer::CrtEffect::ALL
                .into_iter()
                .map(|c| Choice::new(c.name().into(), Value::Crt(c)))
                .collect(),
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
                }
            }
            Value::Screen(screen) => self.settings.framebuffer = screen,
            Value::Crt(crt) => self.settings.crt = crt,
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
            self.panel(width, height, compact).row(d.field),
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
        let mut d = Dropdown::new(row, selected);
        let popup = self.popup(d, width, height, compact, &choices);
        d.reveal(popup.visible, choices.len());
        self.focus = row;
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
            return false;
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
            if let Some(Hit::Field(row)) = hit_panel(&self.panel(width, height, compact), x, y) {
                if row != d.field {
                    self.open_dropdown(row, width, height, compact);
                }
            }
            return;
        }
        match hit_panel(&self.panel(width, height, compact), x, y) {
            Some(Hit::Close) => self.set_open(false),
            Some(Hit::Reset) => {
                self.reset_defaults();
            }
            Some(Hit::Field(row)) => self.open_dropdown(row, width, height, compact),
            None => {}
        }
    }
    pub fn key(&mut self, input: &KeyInput<'_>, width: usize, height: usize, compact: bool) {
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
                    self.focus = (self.focus
                        + if input.modifiers.shift_key() {
                            ROWS - 1
                        } else {
                            1
                        })
                        % ROWS;
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
            Key::Named(NamedKey::Tab) => {
                self.focus = (self.focus
                    + if input.modifiers.shift_key() {
                        ROWS - 1
                    } else {
                        1
                    })
                    % ROWS
            }
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
        label(
            draws,
            font,
            (width, height),
            (r.x + p.pad, r.y + top),
            "Appearance",
            c.text,
            r.w.saturating_sub(40),
        );
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
            } else if compact {
                "Tab: field  Enter: list".into()
            } else {
                "Tab: field  Enter: list  Esc: close".into()
            },
            c.status_text,
            r.w.saturating_sub(2 * p.pad),
        );
        let t = self.settings.editor_typography();
        let screen = if self.settings.framebuffer == texted_app::framebuffer::Framebuffer::Auto {
            self.settings
                .resolution()
                .map_or("Auto: Native".into(), |(w, h)| format!("Auto: {w}x{h}"))
        } else {
            self.settings.framebuffer.name().into()
        };
        let mut values = [
            self.settings.theme.name().to_owned(),
            screen,
            t.family.name().into(),
            format!("{}%", t.scale_percent()),
            format!("{}%", t.line_spacing),
            self.settings.mouse_cursor.name().into(),
            self.settings.crt.name().into(),
        ];
        if self.cell_backend {
            for value in &mut values[1..] {
                *value = "Terminal setting".into();
            }
        }
        if self.terminal_graphics {
            values[5] = "Terminal pointer".into();
            values[6] = "Window only".into();
        }
        let titles = if compact {
            [
                "Theme", "Screen", "Font", "Size", "Spacing", "Cursor", "CRT",
            ]
        } else {
            [
                "Theme",
                "Resolution",
                "Editor font",
                "Text scale",
                "Line height",
                "Mouse cursor",
                "CRT effect",
            ]
        };
        for (row, (title, value)) in titles.iter().zip(values).enumerate() {
            let field = p.row(row);
            let text_y = field.y + if compact { 4 } else { 5 };
            label(
                draws,
                font,
                (width, height),
                (r.x + p.pad, text_y),
                title,
                c.text,
                p.input.saturating_sub(2 * p.pad),
            );
            draw_box(
                draws,
                width,
                height,
                field.x,
                field.y,
                field.w,
                field.h,
                c.input_bg,
                if self.focus == row {
                    c.input_active
                } else {
                    c.input_border
                },
            );
            label(
                draws,
                font,
                (width, height),
                (field.x + if compact { 4 } else { 10 }, text_y),
                &value,
                c.text,
                field.w.saturating_sub(if compact { 20 } else { 38 }),
            );
            label(
                draws,
                font,
                (width, height),
                (
                    field.x + field.w.saturating_sub(if compact { 12 } else { 24 }),
                    text_y,
                ),
                "v",
                c.status_accent,
                16,
            );
        }
        for (rect, text) in [
            (p.reset(), if compact { "Reset" } else { "Reset defaults" }),
            (p.done(), "Done"),
        ] {
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
        assert_eq!(ui.dropdown.unwrap().highlighted, 9);
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
        assert_eq!(ui.settings.theme, Theme::GreenCrt);
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
    #[test]
    fn the_crt_row_offers_three_strengths_and_keeps_the_choice() {
        use texted_app::framebuffer::CrtEffect;
        let mut ui = AppearanceUi::new(Appearance {
            theme: Theme::C64,
            ..Default::default()
        });
        let labels: Vec<_> = ui.choices(6).iter().map(|c| c.label.clone()).collect();
        assert_eq!(labels, ["Off", "Subtle", "Strong", "Wallpaper", "Custom"]);
        ui.set_open(true);
        ui.open_dropdown(6, 320, 200, true);
        let d = ui.dropdown.unwrap();
        let choices = ui.choices(6);
        let popup = ui.popup(d, 320, 200, true, &choices);
        let row = popup.row(2, d.first).unwrap();
        ui.click(320, 200, (row.x + 10) as f32, (row.y + 4) as f32, true);
        assert_eq!(ui.settings.crt, CrtEffect::Strong);
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
