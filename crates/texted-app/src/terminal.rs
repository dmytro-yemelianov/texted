//! Terminal transport for the same App, scene, commands and dialogs as the window.
use crate::{
    app::{App, Shell},
    input::KeyInput,
    surface::{Host, Renderer},
};
use crossterm::{
    cursor,
    event::{
        self, DisableBracketedPaste, DisableFocusChange, DisableMouseCapture, EnableBracketedPaste,
        EnableFocusChange, EnableMouseCapture, Event, KeyCode as CKey, KeyEvent, KeyEventKind,
        KeyModifiers, KeyboardEnhancementFlags, MouseButton, MouseEventKind,
        PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    execute,
    terminal::{self, DisableLineWrap, EnableLineWrap, EnterAlternateScreen, LeaveAlternateScreen},
};
use std::{
    io::{self, IsTerminal},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use winit::{
    event::ElementState,
    keyboard::{Key, KeyCode, ModifiersState, NamedKey, NativeKeyCode, PhysicalKey},
};

static ACTIVE: AtomicBool = AtomicBool::new(false);
struct TerminalGuard;
impl TerminalGuard {
    fn enter() -> io::Result<Self> {
        terminal::enable_raw_mode()?;
        ACTIVE.store(true, Ordering::Release);
        let guard = Self;
        execute!(
            io::stdout(),
            EnterAlternateScreen,
            DisableLineWrap,
            EnableMouseCapture,
            EnableBracketedPaste,
            EnableFocusChange,
            PushKeyboardEnhancementFlags(
                KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                    | KeyboardEnhancementFlags::REPORT_EVENT_TYPES
                    | KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES
            )
        )?;
        Ok(guard)
    }
}
fn restore() {
    if !ACTIVE.swap(false, Ordering::AcqRel) {
        return;
    }
    let _ = execute!(
        io::stdout(),
        crossterm::style::ResetColor,
        PopKeyboardEnhancementFlags,
        DisableFocusChange,
        DisableBracketedPaste,
        DisableMouseCapture,
        EnableLineWrap,
        cursor::Show,
        LeaveAlternateScreen
    );
    let _ = terminal::disable_raw_mode();
}
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore();
    }
}

