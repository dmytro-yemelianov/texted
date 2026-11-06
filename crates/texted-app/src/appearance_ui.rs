//! Appearance panel interaction and drawing. Edits only appearance values, never a document.
use crate::graphics::{draw_box, DrawList, FontEngine};
use texted_app::{
    appearance::{Appearance, FontFamily, Palette, Typography},
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
pub(crate) fn status_button(width: usize, height: usize) -> Rect {
    Rect {
        x: width.saturating_sub(88),
        y: height.saturating_sub(26),
        w: 80.min(width),
        h: 24,
    }
}
fn panel(width: usize, height: usize) -> Rect {
    let w = width.saturating_sub(24).min(620);
    Rect {
        x: width.saturating_sub(w) / 2,
        y: height.saturating_sub(296) / 2,
        w,
        h: 284,
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hit {
    Close,
    Reset,
    Adjust(usize, i32),
}
fn hit(width: usize, height: usize, x: f32, y: f32) -> Option<Hit> {
    let p = panel(width, height);
    if !p.contains(x, y) {
        return Some(Hit::Close);
    }
    let x = x - p.x as f32;
    let y = y - p.y as f32;
    if y < 40.0 && x > p.w.saturating_sub(56) as f32 {
        return Some(Hit::Close);
    }
    for row in 0..4 {
        let top = 64 + row * 36;
        if (top as f32..(top + 28) as f32).contains(&y) {
            return Some(Hit::Adjust(
                row,
                if x > p.w.saturating_sub(52) as f32 {
                    1
                } else if (148.0..184.0).contains(&x) {
                    -1
                } else {
                    1
                },
            ));
        }
    }
    if (216.0..244.0).contains(&y) {
        return if x < 190.0 {
            Some(Hit::Reset)
        } else if x > p.w.saturating_sub(100) as f32 {
            Some(Hit::Close)
        } else {
            None
        };
    }
    None
}
pub(crate) struct AppearanceUi {
    pub settings: Appearance,
    pub palette: Palette,
    pub open: bool,
    pub save_status: String,
    focus: usize,
    available: [bool; 4],
}
impl AppearanceUi {
    pub fn new(mut settings: Appearance, available: [bool; 4]) -> Self {
        if !available[FontFamily::ALL
            .iter()
            .position(|f| *f == settings.typography.family)
            .unwrap()]
        {
            settings.typography.family = FontFamily::ALL
                .iter()
                .zip(available)
                .find(|(_, available)| *available)
                .map_or(FontFamily::Pixel, |(f, _)| *f);
        }
        settings.typography.normalize();
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
        }
    }
    fn adjust(&mut self, row: usize, delta: i32) {
        match row {
            0 => {
                self.settings.theme = self.settings.theme.step(delta);
                self.settings.colors.clear();
            }
            1 => {
                for _ in 0..4 {
                    self.settings.typography.family = self.settings.typography.family.step(delta);
                    let index = FontFamily::ALL
                        .iter()
                        .position(|f| *f == self.settings.typography.family)
                        .unwrap();
                    if self.available[index] {
                        break;
                    }
                }
            }
            2 => self.settings.typography.resize(delta),
            _ => {
                self.settings.typography.line_spacing =
                    (i32::from(self.settings.typography.line_spacing) + delta * 10).clamp(100, 180)
                        as u16
            }
        }
        self.settings.typography.normalize();
        self.palette = self.settings.palette().unwrap();
    }
    pub fn zoom(&mut self, delta: i32) {
        self.adjust(2, delta);
    }
    pub fn reset_size(&mut self) {
        self.settings.typography.size_px = Typography::default().size_px;
        self.settings.typography.normalize();
    }
    pub fn click(&mut self, width: usize, height: usize, x: f32, y: f32) {
        match hit(width, height, x, y) {
            Some(Hit::Close) => self.open = false,
            Some(Hit::Reset) => {
                let available = self.available;
                let open = self.open;
                let save_status = self.save_status.clone();
                *self = Self::new(Appearance::default(), available);
                self.open = open;
                self.save_status = save_status;
            }
            Some(Hit::Adjust(row, delta)) => {
                self.focus = row;
                self.adjust(row, delta);
            }
            None => {}
        }
    }
    pub fn key(&mut self, input: &KeyInput<'_>) {
        match input.logical {
            Key::Named(NamedKey::Escape) => self.open = false,
            Key::Named(NamedKey::Tab) => {
                self.focus = (self.focus + if input.modifiers.shift_key() { 3 } else { 1 }) % 4
            }
            Key::Named(NamedKey::ArrowUp) => self.focus = (self.focus + 3) % 4,
            Key::Named(NamedKey::ArrowDown) => self.focus = (self.focus + 1) % 4,
            Key::Named(NamedKey::ArrowLeft) => self.adjust(self.focus, -1),
            Key::Named(NamedKey::ArrowRight | NamedKey::Enter) => self.adjust(self.focus, 1),
            _ => {}
        }
    }
    pub fn paint(&self, draws: &mut DrawList, font: &mut FontEngine, width: usize, height: usize) {
        if !self.open {
            return;
        }
        let p = panel(width, height);
        let c = self.palette;
        draw_box(
            draws,
            width,
            height,
            p.x,
            p.y,
            p.w,
            p.h,
            c.panel_bg,
            c.panel_border,
        );
        label(
            draws,
            font,
            (width, height),
            (p.x + 16, p.y + 14),
            "Appearance",
            c.text,
            p.w.saturating_sub(70),
        );
        label(
            draws,
            font,
            (width, height),
            (p.x + p.w.saturating_sub(36), p.y + 14),
            "X",
            c.text,
            28,
        );
        label(
            draws,
            font,
            (width, height),
            (p.x + 16, p.y + 40),
            "Tab: field   Arrows: adjust   Esc: close",
            c.status_text,
            p.w.saturating_sub(28),
        );
        let values = [
            format!(
                "{}{}",
                self.settings.theme.name(),
                if self.settings.colors.is_empty() {
                    ""
                } else {
                    " (custom)"
                }
            ),
            self.settings.typography.family.name().to_owned(),
            format!("{} px", self.settings.typography.size_px),
            format!("{}%", self.settings.typography.line_spacing),
        ];
        for (row, (title, value)) in ["Theme", "Editor font", "Font size", "Line height"]
            .iter()
            .zip(values)
            .enumerate()
        {
            let y = p.y + 64 + row * 36;
            label(
                draws,
                font,
                (width, height),
                (p.x + 16, y + 5),
                title,
                c.text,
                130,
            );
            let w = p.w.saturating_sub(164);
            draw_box(
                draws,
                width,
                height,
                p.x + 148,
                y,
                w,
                28,
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
                (p.x + 158, y + 5),
                "<",
                c.status_accent,
                18,
            );
            label(
                draws,
                font,
                (width, height),
                (p.x + 186, y + 5),
                &value,
                c.text,
                w.saturating_sub(72),
            );
            label(
                draws,
                font,
                (width, height),
                (p.x + p.w.saturating_sub(40), y + 5),
                ">",
                c.status_accent,
                20,
            );
        }
        draw_box(
            draws,
            width,
            height,
            p.x + 16,
            p.y + 216,
            170.min(p.w.saturating_sub(32)),
            28,
            c.button_bg,
            c.button_border,
        );
        label(
            draws,
            font,
            (width, height),
            (p.x + 25, p.y + 221),
            "Reset defaults",
            c.text,
            150,
        );
        draw_box(
            draws,
            width,
            height,
            p.x + p.w.saturating_sub(96),
            p.y + 216,
            80,
            28,
            c.button_bg,
            c.button_border,
        );
        label(
            draws,
            font,
            (width, height),
            (p.x + p.w.saturating_sub(80), p.y + 221),
            "Done",
            c.text,
            64,
        );
        label(
            draws,
            font,
            (width, height),
            (p.x + 16, p.y + 250),
            &self.save_status,
            c.status_text,
            p.w.saturating_sub(28),
        );
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
) {
    let r = status_button(width, height);
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
        (r.x + 12, r.y + 3),
        "Style",
        c.text,
        r.w.saturating_sub(18),
    );
}
#[cfg(test)]
mod tests {
    use super::*;
    use texted_app::appearance::Theme;
    use winit::keyboard::{KeyCode, ModifiersState, PhysicalKey};
    #[test]
    fn palette_and_typography_controls_are_independent_and_pixel_sizes_are_integral() {
        let mut ui = AppearanceUi::new(Appearance::default(), [true; 4]);
        let typography = ui.settings.typography;
        ui.adjust(0, 6);
        assert_eq!(ui.settings.theme, Theme::C64);
        assert_eq!(ui.settings.typography, typography);
        ui.adjust(1, -1);
        assert_eq!(ui.settings.typography.family, FontFamily::Pixel);
        assert_eq!(ui.settings.typography.size_px, 16);
        ui.zoom(1);
        assert_eq!(ui.settings.typography.size_px, 24);
        assert_eq!(ui.settings.theme, Theme::C64);
    }
    #[test]
    fn keyboard_panel_consumes_text_without_touching_settings_and_skips_unavailable_fonts() {
        let mut ui = AppearanceUi::new(Appearance::default(), [true, false, false, true]);
        ui.adjust(1, 1);
        assert_eq!(ui.settings.typography.family, FontFamily::Pixel);
        let old = ui.settings.clone();
        ui.key(&KeyInput {
            physical: PhysicalKey::Code(KeyCode::KeyA),
            logical: &Key::Character("a".into()),
            text: Some("a"),
            modifiers: ModifiersState::empty(),
        });
        assert_eq!(ui.settings, old);
        assert_eq!(hit(980, 700, 600.0, 700.0), Some(Hit::Close));
    }
}
