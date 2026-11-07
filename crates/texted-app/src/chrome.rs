//! Window chrome uses scene coordinates; document input and drawing use a translated viewport.
use crate::graphics::{draw_rect, DrawList, FontEngine};
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
    pub multiple_tabs: bool,
    pub selected: bool,
    pub can_undo: bool,
    pub can_redo: bool,
    pub editable: bool,
    pub has_query: bool,
}
pub(crate) fn command(
    index: usize,
    context: &Context,
    modifiers: winit::keyboard::ModifiersState,
) -> Option<texted_app::input::Intent> {
    use texted_app::input::Intent as I;
    if context.appearance {
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
    texted_app::input::function_key_for(index, modifiers, context.search)
}
pub(crate) struct Commands {
    pub items: Vec<(usize, &'static str)>,
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

    pub fn new(context: &Context, modifiers: winit::keyboard::ModifiersState) -> Self {
        use texted_app::{
            commands::EditorCommand as E,
            input::{FileIntent as F, Intent as I},
        };
        let mut items = Vec::new();
        for index in 0..12 {
            let Some(intent) = command(index, context, modifiers) else {
                continue;
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
                I::ToggleWrap => ("Wrap", context.editable),
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
                I::ContextMenu => ("Menu", context.editable),
                I::AcceptAppearance => ("Keep", true),
                I::ResetAppearance => ("Reset", true),
                I::BrowseAppearance { forward } => (if forward { "Next" } else { "Prev" }, true),
                _ => continue,
            };
            if available {
                items.push((index, label));
            }
        }
        // Narrow rasters show the operations for the active task first.
        if context.search.open {
            items.sort_by_key(|(i, _)| if matches!(i, 2 | 3 | 9) { 0 } else { 1 });
        }
        Self { items }
    }
}
impl Geometry {
    pub fn new(width: usize, height: usize, compact: bool, commands: bool) -> Self {
        let columns = (width / if compact { 48 } else { 64 }).clamp(1, 12);
        let bottom = if commands {
            if compact {
                16
            } else {
                28
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
        if self.bottom > 0 && y >= self.height.saturating_sub(self.bottom) as f32 {
            let column = (x as usize * self.columns / self.width.max(1)).min(self.columns - 1);
            return Some(Hit::Command(column));
        }
        None
    }
}
#[expect(
    clippy::too_many_arguments,
    reason = "Chrome paints shared window geometry and tab/command state"
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
    let y = g.height - g.bottom;
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
            p.status_bg,
        );
        let text = format!(
            "F{}{}",
            key + 1,
            if g.compact {
                label.chars().take(2).collect::<String>()
            } else {
                (*label).into()
            }
        );
        let fit = ((next - x).saturating_sub(4) as f32 / font.char_width_px) as usize;
        font.draw_string(
            list,
            g.width,
            g.height,
            x + 2,
            y + if g.compact { 4 } else { 6 },
            &text.chars().take(fit).collect::<String>(),
            p.status_text,
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
            multiple_tabs: true,
            selected: false,
            can_undo: true,
            can_redo: false,
            editable: true,
            has_query: true,
        }
    }
    #[test]
    fn command_labels_follow_search_appearance_and_modifier_layers() {
        use texted_app::input::Intent as I;
        use winit::keyboard::ModifiersState as M;
        let mut c = context();
        let editor = Commands::new(&c, M::empty());
        assert!(editor.items.contains(&(2, "Find")));
        let geometry = Geometry::new(980, 700, false, true);
        for (key, _) in &editor.items {
            assert_eq!(editor.key_at(geometry, *key), Some(*key));
        }
        c.search.open = true;
        let search = Commands::new(&c, M::empty());
        assert_eq!(search.items[0], (2, "Next"));
        assert!(matches!(
            command(2, &c, M::SHIFT),
            Some(I::NextMatch { forward: false })
        ));
        assert!(Commands::new(&c, M::SHIFT).items.contains(&(1, "As")));
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
        assert!(Commands::new(&c, M::ALT).items.contains(&(1, "Copy")));
        c.appearance = true;
        let appearance = Commands::new(&c, M::empty());
        assert!(appearance.items.contains(&(1, "Keep")));
        assert!(!appearance.items.iter().any(|(_, label)| *label == "Save"));
        assert!(matches!(
            command(2, &c, M::empty()),
            Some(I::BrowseAppearance { forward: false })
        ));
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
                    assert_eq!(g.bottom, if compact { 16 } else { 28 });
                }
                let mut first = 0;
                g.reveal(&mut first, 19, 20);
                assert!(first <= 19 && first + g.tab_capacity() > 19);
                assert_eq!(g.hit(2.0, 2.0, first, 20), Some(Hit::Tab(first)));
            }
        }
    }
}
