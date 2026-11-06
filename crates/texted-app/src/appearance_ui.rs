//! Appearance panel interaction and drawing, sharing native and compact framebuffer geometry.
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
    Adjust(usize, i32),
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
            return Some(Hit::Adjust(row, if x < (r.x + 20) as f32 { -1 } else { 1 }));
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
        }
    }
    fn adjust(&mut self, row: usize, delta: i32) {
        match row {
            0 => {
                self.settings.theme = self.settings.theme.step(delta);
                self.settings.colors.clear();
            }
            1 => self.settings.framebuffer = self.settings.framebuffer.step(delta),
            2 => {
                for _ in 0..4 {
                    let t = self.settings.editor_typography_mut();
                    t.family = t.family.step(delta);
                    let i = FontFamily::ALL.iter().position(|f| *f == t.family).unwrap();
                    if self.available[i] {
                        break;
                    }
                }
            }
            3 => self.settings.editor_typography_mut().resize(delta),
            _ => {
                let t = self.settings.editor_typography_mut();
                t.line_spacing = (i32::from(t.line_spacing) + delta * 10).clamp(100, 180) as u16;
            }
        }
        self.settings.editor_typography_mut().normalize();
        self.palette = self.settings.palette().unwrap();
    }
    pub fn zoom(&mut self, delta: i32) {
        self.adjust(3, delta);
    }
    pub fn reset_size(&mut self) {
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
        match hit(width, height, x, y, compact) {
            Some(Hit::Close) => self.open = false,
            Some(Hit::Reset) => {
                let open = self.open;
                let status = self.save_status.clone();
                *self = Self::new(Appearance::default(), self.available);
                self.open = open;
                self.save_status = status;
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
                self.focus = (self.focus + if input.modifiers.shift_key() { 4 } else { 1 }) % 5
            }
            Key::Named(NamedKey::ArrowUp) => self.focus = (self.focus + 4) % 5,
            Key::Named(NamedKey::ArrowDown) => self.focus = (self.focus + 1) % 5,
            Key::Named(NamedKey::ArrowLeft) => self.adjust(self.focus, -1),
            Key::Named(NamedKey::ArrowRight | NamedKey::Enter) => self.adjust(self.focus, 1),
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
                "Tab: field  Arrows: change"
            } else {
                "Tab: field   Arrows: adjust   Esc: close"
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
                (field.x + 3, text_y),
                "<",
                c.status_accent,
                12,
            );
            label(
                draws,
                font,
                (width, height),
                (field.x + if compact { 16 } else { 38 }, text_y),
                &value,
                c.text,
                field.w.saturating_sub(if compact { 30 } else { 72 }),
            );
            label(
                draws,
                font,
                (width, height),
                (
                    field.x + field.w.saturating_sub(if compact { 12 } else { 24 }),
                    text_y,
                ),
                ">",
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
                    Some(Hit::Adjust(row, -1))
                );
                assert_eq!(
                    hit(w, h, (r.x + r.w - 4) as f32, (r.y + 4) as f32, true),
                    Some(Hit::Adjust(row, 1))
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
        ui.adjust(0, 6);
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
        ui.adjust(2, 1);
        assert_eq!(ui.settings.typography.family, FontFamily::Pixel);
        let old = ui.settings.clone();
        ui.key(&KeyInput {
            physical: PhysicalKey::Code(KeyCode::KeyA),
            logical: &Key::Character("a".into()),
            text: Some("a"),
            modifiers: ModifiersState::empty(),
        });
        assert_eq!(ui.settings, old);
        assert_eq!(hit(980, 700, 600.0, 700.0, false), Some(Hit::Close));
    }
}
