//! Appearance panel interaction and drawing, sharing native and compact framebuffer geometry.
use crate::{
    dropdown::{Dropdown, Popup},
    graphics::{draw_box, draw_rect, DrawList, FontEngine},
};
use texted_app::{
    appearance::{Appearance, FontFamily, Palette, Theme, Typography},
    framebuffer::Framebuffer,
    input::KeyInput,
};
use winit::keyboard::{Key, NamedKey};
#[derive(Clone, Copy)]
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
pub(crate) fn status_button(width: usize, height: usize, compact: bool) -> Rect {
    let (w, h, pad) = if compact { (44, 12, 2) } else { (80, 24, 8) };
    Rect {
        x: width.saturating_sub(w + pad),
        y: height.saturating_sub(h + 2),
        w: w.min(width),
        h,
    }
}
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
            (400, 184, 8, 72, 32, 22, 18, 8)
        } else {
            (620, 320, 24, 148, 64, 36, 28, 16)
        };
        let w = width.saturating_sub(margin).min(limit);
        Self {
            rect: Rect {
                x: width.saturating_sub(w) / 2,
                y: height.saturating_sub(h) / 2,
                w,
                h,
            },
            input,
            row_top,
            step,
            row_h,
            pad,
            buttons: row_top + 5 * step + 4,
        }
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
            w: if self.pad == 8 { 96 } else { 170 },
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
fn hit(width: usize, height: usize, x: f32, y: f32, compact: bool) -> Option<Hit> {
    let p = Panel::new(width, height, compact);
    if !p.rect.contains(x, y)
        || (y < (p.rect.y + 24) as f32 && x > (p.rect.x + p.rect.w.saturating_sub(28)) as f32)
    {
        return Some(Hit::Close);
    }
    for row in 0..5 {
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
    pub save_status: String,
    focus: usize,
    available: [bool; 4],
    dropdown: Option<Dropdown>,
}
impl AppearanceUi {
    pub fn new(mut settings: Appearance, available: [bool; 4]) -> Self {
        for typography in [&mut settings.typography, &mut settings.pixel_typography] {
            if !available[FontFamily::ALL
                .iter()
                .position(|f| *f == typography.family)
                .unwrap()]
            {
                typography.family = FontFamily::ALL
                    .iter()
                    .zip(available)
                    .find(|(_, a)| *a)
                    .map_or(FontFamily::Pixel, |(f, _)| *f);
            }
            typography.normalize();
        }
        let palette = settings
            .palette()
            .expect("Appearance was validated at load");
        Self {
            settings,
            palette,
            open: false,
            save_status: "Changes save automatically".into(),
            focus: 0,
            available,
            dropdown: None,
        }
    }
    pub fn set_open(&mut self, open: bool) {
        self.open = open;
        self.dropdown = None;
    }
    pub fn dismiss_dropdown(&mut self) -> bool {
        self.dropdown.take().is_some()
    }
    fn selected(&self, row: usize) -> Value {
        let t = self.settings.editor_typography();
        match row {
            0 => Value::Theme(self.settings.theme),
            1 => Value::Screen(self.settings.framebuffer),
            2 => Value::Font(t.family),
            3 => Value::Size(t.size_px),
            _ => Value::Spacing(t.line_spacing),
        }
    }
    fn choices(&self, row: usize) -> Vec<Choice> {
        match row {
            0 => Theme::ALL
                .into_iter()
                .map(|v| Choice::new(v.name().into(), Value::Theme(v)))
                .collect(),
            1 => Framebuffer::ALL
                .into_iter()
                .map(|v| Choice::new(v.name().into(), Value::Screen(v)))
                .collect(),
            2 => FontFamily::ALL
                .into_iter()
                .zip(self.available)
                .map(|(v, enabled)| Choice {
                    label: if enabled {
                        v.name().into()
                    } else {
                        format!("{} (unavailable)", v.name())
                    },
                    value: Value::Font(v),
                    enabled,
                })
                .collect(),
            row => {
                let t = self.settings.editor_typography();
                let mut numbers: Vec<u16> = if row == 3 {
                    if t.family == FontFamily::Pixel {
                        (8..=40).step_by(8).collect()
                    } else {
                        (8..=40).collect()
                    }
                } else {
                    (100..=180).step_by(10).collect()
                };
                let selected = if row == 3 { t.size_px } else { t.line_spacing };
                if !numbers.contains(&selected) {
                    numbers.push(selected);
                    numbers.sort_unstable();
                }
                numbers
                    .into_iter()
                    .map(|v| {
                        if row == 3 {
                            Choice::new(format!("{v} px"), Value::Size(v))
                        } else {
                            Choice::new(format!("{v}%"), Value::Spacing(v))
                        }
                    })
                    .collect()
            }
        }
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
            Value::Font(font) => self.settings.editor_typography_mut().family = font,
            Value::Size(size) => self.settings.editor_typography_mut().size_px = size,
            Value::Spacing(spacing) => self.settings.editor_typography_mut().line_spacing = spacing,
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
        let label_width = choices
            .iter()
            .map(|c| c.label.chars().count() * cell)
            .max()
            .unwrap_or(0);
        Popup::new(
            Panel::new(width, height, compact).row(d.field),
            (width, height),
            compact,
            choices.len(),
            label_width,
        )
    }
    fn open_dropdown(&mut self, row: usize, width: usize, height: usize, compact: bool) {
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
        self.dropdown = None;
        self.settings.editor_typography_mut().resize(delta);
    }
    pub fn reset_size(&mut self) {
        self.dropdown = None;
        let size = if self.settings.resolution().is_some() {
            8
        } else {
            Typography::default().size_px
        };
        let t = self.settings.editor_typography_mut();
        t.size_px = size;
        t.normalize();
    }
    pub fn click(&mut self, width: usize, height: usize, x: f32, y: f32, compact: bool) {
        if let Some(d) = self.dropdown {
            let choices = self.choices(d.field);
            let popup = self.popup(d, width, height, compact, &choices);
            if let Some(index) = popup.item_at(x, y, d.first, choices.len()) {
                if choices[index].enabled {
                    self.choose(&choices[index]);
                    self.dropdown = None;
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
            self.dropdown = None;
            if let Some(Hit::Field(row)) = hit(width, height, x, y, compact) {
                if row != d.field {
                    self.open_dropdown(row, width, height, compact);
                }
            }
            return;
        }
        match hit(width, height, x, y, compact) {
            Some(Hit::Close) => self.set_open(false),
            Some(Hit::Reset) => {
                let open = self.open;
                let status = self.save_status.clone();
                *self = Self::new(Appearance::default(), self.available);
                self.open = open;
                self.save_status = status;
            }
            Some(Hit::Field(row)) => self.open_dropdown(row, width, height, compact),
            None => {}
        }
    }
    pub fn key(&mut self, input: &KeyInput<'_>, width: usize, height: usize, compact: bool) {
        let activate = matches!(input.logical, Key::Named(NamedKey::Enter))
            || matches!(input.logical,Key::Character(s) if s==" ");
        if let Some(mut d) = self.dropdown {
            let choices = self.choices(d.field);
            let enabled: Vec<_> = choices.iter().map(|c| c.enabled).collect();
            let popup = self.popup(d, width, height, compact, &choices);
            match input.logical {
                Key::Named(NamedKey::Escape) => {
                    self.dropdown = None;
                    return;
                }
                Key::Named(NamedKey::Tab) => {
                    self.dropdown = None;
                    self.focus = (self.focus + if input.modifiers.shift_key() { 4 } else { 1 }) % 5;
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
                    self.choose(&choices[d.highlighted]);
                    self.dropdown = None;
                    return;
                }
                _ => return,
            }
            d.reveal(popup.visible, choices.len());
            self.dropdown = Some(d);
            return;
        }
        match input.logical {
            Key::Named(NamedKey::Escape) => self.set_open(false),
            Key::Named(NamedKey::Tab) => {
                self.focus = (self.focus + if input.modifiers.shift_key() { 4 } else { 1 }) % 5
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
        &self,
        draws: &mut DrawList,
        font: &mut FontEngine,
        width: usize,
        height: usize,
        compact: bool,
    ) {
        if !self.open {
            return;
        }
        let p = Panel::new(width, height, compact);
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
        let top = if compact { 4 } else { 14 };
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
            (r.x + p.pad, r.y + if compact { 18 } else { 40 }),
            if compact {
                "Tab: field  Enter: list"
            } else {
                "Tab: field  Enter: list  Esc: close"
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
        let values = [
            self.settings.theme.name().to_owned(),
            screen,
            t.family.name().into(),
            format!("{} px", t.size_px),
            format!("{}%", t.line_spacing),
        ];
        let titles = if compact {
            ["Theme", "Screen", "Font", "Size", "Spacing"]
        } else {
            [
                "Theme",
                "Resolution",
                "Editor font",
                "Font size",
                "Line height",
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
            &self.save_status,
            c.status_text,
            r.w.saturating_sub(2 * p.pad),
        );
        if let Some(d) = self.dropdown {
            self.paint_dropdown(draws, font, width, height, compact, d);
        }
    }
    fn paint_dropdown(
        &self,
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
                row.w.saturating_sub(if compact { 16 } else { 24 }),
            );
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
pub(crate) fn paint_status_button(
    draws: &mut DrawList,
    font: &mut FontEngine,
    width: usize,
    height: usize,
    c: &Palette,
    compact: bool,
) {
    let r = status_button(width, height, compact);
    draw_box(
        draws,
        width,
        height,
        r.x,
        r.y,
        r.w,
        r.h,
        c.button_bg,
        c.button_border,
    );
    label(
        draws,
        font,
        (width, height),
        (
            r.x + if compact { 2 } else { 12 },
            r.y + if compact { 2 } else { 3 },
        ),
        "Style",
        c.text,
        r.w.saturating_sub(if compact { 4 } else { 18 }),
    );
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
    fn browsing_and_cancelling_a_list_preserves_values_and_custom_colors() {
        let mut settings = Appearance::default();
        settings.colors.insert("text".into(), "#ABCDEF".into());
        let mut ui = AppearanceUi::new(settings, [true; 4]);
        ui.set_open(true);
        let original = ui.settings.clone();
        let r = Panel::new(980, 700, false).row(0);
        ui.click(980, 700, (r.x + 20) as f32, (r.y + 10) as f32, false);
        assert!(ui.dropdown.is_some());
        assert_eq!(ui.settings, original);
        press(&mut ui, NamedKey::End, false);
        assert_eq!(ui.dropdown.unwrap().highlighted, 9);
        assert_eq!(ui.settings, original);
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
    fn disabled_fonts_cannot_be_clicked_and_keyboard_selection_skips_them() {
        let mut ui = AppearanceUi::new(Appearance::default(), [true, false, false, true]);
        ui.set_open(true);
        ui.open_dropdown(2, 980, 700, false);
        let d = ui.dropdown.unwrap();
        let choices = ui.choices(2);
        let popup = ui.popup(d, 980, 700, false, &choices);
        let row = popup.row(1, d.first).unwrap();
        ui.click(980, 700, (row.x + 10) as f32, (row.y + 10) as f32, false);
        assert_eq!(ui.settings.typography.family, FontFamily::Courier);
        assert!(ui.dropdown.is_some());
        press(&mut ui, NamedKey::ArrowDown, false);
        assert_eq!(ui.dropdown.unwrap().highlighted, 3);
        assert_eq!(ui.settings.typography.family, FontFamily::Courier);
        press(&mut ui, NamedKey::Enter, false);
        assert_eq!(ui.settings.typography.family, FontFamily::Pixel);
    }
    #[test]
    fn compact_lists_commit_clicked_values_and_outside_clicks_dismiss_only_the_list() {
        let mut ui = AppearanceUi::new(
            Appearance {
                theme: Theme::C64,
                ..Default::default()
            },
            [true; 4],
        );
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
    fn long_size_lists_scroll_and_keyboard_navigation_reveals_and_commits_the_last_value() {
        let mut ui = AppearanceUi::new(
            Appearance {
                theme: Theme::C64,
                ..Default::default()
            },
            [true; 4],
        );
        ui.settings.pixel_typography.family = FontFamily::Courier;
        ui.open_dropdown(3, 256, 192, true);
        ui.wheel(100.0, 256, 192, true);
        let d = ui.dropdown.unwrap();
        let choices = ui.choices(3);
        let popup = ui.popup(d, 256, 192, true, &choices);
        assert_eq!(d.first + popup.visible, choices.len());
        assert_eq!(ui.settings.pixel_typography.size_px, 8);
        press(&mut ui, NamedKey::End, true);
        press(&mut ui, NamedKey::PageDown, true);
        let d = ui.dropdown.unwrap();
        assert_eq!(d.highlighted, choices.len() - 1);
        assert!(popup.row(d.highlighted, d.first).is_some());
        press(&mut ui, NamedKey::Enter, true);
        assert_eq!(ui.settings.pixel_typography.size_px, 40);
    }
    #[test]
    fn compact_panel_controls_fit_the_smallest_canvas_and_share_hit_geometry() {
        for (w, h) in [(256, 192), (320, 200), (640, 350), (720, 400)] {
            let p = Panel::new(w, h, true);
            assert!(p.rect.x + p.rect.w <= w && p.rect.y + p.rect.h <= h);
            for row in 0..5 {
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
        let mut ui = AppearanceUi::new(Appearance::default(), [true; 4]);
        let typography = ui.settings.typography;
        ui.choose(&Choice::new("C64".into(), Value::Theme(Theme::C64)));
        assert_eq!(ui.settings.theme, Theme::C64);
        assert_eq!(ui.settings.typography, typography);
        assert_eq!(ui.settings.editor_typography().family, FontFamily::Pixel);
        assert_eq!(ui.settings.editor_typography().size_px, 8);
        ui.zoom(1);
        assert_eq!(ui.settings.editor_typography().size_px, 16);
        assert_eq!(ui.settings.typography, typography);
        assert_eq!(ui.settings.theme, Theme::C64);
    }
    #[test]
    fn keyboard_panel_consumes_text_without_touching_settings_and_skips_unavailable_fonts() {
        let mut ui = AppearanceUi::new(Appearance::default(), [true, false, false, true]);
        ui.open_dropdown(2, 980, 700, false);
        press(&mut ui, NamedKey::End, false);
        press(&mut ui, NamedKey::Enter, false);
        assert_eq!(ui.settings.typography.family, FontFamily::Pixel);
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