fn modifiers(m: KeyModifiers) -> ModifiersState {
    let mut result = ModifiersState::empty();
    for (source, target) in [
        (KeyModifiers::SHIFT, ModifiersState::SHIFT),
        (KeyModifiers::CONTROL, ModifiersState::CONTROL),
        (KeyModifiers::ALT, ModifiersState::ALT),
        (KeyModifiers::SUPER, ModifiersState::SUPER),
    ] {
        if m.contains(source) {
            result |= target;
        }
    }
    result
}
fn key(event: KeyEvent) -> (Key, PhysicalKey, Option<String>) {
    use NamedKey as N;
    let named = match event.code {
        CKey::Backspace => Some(N::Backspace),
        CKey::Enter => Some(N::Enter),
        CKey::Left => Some(N::ArrowLeft),
        CKey::Right => Some(N::ArrowRight),
        CKey::Up => Some(N::ArrowUp),
        CKey::Down => Some(N::ArrowDown),
        CKey::Home => Some(N::Home),
        CKey::End => Some(N::End),
        CKey::PageUp => Some(N::PageUp),
        CKey::PageDown => Some(N::PageDown),
        CKey::Tab | CKey::BackTab => Some(N::Tab),
        CKey::Delete => Some(N::Delete),
        CKey::Insert => Some(N::Insert),
        CKey::Esc => Some(N::Escape),
        CKey::F(n) => [
            N::F1,
            N::F2,
            N::F3,
            N::F4,
            N::F5,
            N::F6,
            N::F7,
            N::F8,
            N::F9,
            N::F10,
            N::F11,
            N::F12,
        ]
        .get(usize::from(n.saturating_sub(1)))
        .copied(),
        _ => None,
    };
    if let Some(named) = named {
        return (
            Key::Named(named),
            PhysicalKey::Unidentified(NativeKeyCode::Unidentified),
            None,
        );
    }
    if let CKey::Char(c) = event.code {
        // Legacy terminals send characters rather than scan codes. Recover the
        // standard Ukrainian/Latin shortcut positions while preserving inserted text.
        let lower = c.to_lowercase().next().unwrap_or(c);
        let latin = "qwertyuiop[]asdfghjkl;'zxcvbnm,./`";
        let ukrainian = "йцукенгшщзхїфівапролджєячсмитьбю.ґ";
        let position = ukrainian
            .chars()
            .position(|v| v == lower)
            .and_then(|i| latin.chars().nth(i))
            .unwrap_or(lower);
        let code = match position {
            'a' => Some(KeyCode::KeyA),
            'b' => Some(KeyCode::KeyB),
            'c' => Some(KeyCode::KeyC),
            'd' => Some(KeyCode::KeyD),
            'e' => Some(KeyCode::KeyE),
            'f' => Some(KeyCode::KeyF),
            'g' => Some(KeyCode::KeyG),
            'h' => Some(KeyCode::KeyH),
            'i' => Some(KeyCode::KeyI),
            'j' => Some(KeyCode::KeyJ),
            'k' => Some(KeyCode::KeyK),
            'l' => Some(KeyCode::KeyL),
            'm' => Some(KeyCode::KeyM),
            'n' => Some(KeyCode::KeyN),
            'o' => Some(KeyCode::KeyO),
            'p' => Some(KeyCode::KeyP),
            'q' => Some(KeyCode::KeyQ),
            'r' => Some(KeyCode::KeyR),
            's' => Some(KeyCode::KeyS),
            't' => Some(KeyCode::KeyT),
            'u' => Some(KeyCode::KeyU),
            'v' => Some(KeyCode::KeyV),
            'w' => Some(KeyCode::KeyW),
            'x' => Some(KeyCode::KeyX),
            'y' => Some(KeyCode::KeyY),
            'z' => Some(KeyCode::KeyZ),
            ',' => Some(KeyCode::Comma),
            '[' => Some(KeyCode::BracketLeft),
            ']' => Some(KeyCode::BracketRight),
            '=' | '+' => Some(KeyCode::Equal),
            '-' => Some(KeyCode::Minus),
            '0' => Some(KeyCode::Digit0),
            _ => None,
        };
        return (
            Key::Character(c.to_string().into()),
            code.map(PhysicalKey::Code)
                .unwrap_or(PhysicalKey::Unidentified(NativeKeyCode::Unidentified)),
            Some(c.to_string()),
        );
    }
    (
        Key::Unidentified(winit::keyboard::NativeKey::Unidentified),
        PhysicalKey::Unidentified(NativeKeyCode::Unidentified),
        None,
    )
}
fn size(columns: u16, rows: u16) -> Result<(u16, u16), String> {
    if columns < 40 || rows < 24 {
        return Err("The terminal must be at least 40 columns by 24 rows".into());
    }
    if usize::from(columns) * usize::from(rows) > crate::cell_grid::MAX_CELLS {
        return Err("Terminal dimensions exceed the cell-grid limit".into());
    }
    Ok((columns, rows))
}
fn application(
    columns: u16,
    rows: u16,
    files: Vec<PathBuf>,
    output: Option<Box<dyn io::Write + Send>>,
    preferences: Option<PathBuf>,
) -> (Arc<Host>, App) {
    let dirty = Arc::new(AtomicBool::new(true));
    let flag = dirty.clone();
    let wake: crate::Wake = Arc::new(move || {
        flag.store(true, Ordering::Release);
    });
    let host = Host::cells(columns, rows, dirty);
    let app = App::from_shell(
        Shell {
            host: host.clone(),
            renderer: Renderer::cells(output),
            menus: None,
            wake,
            preferences,
        },
        files,
    );
    (host, app)
}
pub(crate) fn run(mut arguments: Vec<String>) -> Result<(), String> {
    if arguments.iter().any(|a| a == "--help" || a == "-h") {
        println!("Texted {}\nUsage: texted-tui [FILE ...]\n       texted --terminal [FILE ...]\n       texted-tui --dump-grid COLSxROWS [FILE ...]\n\nF2 Save · F3 Find · F5 Open · F6 New tab · F7/F8 Tabs · F9 Appearance\nF10 Close tab · Ctrl+Q Quit · Shift+F10 Context menu\nCtrl/Alt/Shift change F-key actions. Mouse and bracketed paste are supported.\nFont size and pointer shape are controlled by the terminal emulator.",env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if arguments.iter().any(|a| a == "--version" || a == "-V") {
        println!("Texted {} (terminal)", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if arguments.first().is_some_and(|a| a == "--dump-grid") {
        if arguments.len() < 2 {
            return Err("--dump-grid requires COLSxROWS".into());
        }
        let dimensions = arguments.remove(1);
        arguments.remove(0);
        let (c, r) = dimensions.split_once('x').ok_or("Expected COLSxROWS")?;
        let (c, r) = size(
            c.parse().map_err(|_| "Invalid columns")?,
            r.parse().map_err(|_| "Invalid rows")?,
        )?;
        let (_, mut app) = application(
            c,
            r,
            arguments.into_iter().map(PathBuf::from).collect(),
            None,
            None,
        );
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            app.tick();
            if app.settled() {
                break;
            }
            if Instant::now() > deadline {
                return Err("Timed out loading files".into());
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        app.redraw();
        let frame = app.rendered_grid().ok_or("No cell frame was produced")?;
        serde_json::to_writer(io::stdout().lock(), frame).map_err(|e| e.to_string())?;
        return Ok(());
    }
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err("Interactive mode requires a TTY; use --dump-grid for cell-grid JSON".into());
    }
    let (columns, rows) = terminal::size().map_err(|e| e.to_string())?;
    let (columns, rows) = size(columns, rows)?;
    let preferences =
        crate::preferences::default_path().map(|p| p.with_file_name("terminal-appearance.json"));
    let (host, mut app) = application(
        columns,
        rows,
        arguments.into_iter().map(PathBuf::from).collect(),
        Some(Box::new(io::stdout())),
        preferences,
    );
    let old_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore();
        old_hook(info);
    }));
    let _guard = TerminalGuard::enter().map_err(|e| e.to_string())?;
    let mut last_paint = Instant::now();
    let mut last_title = String::new();
    let mut legacy_modifiers_until = None;
    let mut legacy_key = None;
    let mut enhanced_keys = false;
    let mut fits = true;
    loop {
        if app.tick() {
            break;
        }
        if legacy_modifiers_until.is_some_and(|until| Instant::now() >= until) {
            app.set_modifiers(ModifiersState::empty());
            if let Some(key) = legacy_key.take() {
                app.key_release(&key);
            }
            legacy_modifiers_until = None;
        }
        if fits && (host.take_redraw() || last_paint.elapsed() >= Duration::from_millis(500)) {
            app.redraw();
            if let Some(error) = app.render_failure() {
                return Err(error.into());
            }
            last_paint = Instant::now();
            let title = host.title();
            if title != last_title {
                let safe: String = title.chars().filter(|c| !c.is_control()).collect();
                execute!(io::stdout(), terminal::SetTitle(safe)).map_err(|e| e.to_string())?;
                last_title = title;
            }
        }
        if !event::poll(Duration::from_millis(16)).map_err(|e| e.to_string())? {
            continue;
        }
        match event::read().map_err(|e| e.to_string())? {
            Event::Key(event) if fits => {
                let mut mods = modifiers(event.modifiers);
                if event.code == CKey::BackTab {
                    mods |= ModifiersState::SHIFT;
                }
                enhanced_keys |=
                    event.kind == KeyEventKind::Release || matches!(event.code, CKey::Modifier(_));
                if !enhanced_keys {
                    if let Some(key) = legacy_key.take() {
                        app.key_release(&key);
                    }
                }
                app.set_modifiers(mods);
                let (logical, physical, text) = key(event);
                if !enhanced_keys {
                    legacy_key = Some(logical.clone());
                }
                if event.kind == KeyEventKind::Release {
                    app.key_release(&logical);
                } else {
                    app.key(&KeyInput {
                        physical,
                        logical: &logical,
                        text: text.as_deref(),
                        modifiers: mods,
                    });
                }
                // Legacy terminals have no modifier release event. Keep a brief,
                // visible layer preview without leaving a stale Ctrl/Alt panel.
                legacy_modifiers_until = if enhanced_keys {
                    None
                } else {
                    Some(Instant::now() + Duration::from_millis(180))
                };
            }
            Event::Paste(text) if fits => app.paste_text(text),
            Event::Resize(c, r) => {
                fits = size(c, r).is_ok();
                if fits {
                    host.resize_cells(c, r);
                    app.resized();
                } else {
                    execute!(
                        io::stdout(),
                        terminal::Clear(terminal::ClearType::All),
                        cursor::MoveTo(0, 0),
                        cursor::Hide,
                        crossterm::style::Print("Resize to at least 40 x 24")
                    )
                    .map_err(|e| e.to_string())?;
                }
            }
            Event::Mouse(event) if fits => {
                app.set_modifiers(modifiers(event.modifiers));
                app.pointer_move(
                    f32::from(event.column) * 8. + 4.,
                    f32::from(event.row) * 16. + 8.,
                );
                match event.kind {
                    MouseEventKind::Down(MouseButton::Left) => {
                        app.pointer_button(ElementState::Pressed)
                    }
                    MouseEventKind::Up(MouseButton::Left) => {
                        app.pointer_button(ElementState::Released)
                    }
                    MouseEventKind::Down(MouseButton::Right) => app.context_click(),
                    MouseEventKind::ScrollUp => app.wheel_lines(0., 1.),
                    MouseEventKind::ScrollDown => app.wheel_lines(0., -1.),
                    MouseEventKind::ScrollLeft => app.wheel_lines(1., 0.),
                    MouseEventKind::ScrollRight => app.wheel_lines(-1., 0.),
                    _ => {}
                }
            }
            Event::FocusLost => {
                app.set_modifiers(ModifiersState::empty());
                app.pointer_button(ElementState::Released);
            }
            _ => {}
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ukrainian_shortcut_positions_preserve_inserted_characters() {
        let (logical, physical, text) = key(KeyEvent::new(CKey::Char('і'), KeyModifiers::CONTROL));
        assert_eq!(physical, PhysicalKey::Code(KeyCode::KeyS));
        assert_eq!(logical, Key::Character("і".into()));
        assert_eq!(text.as_deref(), Some("і"));
    }
    #[test]
    fn shared_app_renders_loaded_text_tabs_commands_and_modal_without_a_window() {
        let (_, mut app) = application(100, 30, vec![], None, None);
        app.tick();
        app.paste_text("Привіт, Ґава!".into());
        let deadline = Instant::now() + Duration::from_secs(2);
        while !app.settled() {
            app.tick();
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(2));
        }
        app.redraw();
        let text = app.rendered_grid().unwrap().text();
        assert!(text.contains("Привіт, Ґава!"), "{text}");
        assert!(text.contains("Untitled"));
        assert!(text.contains("Save"));
        let logical = Key::Named(NamedKey::F2);
        app.key(&KeyInput {
            physical: PhysicalKey::Unidentified(NativeKeyCode::Unidentified),
            logical: &logical,
            text: None,
            modifiers: ModifiersState::empty(),
        });
        for _ in 0..30 {
            app.tick();
            std::thread::sleep(Duration::from_millis(2));
        }
        app.redraw();
        assert!(app.rendered_grid().unwrap().text().contains("Save file"));
        let escape = Key::Named(NamedKey::Escape);
        app.key(&KeyInput {
            physical: PhysicalKey::Unidentified(NativeKeyCode::Unidentified),
            logical: &escape,
            text: None,
            modifiers: ModifiersState::empty(),
        });
        app.tick();
        app.redraw();
        assert!(!app.rendered_grid().unwrap().text().contains("Save file"));
    }
}
