//! Native menu adapter. All document actions return through the same typed intent path as keys.
use texted_app::{
    commands::{DeleteUnit, EditorCommand},
    input::{FileIntent, Intent},
};
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuCommand {
    New,
    Open,
    Save,
    SaveAs,
    Close,
    Quit,
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    Delete,
    SelectAll,
    Block,
    DuplicateLines,
    MoveLineUp,
    MoveLineDown,
    TrimTrailingSpaces,
    GoToLine,
    Find,
    Replace,
    Next,
    Previous,
    Style,
    DocumentInfo,
    ZoomIn,
    ZoomOut,
    ResetSize,
    Wrap,
    Lines,
    Diagnostics,
    Commands,
    PreviousTab,
    NextTab,
}
impl MenuCommand {
    pub fn intent(self) -> Intent {
        use MenuCommand::*;
        match self {
            New => Intent::File(FileIntent::New),
            Open => Intent::File(FileIntent::Open),
            Save | SaveAs => Intent::File(FileIntent::Save {
                as_new: self == SaveAs,
            }),
            Close => Intent::File(FileIntent::Close),
            Quit => Intent::File(FileIntent::Quit),
            Undo => Intent::Edit(EditorCommand::Undo),
            Redo => Intent::Edit(EditorCommand::Redo),
            Cut | Copy => Intent::Copy { cut: self == Cut },
            Paste => Intent::Paste,
            Delete => Intent::Edit(EditorCommand::Delete {
                backward: false,
                unit: DeleteUnit::Character,
            }),
            SelectAll => Intent::Edit(EditorCommand::SelectAll),
            Block => Intent::Edit(EditorCommand::ToggleBlock),
            DuplicateLines => Intent::Edit(EditorCommand::DuplicateLines),
            MoveLineUp | MoveLineDown => Intent::Edit(EditorCommand::MoveLines {
                up: self == MoveLineUp,
            }),
            TrimTrailingSpaces => Intent::Edit(EditorCommand::TrimTrailingWhitespace),
            GoToLine => Intent::GoToLine,
            Find => Intent::Find,
            Replace => Intent::Replace,
            Next | Previous => Intent::NextMatch {
                forward: self == Next,
            },
            Style => Intent::ToggleAppearance,
            DocumentInfo => Intent::DocumentInfo,
            ZoomIn | ZoomOut => Intent::Zoom(if self == ZoomIn { 1 } else { -1 }),
            ResetSize => Intent::ResetFontSize,
            Wrap => Intent::ToggleWrap,
            Lines => Intent::ToggleLineNumbers,
            Diagnostics => Intent::ToggleDiagnostics,
            Commands => Intent::ToggleCommands,
            PreviousTab | NextTab => Intent::NextTab {
                forward: self == NextTab,
            },
        }
    }
}
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct MenuState {
    pub editable: bool,
    pub style_available: bool,
    pub clipboard_available: bool,
    pub selected: bool,
    pub can_undo: bool,
    pub can_redo: bool,
    pub has_query: bool,
    pub wrap: bool,
    pub lines: bool,
    pub diagnostics: bool,
    pub block: bool,
    pub commands: bool,
}
impl MenuState {
    pub fn enabled(self, command: MenuCommand) -> bool {
        use MenuCommand::*;
        match command {
            New | Open | Save | SaveAs | Close | Quit | DocumentInfo => true, // FileFlow owns lifecycle barriers.
            Style | ZoomIn | ZoomOut | ResetSize => self.style_available,
            Undo => self.editable && self.can_undo,
            Redo => self.editable && self.can_redo,
            Cut | Copy => self.editable && self.clipboard_available && self.selected,
            Delete => self.editable && self.selected,
            Paste => self.editable && self.clipboard_available,
            Next | Previous => self.editable && self.has_query,
            _ => self.editable,
        }
    }
    fn checked(self, command: MenuCommand) -> Option<bool> {
        match command {
            MenuCommand::Wrap => Some(self.wrap),
            MenuCommand::Lines => Some(self.lines),
            MenuCommand::Diagnostics => Some(self.diagnostics),
            MenuCommand::Block => Some(self.block),
            MenuCommand::Commands => Some(self.commands),
            _ => None,
        }
    }
}
#[cfg(target_os = "macos")]
mod mac {
    use super::*;
    use crate::AppEvent;
    use muda::{
        accelerator::{Accelerator, Code, Modifiers},
        AboutMetadata, CheckMenuItem, IsMenuItem, Menu, MenuEvent, MenuItem, MenuItemKind,
        PredefinedMenuItem, Submenu,
    };
    use winit::event_loop::EventLoopProxy;
    pub(crate) struct NativeMenus {
        _bar: Menu,
        items: Vec<(MenuCommand, MenuItemKind)>,
        state: Option<MenuState>,
    }
    fn native(item: &MenuItemKind) -> &dyn IsMenuItem {
        match item {
            MenuItemKind::MenuItem(item) => item,
            MenuItemKind::Check(item) => item,
            _ => unreachable!("only editor actions are stored"),
        }
    }
    impl NativeMenus {
        pub fn new(proxy: EventLoopProxy<AppEvent>) -> Result<Self, String> {
            let bar = Menu::new();
            let mut items = Vec::new();
            let app = Submenu::new("Texted", true);
            let file = Submenu::new("File", true);
            let edit = Submenu::new("Edit", true);
            let search = Submenu::new("Search", true);
            let view = Submenu::new("View", true);
            let window = Submenu::new("Window", true);
            let sep = PredefinedMenuItem::separator;
            app.append(&PredefinedMenuItem::about(
                Some("About Texted"),
                Some(AboutMetadata {
                    name: Some("Texted".into()),
                    version: Some(env!("CARGO_PKG_VERSION").into()),
                    ..Default::default()
                }),
            ))
            .map_err(|e| e.to_string())?;
            // IDs and shortcuts are registered once; shared items synchronize both menu surfaces.
            let mut add = |menu: &Submenu,
                           command,
                           label,
                           shortcut: Option<(Modifiers, Code)>|
             -> Result<MenuItemKind, String> {
                let accelerator = shortcut.map(|(mods, code)| Accelerator::new(Some(mods), code));
                let item = if MenuState::default().checked(command).is_some() {
                    CheckMenuItem::new(label, false, false, accelerator).kind()
                } else {
                    MenuItem::new(label, false, accelerator).kind()
                };
                menu.append(native(&item)).map_err(|e| e.to_string())?;
                items.push((command, item.clone()));
                Ok(item)
            };
            use MenuCommand::*;
            let cmd = Modifiers::SUPER;
            let shift = cmd | Modifiers::SHIFT;
            let style = add(&app, Style, "Style…", Some((cmd, Code::Comma)))?;
            app.append_items(&[
                &sep(),
                &PredefinedMenuItem::services(None),
                &sep(),
                &PredefinedMenuItem::hide(None),
                &PredefinedMenuItem::hide_others(None),
                &PredefinedMenuItem::show_all(None),
                &sep(),
            ])
            .map_err(|e| e.to_string())?;
            add(&app, Quit, "Quit Texted", Some((cmd, Code::KeyQ)))?;
            add(&file, New, "New", Some((cmd, Code::KeyN)))?;
            add(&file, Open, "Open…", Some((cmd, Code::KeyO)))?;
            file.append(&sep()).map_err(|e| e.to_string())?;
            add(&file, Save, "Save", Some((cmd, Code::KeyS)))?;
            add(&file, SaveAs, "Save As…", Some((shift, Code::KeyS)))?;
            file.append(&sep()).map_err(|e| e.to_string())?;
            add(&file, Close, "Close", Some((cmd, Code::KeyW)))?;
            add(&edit, Undo, "Undo", Some((cmd, Code::KeyZ)))?;
            add(&edit, Redo, "Redo", Some((shift, Code::KeyZ)))?;
            edit.append(&sep()).map_err(|e| e.to_string())?;
            for (command, label, code) in [
                (Cut, "Cut", Code::KeyX),
                (Copy, "Copy", Code::KeyC),
                (Paste, "Paste", Code::KeyV),
            ] {
                add(&edit, command, label, Some((cmd, code)))?;
            }
            add(&edit, Delete, "Delete Selection", None)?;
            edit.append(&sep()).map_err(|e| e.to_string())?;
            add(&edit, SelectAll, "Select All", Some((cmd, Code::KeyA)))?;
            add(&edit, Block, "Column Selection", Some((cmd, Code::KeyB)))?;
            edit.append(&sep()).map_err(|e| e.to_string())?;
            add(
                &edit,
                DuplicateLines,
                "Duplicate Line",
                Some((shift, Code::KeyD)),
            )?;
            let option = cmd | Modifiers::ALT;
            add(
                &edit,
                MoveLineUp,
                "Move Line Up",
                Some((option, Code::ArrowUp)),
            )?;
            add(
                &edit,
                MoveLineDown,
                "Move Line Down",
                Some((option, Code::ArrowDown)),
            )?;
            add(&edit, TrimTrailingSpaces, "Trim Trailing Spaces", None)?;
            add(&search, Find, "Find…", Some((cmd, Code::KeyF)))?;
            add(&search, Replace, "Replace…", Some((cmd, Code::KeyR)))?;
            add(&search, GoToLine, "Go to Line…", Some((cmd, Code::KeyJ)))?;
            search.append(&sep()).map_err(|e| e.to_string())?;
            add(&search, Next, "Find Next", Some((cmd, Code::KeyG)))?;
            add(
                &search,
                Previous,
                "Find Previous",
                Some((shift, Code::KeyG)),
            )?;
            // Style also appears in View and at the end of the editor context menu.
            view.append(native(&style)).map_err(|e| e.to_string())?;
            add(
                &view,
                DocumentInfo,
                "Document Details…",
                Some((shift, Code::KeyI)),
            )?;
            add(
                &view,
                ZoomIn,
                "Increase Font Size",
                Some((cmd, Code::Equal)),
            )?;
            add(
                &view,
                ZoomOut,
                "Decrease Font Size",
                Some((cmd, Code::Minus)),
            )?;
            add(
                &view,
                ResetSize,
                "Reset Font Size",
                Some((cmd, Code::Digit0)),
            )?;
            view.append(&sep()).map_err(|e| e.to_string())?;
            add(&view, Wrap, "Word Wrap", Some((Modifiers::ALT, Code::KeyZ)))?;
            add(&view, Lines, "Line Numbers", Some((cmd, Code::KeyL)))?;
            add(&view, Diagnostics, "Diagnostics", None)?;
            add(
                &view,
                Commands,
                "Function Key Commands",
                Some((Modifiers::empty(), Code::F1)),
            )?;
            add(
                &window,
                PreviousTab,
                "Previous Tab",
                Some((shift, Code::BracketLeft)),
            )?;
            add(
                &window,
                NextTab,
                "Next Tab",
                Some((shift, Code::BracketRight)),
            )?;
            window
                .append_items(&[
                    &PredefinedMenuItem::minimize(None),
                    &PredefinedMenuItem::maximize(Some("Zoom")),
                    &PredefinedMenuItem::fullscreen(None),
                    &sep(),
                    &PredefinedMenuItem::bring_all_to_front(None),
                ])
                .map_err(|e| e.to_string())?;
            bar.append_items(&[&app, &file, &edit, &search, &view, &window])
                .map_err(|e| e.to_string())?;
            let commands: Vec<_> = items
                .iter()
                .map(|(command, item)| (item.id().clone(), *command))
                .collect();
            let proxy = std::sync::Mutex::new(proxy);
            MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
                if let Some((_, command)) = commands.iter().find(|(id, _)| *id == event.id) {
                    let _ = proxy
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .send_event(AppEvent::Menu(*command));
                }
            }));
            bar.init_for_nsapp();
            window.set_as_windows_menu_for_nsapp();
            Ok(Self {
                _bar: bar,
                items,
                state: None,
            })
        }
        pub fn update(&mut self, state: MenuState) {
            if self.state == Some(state) {
                return;
            }
            for (command, item) in &self.items {
                let enabled = state.enabled(*command);
                match item {
                    MenuItemKind::MenuItem(item) => item.set_enabled(enabled),
                    MenuItemKind::Check(item) => {
                        item.set_enabled(enabled);
                        item.set_checked(state.checked(*command).unwrap_or(false));
                    }
                    _ => unreachable!("only editor actions are stored"),
                }
            }
            self.state = Some(state);
        }
        pub fn invalidate(&mut self) {
            // Native check items toggle before the queued intent is validated.
            self.state = None;
        }
    }
}
#[cfg(target_os = "macos")]
pub(crate) use mac::NativeMenus;
#[cfg(not(target_os = "macos"))]
pub(crate) struct NativeMenus;
#[cfg(not(target_os = "macos"))]
impl NativeMenus {
    pub fn new(_: winit::event_loop::EventLoopProxy<crate::AppEvent>) -> Result<Self, String> {
        Ok(Self)
    }
    pub fn update(&mut self, _: MenuState) {}
    pub fn invalidate(&mut self) {}
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unavailable_actions_cannot_mutate_documents_but_lifecycle_keeps_its_barriers() {
        let locked = MenuState::default();
        for command in [
            MenuCommand::Undo,
            MenuCommand::Cut,
            MenuCommand::Paste,
            MenuCommand::Style,
        ] {
            assert!(!locked.enabled(command));
        }
        assert!(locked.enabled(MenuCommand::Save));
        let mut state = MenuState {
            editable: true,
            clipboard_available: true,
            ..locked
        };
        assert!(state.enabled(MenuCommand::Paste));
        assert!(!state.enabled(MenuCommand::Copy));
        assert!(!state.enabled(MenuCommand::Undo));
        state.selected = true;
        state.can_undo = true;
        assert!(state.enabled(MenuCommand::Copy));
        assert!(state.enabled(MenuCommand::Undo));
        state.clipboard_available = false;
        assert!(!state.enabled(MenuCommand::Cut));
        assert!(!state.enabled(MenuCommand::Paste));
    }
}
