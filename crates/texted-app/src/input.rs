//! Pure native-key decoding into editor/UI intents. No I/O, RPC or window is needed.
use crate::commands::{DeleteUnit, EditorCommand, Motion};
use winit::keyboard::{Key, KeyCode, ModifiersState, NamedKey, PhysicalKey};
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileIntent {
    New,
    Open,
    Save { as_new: bool },
    Close,
    Quit,
}
#[derive(Debug, Clone)]
pub enum Intent {
    Edit(EditorCommand),
    File(FileIntent),
    Copy {
        cut: bool,
    },
    Paste,
    Find,
    Replace,
    NextMatch {
        forward: bool,
    },
    /// Search switches; they rerun the current query.
    ToggleIgnoreCase,
    ToggleWholeWord,
    ReplaceMatch {
        all: bool,
    },
    SearchText(String),
    SearchBackspace,
    SearchTab,
    SearchClose,
    ContextMenu,
    ToggleAppearance,
    DocumentInfo,
    Zoom(i32),
    ResetFontSize,
    ToggleWrap,
    ToggleLineNumbers,
    ToggleDiagnostics,
    ToggleCommands,
    AcceptAppearance,
    AcceptDialog,
    CancelDialog,
    NavigateDialog {
        forward: bool,
    },
    ResetAppearance,
    BrowseAppearance {
        forward: bool,
    },
    NextTab {
        forward: bool,
    },
    Cancel,
}
#[derive(Clone, Copy, Default)]
pub struct SearchContext {
    pub open: bool,
    pub replace: bool,
    pub replacement_field: bool,
    pub ignore_case: bool,
    pub whole_word: bool,
}
pub struct KeyInput<'a> {
    pub physical: PhysicalKey,
    pub logical: &'a Key,
    pub text: Option<&'a str>,
    pub modifiers: ModifiersState,
}
fn is(input: &KeyInput<'_>, code: KeyCode, letter: char) -> bool {
    input.physical == PhysicalKey::Code(code)
        || matches!(input.logical,Key::Character(s) if s.chars().next().is_some_and(|c|c.eq_ignore_ascii_case(&letter)))
}
fn shortcut(input: &KeyInput<'_>) -> Option<Intent> {
    let shift = input.modifiers.shift_key();
    match input.physical {
        PhysicalKey::Code(KeyCode::Comma) => return Some(Intent::ToggleAppearance),
        PhysicalKey::Code(KeyCode::Equal | KeyCode::NumpadAdd) => return Some(Intent::Zoom(1)),
        PhysicalKey::Code(KeyCode::Minus | KeyCode::NumpadSubtract) => {
            return Some(Intent::Zoom(-1))
        }
        PhysicalKey::Code(KeyCode::Digit0 | KeyCode::Numpad0) => {
            return Some(Intent::ResetFontSize)
        }
        _ => {}
    }
    let bindings = [
        (KeyCode::KeyA, 'a', Intent::Edit(EditorCommand::SelectAll)),
        (KeyCode::KeyC, 'c', Intent::Copy { cut: false }),
        (KeyCode::KeyX, 'x', Intent::Copy { cut: true }),
        (KeyCode::KeyV, 'v', Intent::Paste),
        (
            KeyCode::KeyZ,
            'z',
            Intent::Edit(if shift {
                EditorCommand::Redo
            } else {
                EditorCommand::Undo
            }),
        ),
        (KeyCode::KeyY, 'y', Intent::Edit(EditorCommand::Redo)),
        (KeyCode::KeyF, 'f', Intent::Find),
        (KeyCode::KeyR, 'r', Intent::Replace),
        (KeyCode::KeyG, 'g', Intent::NextMatch { forward: !shift }),
        (KeyCode::KeyB, 'b', Intent::Edit(EditorCommand::ToggleBlock)),
        (KeyCode::KeyL, 'l', Intent::ToggleLineNumbers),
        (KeyCode::KeyN, 'n', Intent::File(FileIntent::New)),
        (KeyCode::KeyO, 'o', Intent::File(FileIntent::Open)),
        (
            KeyCode::KeyS,
            's',
            Intent::File(FileIntent::Save { as_new: shift }),
        ),
        (KeyCode::KeyQ, 'q', Intent::File(FileIntent::Quit)),
        (KeyCode::KeyW, 'w', Intent::File(FileIntent::Close)),
    ];
    bindings
        .into_iter()
        .find_map(|(code, letter, intent)| is(input, code, letter).then_some(intent))
}
/// Shared by the visible command bar and physical function keys.
pub fn function_key(index: usize) -> Option<Intent> {
    Some(match index {
        0 => Intent::ToggleCommands,
        1 => Intent::File(FileIntent::Save { as_new: false }),
        2 => Intent::Find,
        3 => Intent::Replace,
        4 => Intent::File(FileIntent::Open),
        5 => Intent::File(FileIntent::New),
        6 => Intent::NextTab { forward: false },
        7 => Intent::NextTab { forward: true },
        8 => Intent::ToggleAppearance,
        9 => Intent::File(FileIntent::Close),
        10 => Intent::ToggleWrap,
        11 => Intent::DocumentInfo,
        _ => return None,
    })
}
pub fn function_key_index(key: &Key) -> Option<usize> {
    Some(match key {
        Key::Named(NamedKey::F1) => 0,
        Key::Named(NamedKey::F2) => 1,
        Key::Named(NamedKey::F3) => 2,
        Key::Named(NamedKey::F4) => 3,
        Key::Named(NamedKey::F5) => 4,
        Key::Named(NamedKey::F6) => 5,
        Key::Named(NamedKey::F7) => 6,
        Key::Named(NamedKey::F8) => 7,
        Key::Named(NamedKey::F9) => 8,
        Key::Named(NamedKey::F10) => 9,
        Key::Named(NamedKey::F11) => 10,
        Key::Named(NamedKey::F12) => 11,
        _ => return None,
    })
}
/// Modifier layers are shared by keyboard decoding and the clickable panel.
pub fn function_key_for(
    index: usize,
    modifiers: ModifiersState,
    search: SearchContext,
) -> Option<Intent> {
    let shift = modifiers.shift_key();
    if index == 9 && shift {
        return Some(Intent::ContextMenu);
    }
    if modifiers.super_key() {
        match index {
            6 => return Some(Intent::Edit(EditorCommand::Undo)),
            7 => return Some(Intent::Edit(EditorCommand::Redo)),
            9 => return Some(Intent::File(FileIntent::Quit)),
            10 => return Some(Intent::ToggleLineNumbers),
            11 => return Some(Intent::ToggleDiagnostics),
            _ => {}
        }
    } else if modifiers.alt_key() {
        return Some(match index {
            1 => Intent::Copy { cut: false },
            2 => Intent::Paste,
            3 => Intent::Copy { cut: true },
            4 => Intent::Edit(EditorCommand::SelectAll),
            5 => Intent::Edit(EditorCommand::ToggleBlock),
            6 => Intent::Zoom(-1),
            7 => Intent::Zoom(1),
            8 => Intent::ResetFontSize,
            9 => Intent::Cancel,
            10 => Intent::ToggleWrap,
            11 => Intent::ToggleLineNumbers,
            _ => return function_key(index),
        });
    } else if modifiers.control_key() {
        match index {
            6 => return Some(Intent::Zoom(-1)),
            7 => return Some(Intent::Zoom(1)),
            8 => return Some(Intent::ResetFontSize),
            9 | 11 => return Some(Intent::DocumentInfo),
            10 => return Some(Intent::ToggleWrap),
            _ => {}
        }
    }
    Some(match index {
        1 => Intent::File(FileIntent::Save { as_new: shift }),
        2 if search.open || shift => Intent::NextMatch { forward: !shift },
        3 if search.replace => Intent::ReplaceMatch { all: shift },
        4 if search.open => Intent::ToggleIgnoreCase,
        5 if search.open => Intent::ToggleWholeWord,
        9 if search.open => Intent::SearchClose,
        _ => return function_key(index),
    })
}
fn motion(key: &Key, meta: bool, alt: bool, block: bool) -> Option<Motion> {
    use NamedKey::*;
    Some(match key {
        Key::Named(ArrowLeft) => {
            if meta {
                Motion::LineStart
            } else if alt {
                Motion::WordLeft
            } else {
                Motion::Left
            }
        }
        Key::Named(ArrowRight) => {
            if meta {
                Motion::LineEnd
            } else if alt {
                Motion::WordRight
            } else {
                Motion::Right
            }
        }
        Key::Named(ArrowUp) => {
            if block {
                Motion::Up
            } else if meta {
                Motion::DocumentStart
            } else if alt {
                Motion::Rows(-5)
            } else {
                Motion::Up
            }
        }
        Key::Named(ArrowDown) => {
            if block {
                Motion::Down
            } else if meta {
                Motion::DocumentEnd
            } else if alt {
                Motion::Rows(5)
            } else {
                Motion::Down
            }
        }
        Key::Named(Home) => Motion::LineStart,
        Key::Named(End) => Motion::LineEnd,
        Key::Named(PageUp) => Motion::Rows(-15),
        Key::Named(PageDown) => Motion::Rows(15),
        _ => return None,
    })
}
pub fn decode(input: &KeyInput<'_>, search: SearchContext) -> Option<Intent> {
    let meta = input.modifiers.control_key() || input.modifiers.super_key();
    let alt = input.modifiers.alt_key();
    let shift = input.modifiers.shift_key();
    if *input.logical == Key::Named(NamedKey::ContextMenu)
        || (shift && *input.logical == Key::Named(NamedKey::F10))
    {
        return Some(Intent::ContextMenu);
    }
    if meta && shift && is(input, KeyCode::KeyI, 'i') {
        return Some(Intent::DocumentInfo);
    }
    if meta {
        if let Some(intent) = shortcut(input) {
            return Some(intent);
        }
    }
    if alt && is(input, KeyCode::KeyZ, 'z') {
        return Some(Intent::ToggleWrap);
    }
    if input.modifiers.control_key() && *input.logical == Key::Named(NamedKey::Tab) {
        return Some(Intent::NextTab { forward: !shift });
    }
    if meta
        && shift
        && matches!(
            input.physical,
            PhysicalKey::Code(KeyCode::BracketLeft | KeyCode::BracketRight)
        )
    {
        return Some(Intent::NextTab {
            forward: input.physical == PhysicalKey::Code(KeyCode::BracketRight),
        });
    }
    if let Some(index) = function_key_index(input.logical) {
        return function_key_for(index, input.modifiers, search);
    }
    if search.open {
        match input.logical {
            Key::Named(NamedKey::Escape) => return Some(Intent::SearchClose),
            Key::Named(NamedKey::Tab) => return Some(Intent::SearchTab),
            Key::Named(NamedKey::Backspace) => return Some(Intent::SearchBackspace),
            Key::Named(NamedKey::Enter) => {
                return Some(if search.replace && search.replacement_field {
                    Intent::ReplaceMatch { all: false }
                } else {
                    Intent::NextMatch { forward: !shift }
                })
            }
            _ => {}
        }
    }
    let block = shift
        && alt
        && matches!(
            input.logical,
            Key::Named(NamedKey::ArrowUp | NamedKey::ArrowDown)
        );
    if let Some(motion) = motion(input.logical, meta, alt, block) {
        return Some(Intent::Edit(EditorCommand::Move {
            motion,
            extend: shift,
            block,
        }));
    }
    match input.logical {
        Key::Named(NamedKey::Escape) => Some(Intent::Cancel),
        Key::Named(NamedKey::Backspace | NamedKey::Delete) => {
            Some(Intent::Edit(EditorCommand::Delete {
                backward: *input.logical == Key::Named(NamedKey::Backspace),
                unit: if meta {
                    DeleteUnit::Line
                } else if alt {
                    DeleteUnit::Word
                } else {
                    DeleteUnit::Character
                },
            }))
        }
        Key::Named(NamedKey::Enter) => Some(Intent::Edit(EditorCommand::Insert("\n".into()))),
        Key::Named(NamedKey::Tab) => Some(Intent::Edit(EditorCommand::Insert("    ".into()))),
        _ => input
            .text
            .map(|text| text.chars().filter(|c| !c.is_control()).collect::<String>())
            .filter(|s| !s.is_empty())
            .map(|text| {
                if search.open {
                    Intent::SearchText(text)
                } else {
                    Intent::Edit(EditorCommand::Insert(text))
                }
            }),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn function_panel_keys_and_tab_navigation_share_their_intents() {
        for (i, key) in [
            NamedKey::F1,
            NamedKey::F2,
            NamedKey::F3,
            NamedKey::F4,
            NamedKey::F5,
            NamedKey::F6,
            NamedKey::F7,
            NamedKey::F8,
            NamedKey::F9,
            NamedKey::F10,
            NamedKey::F11,
            NamedKey::F12,
        ]
        .into_iter()
        .enumerate()
        {
            let logical = Key::Named(key);
            let input = KeyInput {
                physical: PhysicalKey::Unidentified(winit::keyboard::NativeKeyCode::Unidentified),
                logical: &logical,
                text: None,
                modifiers: ModifiersState::empty(),
            };
            assert_eq!(
                format!("{:?}", decode(&input, SearchContext::default())),
                format!("{:?}", function_key(i))
            );
        }
        let logical = Key::Named(NamedKey::Tab);
        let input = KeyInput {
            physical: PhysicalKey::Code(KeyCode::Tab),
            logical: &logical,
            text: None,
            modifiers: ModifiersState::CONTROL | ModifiersState::SHIFT,
        };
        assert!(matches!(
            decode(&input, SearchContext::default()),
            Some(Intent::NextTab { forward: false })
        ));
        let logical = Key::Character("й".into());
        let input = KeyInput {
            physical: PhysicalKey::Code(KeyCode::KeyQ),
            logical: &logical,
            text: None,
            modifiers: ModifiersState::SUPER,
        };
        assert!(matches!(
            decode(&input, SearchContext::default()),
            Some(Intent::File(FileIntent::Quit))
        ));
    }
    #[test]
    fn document_details_shortcut_is_independent_of_keyboard_language() {
        let logical = Key::Character("Ш".into());
        let input = KeyInput {
            physical: PhysicalKey::Code(KeyCode::KeyI),
            logical: &logical,
            text: None,
            modifiers: ModifiersState::SUPER | ModifiersState::SHIFT,
        };
        assert!(matches!(
            decode(&input, SearchContext::default()),
            Some(Intent::DocumentInfo)
        ));
    }
    #[test]
    fn appearance_shortcuts_use_physical_keys_even_with_search_open() {
        let key = Key::Character("б".into());
        let mut input = KeyInput {
            physical: PhysicalKey::Code(KeyCode::Comma),
            logical: &key,
            text: Some("б"),
            modifiers: ModifiersState::SUPER,
        };
        assert!(matches!(
            decode(
                &input,
                SearchContext {
                    open: true,
                    ..Default::default()
                }
            ),
            Some(Intent::ToggleAppearance)
        ));
        input.physical = PhysicalKey::Code(KeyCode::Equal);
        assert!(matches!(
            decode(&input, SearchContext::default()),
            Some(Intent::Zoom(1))
        ));
        input.physical = PhysicalKey::Code(KeyCode::Digit0);
        assert!(matches!(
            decode(&input, SearchContext::default()),
            Some(Intent::ResetFontSize)
        ));
    }
    #[test]
    fn physical_shortcuts_work_with_ukrainian_logical_keys() {
        let key = Key::Character("а".into());
        let input = KeyInput {
            physical: PhysicalKey::Code(KeyCode::KeyF),
            logical: &key,
            text: None,
            modifiers: ModifiersState::SUPER,
        };
        assert!(matches!(
            decode(&input, SearchContext::default()),
            Some(Intent::Find)
        ));
    }
    #[test]
    fn navigation_modifiers_and_block_intent_are_preserved() {
        let key = Key::Named(NamedKey::ArrowDown);
        let mut input = KeyInput {
            physical: PhysicalKey::Code(KeyCode::ArrowDown),
            logical: &key,
            text: None,
            modifiers: ModifiersState::SUPER,
        };
        assert!(matches!(
            decode(&input, SearchContext::default()),
            Some(Intent::Edit(EditorCommand::Move {
                motion: Motion::DocumentEnd,
                ..
            }))
        ));
        input.modifiers = ModifiersState::SHIFT | ModifiersState::ALT;
        assert!(matches!(
            decode(&input, SearchContext::default()),
            Some(Intent::Edit(EditorCommand::Move {
                motion: Motion::Down,
                extend: true,
                block: true
            }))
        ));
    }
    #[test]
    fn query_text_produces_one_search_effect_without_advancing_navigation() {
        let key = Key::Character("ї".into());
        let input = KeyInput {
            physical: PhysicalKey::Code(KeyCode::KeyS),
            logical: &key,
            text: Some("ї"),
            modifiers: ModifiersState::empty(),
        };
        assert!(
            matches!(decode(&input,SearchContext{open:true,..Default::default()}),Some(Intent::SearchText(s)) if s=="ї")
        );
    }
}
