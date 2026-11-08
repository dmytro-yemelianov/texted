//! Window chrome uses scene coordinates; document input and drawing use a translated viewport.
use crate::graphics::{draw_rect, DrawList, FontEngine};
use std::borrow::Cow;
use texted_app::appearance::Palette;
#[derive(Clone, Copy)]
pub(crate) struct Geometry {
    pub width: usize,
    pub height: usize,
    pub top: usize,
    pub bottom: usize,
    pub columns: usize,
    pub compact: bool,
}
#[derive(Debug, PartialEq)]
pub(crate) enum Hit {
    Tab(usize),
    Close(usize),
    New,
    Previous,
    Next,
    Command(usize),
    Consumed,
}
pub(crate) struct Context {
    pub search: texted_app::input::SearchContext,
    pub appearance: bool,
    pub dialog: bool,
    pub dialog_accept: &'static str,
    pub multiple_tabs: bool,
    pub selected: bool,
    pub can_undo: bool,
    pub can_redo: bool,
    pub editable: bool,
    pub has_query: bool,
    pub scalable: bool,
    pub zoom_percent: u16,
    pub statistics: String,
    pub statistics_short: String,
    pub cursor: (usize, usize),
    pub wrap: bool,
}
pub(crate) fn command(
    index: usize,
    context: &Context,
    modifiers: winit::keyboard::ModifiersState,
) -> Option<texted_app::input::Intent> {
    use texted_app::input::Intent as I;
    if context.dialog {
        return Some(match index {
            0 => I::ToggleCommands,
            1 => I::AcceptDialog,
            2 => I::NavigateDialog { forward: false },
            3 => I::NavigateDialog { forward: true },
            9 => I::CancelDialog,
            _ => return None,
        });
    }
    if context.appearance
        && !(modifiers.control_key() && !modifiers.super_key() && !modifiers.alt_key())
    {
        return Some(match index {
            0 => I::ToggleCommands,
            1 => I::AcceptAppearance,
            2 => I::BrowseAppearance { forward: false },
            3 => I::BrowseAppearance { forward: true },
            8 => I::ToggleAppearance,
            9 => I::ResetAppearance,
            _ => return None,
        });
    }
    let intent = texted_app::input::function_key_for(index, modifiers, context.search)?;
    if !context.scalable && matches!(intent, I::Zoom(_) | I::ResetFontSize) {
        None
    } else {
        Some(intent)
    }
}
pub(crate) struct Commands {
    pub items: Vec<(usize, Cow<'static, str>)>,
    tips: Vec<(usize, String)>,
}
impl Commands {
    pub fn key_at(&self, geometry: Geometry, slot: usize) -> Option<usize> {
        if geometry.columns == 12 {
            self.items
                .iter()
                .find(|(key, _)| *key == slot)
                .map(|(key, _)| *key)
        } else {
            self.items.get(slot).map(|(key, _)| *key)
        }
    }

    pub fn tip_at(&self, g: Geometry, x: f32, y: f32) -> Option<crate::status_ui::Tip> {
        let slot = g.command_slot(x, y)?;
        let key = self.key_at(g, slot)?;
        let text = self.tips.iter().find(|(index, _)| *index == key)?.1.clone();
        Some(crate::status_ui::Tip {
            rect: g.command_rect(slot),
            text,
        })
    }
    pub fn new(context: &Context, modifiers: winit::keyboard::ModifiersState) -> Self {
        use texted_app::{
            commands::EditorCommand as E,
            input::{FileIntent as F, Intent as I},
        };
        let mut items = Vec::new();
        let mut tips = Vec::new();
        let data_layer = modifiers.control_key() && !modifiers.super_key() && !modifiers.alt_key();
        for index in 0..12 {
            let Some(intent) = command(index, context, modifiers) else {
                continue;
            };
            let value = match &intent {
                I::ResetFontSize => Some(format!("{}%", context.zoom_percent)),
                I::DocumentInfo if data_layer => Some(if index == 9 {
                    format!("{}:{}", context.cursor.0, context.cursor.1)
                } else {
                    context.statistics_short.clone()
                }),
                I::ToggleWrap if data_layer => {
                    Some(if context.wrap { "W:on" } else { "W:off" }.into())
                }
                _ => None,
            };
            let description = match &intent {
                I::ResetFontSize => format!(
                    "Reset text scale to 100% (currently {}%)",
                    context.zoom_percent
                ),
                I::Zoom(delta) => format!(
                    "{} text scale by 10% (currently {}%)",
                    if *delta > 0 { "Increase" } else { "Decrease" },
                    context.zoom_percent
                ),
                I::DocumentInfo => format!(
                    "{}; line {}, column {}. Open document details",
                    context.statistics, context.cursor.0, context.cursor.1
                ),
                I::ToggleAppearance => "Fonts, themes, screen and mouse cursors (Cmd+,)".into(),
                I::ToggleWrap => format!(
                    "Line wrapping is {}. Click to toggle",
                    if context.wrap { "on" } else { "off" }
                ),
                I::Find => "Find text (Cmd+F)".into(),
                I::ToggleIgnoreCase => if context.search.ignore_case {
                    "Capital letters are ignored. Click to match case"
                } else {
                    "Capital letters must match. Click to ignore case"
                }
                .into(),
                I::ToggleWholeWord => if context.search.whole_word {
                    "Only whole words match. Click to match inside words too"
                } else {
                    "Matches inside words count. Click to match whole words only"
                }
                .into(),
                I::Replace => "Find and replace text (Cmd+R)".into(),
                I::File(F::Close) => "Close this tab; ask to save changed text (Cmd+W)".into(),
                I::File(F::Quit) => "Quit; check all changed tabs (Cmd+Q)".into(),
                I::File(F::Save { as_new }) => if *as_new {
                    "Save this tab as a new file"
                } else {
                    "Save this tab (Cmd+S)"
                }
                .into(),
                I::ToggleCommands => {
                    "Show or hide the command panel (F1). Hold Ctrl for scale and statistics".into()
                }
                _ => String::new(),
            };
            let (label, available) = match intent {
                I::ToggleCommands => ("Keys", true),
                I::File(F::Save { as_new }) => (if as_new { "As" } else { "Save" }, true),
                I::File(F::New) => ("New", true),
                I::File(F::Open) => ("Open", true),
                I::File(F::Close) => ("Cls", true),
                I::File(F::Quit) => ("Quit", true),
                I::Find => ("Find", context.editable),
                I::Replace => ("Repl", context.editable),
                I::NextMatch { forward } => {
                    (if forward { "Next" } else { "Prev" }, context.has_query)
                }
                I::ReplaceMatch { all } => (if all { "All" } else { "Repl" }, context.has_query),
                I::NextTab { forward } => {
                    (if forward { "Next" } else { "Prev" }, context.multiple_tabs)
                }
                I::ToggleAppearance => (if context.appearance { "Back" } else { "Look" }, true),
                I::ToggleWrap => ("Wrp", context.editable),
                I::DocumentInfo => ("Info", true),
                I::ToggleLineNumbers => ("Ln", context.editable),
                I::ToggleDiagnostics => ("Perf", context.editable),
                I::Copy { cut } => (
                    if cut { "Cut" } else { "Copy" },
                    context.editable && context.selected,
                ),
                I::Paste => ("Paste", context.editable),
                I::Edit(E::Undo) => ("Undo", context.editable && context.can_undo),
                I::Edit(E::Redo) => ("Redo", context.editable && context.can_redo),
                I::Edit(E::SelectAll) => ("All", context.editable),
                I::Edit(E::ToggleBlock) => ("Block", context.editable),
                I::Zoom(delta) => (if delta > 0 { "+10%" } else { "-10%" }, true),
                I::ResetFontSize => ("100%", true),
                I::SearchClose | I::Cancel => ("Back", true),
                I::ToggleIgnoreCase => (
                    if context.search.ignore_case {
                        "NoCs"
                    } else {
                        "Case"
                    },
                    true,
                ),
                I::ToggleWholeWord => (
                    if context.search.whole_word {
                        "Word"
                    } else {
                        "Part"
                    },
                    true,
                ),
                I::ContextMenu => ("Menu", context.editable),
                I::AcceptDialog => (context.dialog_accept, true),
                I::CancelDialog => ("Back", true),
                I::NavigateDialog { forward } => (if forward { "Next" } else { "Prev" }, true),
                I::AcceptAppearance => ("Keep", true),
                I::ResetAppearance => ("Rst", true),
                I::BrowseAppearance { forward } => (if forward { "Next" } else { "Prev" }, true),
                _ => continue,
            };
            if available {
                let label = value.map_or(Cow::Borrowed(label), Cow::Owned);
                tips.push((
                    index,
                    format!(
                        "F{}: {}",
                        index + 1,
                        if description.is_empty() {
                            label.as_ref()
                        } else {
                            &description
                        }
                    ),
                ));
                items.push((index, label));
            }
        }
        // Narrow rasters show the operations for the active task first.
        if data_layer {
            items.sort_by_key(|(i, _)| {
                [6, 7, 8, 11, 9, 10]
                    .iter()
                    .position(|key| key == i)
                    .unwrap_or(6)
            });
        } else if context.search.open {
            items.sort_by_key(|(i, _)| if matches!(i, 2..=5 | 9) { 0 } else { 1 });
        }
        Self { items, tips }
    }
}
impl Geometry {
    pub fn new(width: usize, height: usize, compact: bool, commands: bool) -> Self {
        let columns = (width / 64).clamp(1, 12);
        let bottom = if commands {
            if compact {
                16
            } else {
                32
            }
        } else {
            0
        };
        Self {
            width,
            height,
            top: if compact { 18 } else { 32 },
            bottom,
            columns,
            compact,
        }
    }
    pub fn command_slot(self, x: f32, y: f32) -> Option<usize> {
        (self.bottom > 0
            && x >= 0.0
            && x < self.width as f32
            && y >= self.height.saturating_sub(self.bottom) as f32
            && y < self.height as f32)
            .then(|| (x as usize * self.columns / self.width.max(1)).min(self.columns - 1))
    }
    pub fn command_rect(self, slot: usize) -> crate::appearance_ui::Rect {
        let x = slot * self.width / self.columns;
        let next = (slot + 1) * self.width / self.columns;
        crate::appearance_ui::Rect {
            x,
            y: self.height.saturating_sub(self.bottom),
            w: next - x,
            h: self.bottom,
        }
    }
    /// Height above the command row; frames shorter than the row have none.
    pub fn above_commands(self) -> usize {
        self.height.saturating_sub(self.bottom)
    }
    pub fn document_height(self) -> usize {
        self.height.saturating_sub(self.top + self.bottom).max(1)
    }
    pub fn document_point(self, (x, y): (f32, f32)) -> (f32, f32) {
        (x, y - self.top as f32)
    }
    pub fn in_document(self, y: f32) -> bool {
        y >= self.top as f32 && y < self.height.saturating_sub(self.bottom) as f32
    }
    pub fn tab_capacity(self) -> usize {
        (self.width.saturating_sub(84) / if self.compact { 88 } else { 176 }).max(1)
    }
    pub fn tab_width(self) -> usize {
        (self.width.saturating_sub(84) / self.tab_capacity()).max(1)
    }
    pub fn reveal(self, first: &mut usize, active: usize, count: usize) {
        let cap = self.tab_capacity();
        if active < *first {
            *first = active;
        }
        if active >= first.saturating_add(cap) {
            *first = active + 1 - cap;
        }
        *first = (*first).min(count.saturating_sub(cap));
    }
    pub fn hit(self, x: f32, y: f32, first: usize, count: usize) -> Option<Hit> {
        if x < 0.0 || x >= self.width as f32 || y < 0.0 || y >= self.height as f32 {
            return None;
        }
        if y < self.top as f32 {
            let x = x as usize;
            if x >= self.width.saturating_sub(28) {
                return Some(Hit::New);
            }
            if x >= self.width.saturating_sub(56) {
                return Some(Hit::Next);
            }
            if x >= self.width.saturating_sub(84) {
                return Some(Hit::Previous);
            }
            let slot = x / self.tab_width();
            let index = first + slot;
            return Some(if index >= count || slot >= self.tab_capacity() {
                Hit::Consumed
            } else if x % self.tab_width() >= self.tab_width().saturating_sub(24) {
                Hit::Close(index)
            } else {
                Hit::Tab(index)
            });
        }
        if let Some(slot) = self.command_slot(x, y) {
            return Some(Hit::Command(slot));
        }
        None
    }
}
pub(crate) struct Feedback {
    pub hover: Option<usize>,
    pub pressed: Option<usize>,
}
#[expect(
    clippy::too_many_arguments,
    reason = "Chrome paints shared geometry and command feedback"
)]
pub(crate) fn paint(
    list: &mut DrawList,
    font: &mut FontEngine,
    g: Geometry,
    p: &Palette,
    labels: &[String],
    active: usize,
    first: usize,
    commands: &Commands,
    feedback: Feedback,
) {
    draw_rect(list, g.width, g.height, 0, 0, g.width, g.top, p.status_bg);
    for (slot, label) in labels.iter().skip(first).take(g.tab_capacity()).enumerate() {
        let x = slot * g.tab_width();
        let selected = first + slot == active;
        draw_rect(
            list,
            g.width,
            g.height,
            x,
            0,
            g.tab_width().saturating_sub(1),
            g.top,
            if selected { p.bg } else { p.status_bg },
        );
        if selected {
            draw_rect(
                list,
                g.width,
                g.height,
                x,
                g.top - 2,
                g.tab_width() - 1,
                2,
                p.caret,
            );
        }
        let chars = ((g.tab_width().saturating_sub(32)) as f32 / font.char_width_px) as usize;
        let mut text: String = label.chars().take(chars).collect();
        if label.chars().count() > chars && chars > 0 {
            text.pop();
            text.push('…');
        }
        font.draw_string(
            list,
            g.width,
            g.height,
            x + 4,
            if g.compact { 4 } else { 8 },
            &text,
            p.status_text,
        );
        font.draw_string(
            list,
            g.width,
            g.height,
            x + g.tab_width().saturating_sub(19),
            if g.compact { 4 } else { 8 },
            "×",
            p.status_text,
        );
    }
    for (distance, text) in [(84, "‹"), (56, "›"), (28, "+")] {
        font.draw_string(
            list,
            g.width,
            g.height,
            g.width.saturating_sub(distance) + 7,
            if g.compact { 4 } else { 8 },
            text,
            p.status_text,
        );
    }
    if g.bottom == 0 {
        return;
    }
    let controls = 0;
    let y = g.above_commands();
    for (ordinal, (key, label)) in commands.items.iter().take(g.columns).enumerate() {
        let slot = if g.columns == 12 { *key } else { ordinal };
        let x = controls + slot * g.width.saturating_sub(controls) / g.columns;
        let next = controls + (slot + 1) * g.width.saturating_sub(controls) / g.columns;
        draw_rect(
            list,
            g.width,
            g.height,
            x,
            y,
            next - x - 1,
            g.bottom,
            if feedback.pressed == Some(*key) {
                p.input_active
            } else if feedback.hover == Some(*key) {
                p.panel_border
            } else {
                p.status_bg
            },
        );
        let text = format!("F{}{}", key + 1, label);
        let fit = ((next - x).saturating_sub(4) as f32 / font.char_width_px) as usize;
        font.draw_string(
            list,
            g.width,
            g.height,
            x + 2,
            y + if g.compact { 4 } else { 6 },
            &text.chars().take(fit).collect::<String>(),
            if feedback.hover == Some(*key) || feedback.pressed == Some(*key) {
                p.text
            } else {
                p.status_text
            },
        );
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn context() -> Context {
        Context {
            search: Default::default(),
            appearance: false,
            dialog: false,
            dialog_accept: "OK",
            multiple_tabs: true,
            selected: false,
            can_undo: true,
            can_redo: false,
            editable: true,
            has_query: true,
            scalable: true,
            zoom_percent: 120,
            statistics: "42 words".into(),
            statistics_short: "42w".into(),
            cursor: (3, 9),
            wrap: true,
        }
    }
    #[test]
    fn search_offers_case_and_whole_word_switches_that_show_their_state() {
        use texted_app::input::Intent as I;
        use winit::keyboard::ModifiersState as M;
        let mut c = context();
        c.search.open = true;
        assert!(matches!(
            command(4, &c, M::empty()),
            Some(I::ToggleIgnoreCase)
        ));
        assert!(matches!(
            command(5, &c, M::empty()),
            Some(I::ToggleWholeWord)
        ));
        let items = Commands::new(&c, M::empty()).items;
        assert!(items.contains(&(4, "Case".into())), "{items:?}");
        assert!(items.contains(&(5, "Part".into())), "{items:?}");
        c.search.ignore_case = true;
        c.search.whole_word = true;
        let items = Commands::new(&c, M::empty()).items;
        assert!(items.contains(&(4, "NoCs".into())), "{items:?}");
        assert!(items.contains(&(5, "Word".into())), "{items:?}");
        c.search.open = false;
        assert!(matches!(
            command(4, &c, M::empty()),
            Some(I::File(texted_app::input::FileIntent::Open))
        ));
    }
    #[test]
    fn command_labels_follow_search_appearance_and_modifier_layers() {
        use texted_app::input::Intent as I;
        use winit::keyboard::ModifiersState as M;
        let mut c = context();
        let editor = Commands::new(&c, M::empty());
        assert!(editor.items.contains(&(2, "Find".into())));
        let geometry = Geometry::new(980, 700, false, true);
        for (key, _) in &editor.items {
            assert_eq!(editor.key_at(geometry, *key), Some(*key));
        }
        c.search.open = true;
        let search = Commands::new(&c, M::empty());
        assert_eq!(search.items[0], (2, "Next".into()));
        assert!(matches!(
            command(2, &c, M::SHIFT),
            Some(I::NextMatch { forward: false })
        ));
        assert!(Commands::new(&c, M::SHIFT)
            .items
            .contains(&(1, "As".into())));
        assert!(matches!(
            command(9, &c, M::SUPER),
            Some(I::File(texted_app::input::FileIntent::Quit))
        ));
        assert!(matches!(command(6, &c, M::CONTROL), Some(I::Zoom(-1))));
        assert!(Commands::new(&c, M::ALT)
            .items
            .iter()
            .all(|(_, label)| *label != "Copy" && *label != "Cut"));
        c.selected = true;
        assert!(Commands::new(&c, M::ALT)
            .items
            .contains(&(1, "Copy".into())));
        c.appearance = true;
        let appearance = Commands::new(&c, M::empty());
        assert!(appearance.items.contains(&(1, "Keep".into())));
        assert!(!appearance.items.iter().any(|(_, label)| *label == "Save"));
        assert!(matches!(
            command(2, &c, M::empty()),
            Some(I::BrowseAppearance { forward: false })
        ));
    }
    #[test]
    fn ctrl_data_uses_existing_slots_and_keeps_scale_and_counts_visible_when_narrow() {
        use winit::keyboard::ModifiersState as M;
        let mut c = context();
        c.has_query = false;
        let commands = Commands::new(&c, M::CONTROL);
        assert!(commands.items.contains(&(8, "120%".into())));
        assert!(commands.items.contains(&(9, "3:9".into())));
        assert!(commands.items.contains(&(10, "W:on".into())));
        assert!(commands.items.contains(&(11, "42w".into())));
        let narrow = Geometry::new(256, 192, true, true);
        assert_eq!(
            commands
                .items
                .iter()
                .take(narrow.columns)
                .map(|(key, _)| *key)
                .collect::<Vec<_>>(),
            vec![6, 7, 8, 11]
        );
        for (slot, (key, label)) in commands.items.iter().take(narrow.columns).enumerate() {
            let rect = narrow.command_rect(slot);
            assert!((format!("F{}{}", key + 1, label).chars().count() * 8 + 4) <= rect.w);
            let tip = commands
                .tip_at(narrow, (rect.x + 2) as f32, (rect.y + 2) as f32)
                .unwrap();
            assert_eq!(tip.rect, rect);
            assert!(!tip.text.is_empty());
        }
        c.appearance = true;
        assert!(matches!(
            command(6, &c, M::CONTROL),
            Some(texted_app::input::Intent::Zoom(-1))
        ));
    }
    #[test]
    fn hover_and_pressed_feedback_color_only_the_invoked_button() {
        let c = context();
        let commands = Commands::new(&c, winit::keyboard::ModifiersState::empty());
        let g = Geometry::new(980, 700, false, true);
        let palette = texted_app::appearance::Appearance::default()
            .palette()
            .unwrap();
        let mut font = FontEngine::new(16.0);
        let mut list = DrawList::default();
        paint(
            &mut list,
            &mut font,
            g,
            &palette,
            &[],
            0,
            0,
            &commands,
            Feedback {
                hover: Some(2),
                pressed: Some(1),
            },
        );
        for (key, color) in [
            (1, palette.input_active),
            (2, palette.panel_border),
            (3, palette.status_bg),
        ] {
            let rect = g.command_rect(key);
            assert!(list.draws.iter().any(|d| d.glyph.is_none()
                && d.rect[0] == rect.x as f32
                && d.rect[1] == rect.y as f32
                && d.color == color));
        }
    }
    #[test]
    fn scene_translation_and_command_hits_match_at_all_resolutions() {
        for (w, h, compact) in [
            (256, 192, true),
            (320, 200, true),
            (980, 700, false),
            (1600, 900, false),
        ] {
            for shown in [false, true] {
                let g = Geometry::new(w, h, compact, shown);
                assert_eq!(g.document_point((12.0, g.top as f32 + 5.0)), (12.0, 5.0));
                assert_eq!(g.top + g.document_height() + g.bottom, h);
                assert!(!g.in_document(0.0));
                assert!(g.hit(2.0, g.top as f32 + 5.0, 0, 2).is_none());
                if shown {
                    let controls = 0;
                    for slot in 0..g.columns {
                        let x =
                            (controls + slot * w.saturating_sub(controls) / g.columns + 2) as f32;
                        assert_eq!(g.hit(x, (h - 2) as f32, 0, 2), Some(Hit::Command(slot)));
                    }
                    assert_eq!(g.bottom, if compact { 16 } else { 32 });
                }
                let mut first = 0;
                g.reveal(&mut first, 19, 20);
                assert!(first <= 19 && first + g.tab_capacity() > 19);
                assert_eq!(g.hit(2.0, 2.0, first, 20), Some(Hit::Tab(first)));
            }
        }
    }
}
