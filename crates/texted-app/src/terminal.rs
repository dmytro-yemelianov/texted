//! Terminal transport for the same App, scene, commands and dialogs as the window.
use crate::{
    app::{App, Shell},
    input::KeyInput,
    surface::{Host, Renderer},
    terminal_input::{Message, Reader},
    terminal_probe::{self, Mode},
};
use crossterm::{
    cursor,
    event::{
        DisableBracketedPaste, DisableFocusChange, DisableMouseCapture, EnableBracketedPaste,
        EnableFocusChange, EnableMouseCapture, Event, KeyCode as CKey, KeyEvent, KeyEventKind,
        KeyModifiers, KeyboardEnhancementFlags, MouseButton, MouseEventKind,
        PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    execute,
    terminal::{self, DisableLineWrap, EnableLineWrap, EnterAlternateScreen, LeaveAlternateScreen},
};
use std::{
    io::{self, IsTerminal, Write},
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
struct TerminalGuard {
    registry: crate::kitty::Registry,
}
impl TerminalGuard {
    fn enter(registry: crate::kitty::Registry) -> io::Result<Self> {
        terminal::enable_raw_mode()?;
        ACTIVE.store(true, Ordering::Release);
        let guard = Self { registry };
        execute!(
            io::stdout(),
            EnterAlternateScreen,
            DisableLineWrap,
            EnableMouseCapture,
            crossterm::style::Print("\x1b[?1016l"),
            EnableBracketedPaste,
            EnableFocusChange,
            PushKeyboardEnhancementFlags(
                KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                    | KeyboardEnhancementFlags::REPORT_EVENT_TYPES
                    | KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES
                    | KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS
            )
        )?;
        Ok(guard)
    }
}
fn restore(registry: &crate::kitty::Registry) {
    if !ACTIVE.swap(false, Ordering::AcqRel) {
        return;
    }
    let _ = crate::kitty::cleanup(&mut io::stdout(), registry);
    let _ = execute!(
        io::stdout(),
        crossterm::style::ResetColor,
        PopKeyboardEnhancementFlags,
        DisableFocusChange,
        DisableBracketedPaste,
        DisableMouseCapture,
        crossterm::style::Print("\x1b[?1016l"),
        EnableLineWrap,
        cursor::Show,
        LeaveAlternateScreen
    );
    let _ = terminal::disable_raw_mode();
}
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore(&self.registry);
    }
}
/// Hand the terminal back before a fatal panic prints. Worker panics are caught and the
/// editor keeps drawing, so only a panic on the thread that owns the terminal restores it.
fn restore_on_terminal_thread(
    owner: std::thread::ThreadId,
    restore: impl Fn() + Send + Sync + 'static,
) -> impl Fn() + Send + Sync + 'static {
    move || {
        if std::thread::current().id() == owner {
            restore();
        }
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
    application_with_graphics(
        columns,
        rows,
        files,
        output,
        preferences,
        None,
        crate::kitty::Registry::default(),
    )
}
fn application_with_graphics(
    columns: u16,
    rows: u16,
    files: Vec<PathBuf>,
    output: Option<Box<dyn io::Write + Send>>,
    preferences: Option<PathBuf>,
    graphics: Option<crate::kitty::Size>,
    registry: crate::kitty::Registry,
) -> (Arc<Host>, App) {
    let dirty = Arc::new(AtomicBool::new(true));
    let flag = dirty.clone();
    let wake: crate::Wake = Arc::new(move || {
        flag.store(true, Ordering::Release);
    });
    let host = if let Some(size) = graphics {
        Host::graphics(size, dirty).expect("validated terminal geometry")
    } else {
        Host::cells(columns, rows, dirty)
    };
    let renderer = if let Some(size) = graphics {
        Renderer::kitty(size, output.expect("graphics output"), registry)
    } else {
        Renderer::cells(output)
    };
    let app = App::from_shell(
        Shell {
            host: host.clone(),
            renderer,
            menus: None,
            wake,
            preferences,
        },
        files,
    );
    (host, app)
}
fn resize_notice() -> Result<(), String> {
    execute!(
        io::stdout(),
        terminal::Clear(terminal::ClearType::All),
        cursor::MoveTo(0, 0),
        cursor::Hide,
        crossterm::style::Print("Resize terminal: min 40x24, max 16MP")
    )
    .map_err(|e| e.to_string())
}
pub(crate) fn run(mut arguments: Vec<String>) -> Result<(), String> {
    if arguments.iter().any(|a| a == "--help" || a == "-h") {
        println!("Texted {}\nUsage: texted-tui [--render=auto|kitty|ansi] [FILE ...]\n       texted --terminal [FILE ...]\n       texted-tui --dump-grid COLSxROWS [FILE ...]\n\nF2 Save · F3 Find · F5 Open · F6 New tab · F7/F8 Tabs · F9 Appearance\nF10 Close tab · Ctrl+Q Quit · Shift+F10 Context menu\nCtrl/Alt/Shift change F-key actions. Mouse and bracketed paste are supported.\n--render=auto|kitty|ansi selects pixel graphics or plain text.\nGraphics mode uses Texted bitmap fonts and scaling; ANSI uses the terminal font.",env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if arguments.iter().any(|a| a == "--version" || a == "-V") {
        println!("Texted {} (terminal)", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    let mut mode = Mode::Auto;
    let mut files = Vec::new();
    let mut args = arguments.into_iter();
    while let Some(arg) = args.next() {
        if arg == "--" {
            files.extend(args);
            break;
        } else if arg == "--render" {
            mode = Mode::parse(&args.next().ok_or("--render requires auto, ansi or kitty")?)?;
        } else if let Some(value) = arg.strip_prefix("--render=") {
            mode = Mode::parse(value)?;
        } else {
            files.push(arg);
        }
    }
    arguments = files;
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
    let registry = crate::kitty::Registry::default();
    let panic_registry = registry.clone();
    let restore_terminal = restore_on_terminal_thread(std::thread::current().id(), move || {
        restore(&panic_registry)
    });
    let old_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        old_hook(info);
    }));
    let _guard = TerminalGuard::enter(registry.clone()).map_err(|e| e.to_string())?;
    let mut reader = Reader::default();
    let (mut graphics, mut queued, mut precise_cell) =
        terminal_probe::negotiate(&mut reader, &mut io::stdout(), mode, columns, rows)?;
    let mut pixel_mouse = false;
    if graphics.is_some() {
        io::stdout()
            .write_all(b"\x1b[?1016$p")
            .and_then(|()| io::stdout().flush())
            .map_err(|e| e.to_string())?;
    }
    let filename = if graphics.is_some() {
        "terminal-graphics.json"
    } else {
        "terminal-appearance.json"
    };
    let preferences = std::env::var_os("TEXTED_SETTINGS_PATH")
        .map(PathBuf::from)
        .or_else(|| crate::preferences::default_path().map(|p| p.with_file_name(filename)));
    let (host, mut app) = application_with_graphics(
        columns,
        rows,
        arguments.into_iter().map(PathBuf::from).collect(),
        Some(Box::new(io::stdout())),
        preferences,
        graphics,
        registry,
    );
    let mut dimensions = (columns, rows);
    let mut geometry_poll = Instant::now();
    let mut pixel_dimensions = terminal::window_size().ok().map(|s| (s.width, s.height));
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
        if geometry_poll.elapsed() >= Duration::from_millis(50) {
            geometry_poll = Instant::now();
            if let Ok(size) = terminal::window_size() {
                let (c, r) = (size.columns, size.rows);
                let pixels = Some((size.width, size.height));
                if (c, r) != dimensions || (graphics.is_some() && pixels != pixel_dimensions) {
                    pixel_dimensions = pixels;
                    dimensions = (c, r);
                    precise_cell = false;
                    fits = self::size(c, r).is_ok();
                    if fits {
                        if let Some(current) = &mut graphics {
                            current.columns = c;
                            current.rows = r;
                            if let Some((w, h)) = terminal_probe::cell_size(c, r) {
                                current.cell_width = w;
                                current.cell_height = h;
                            }
                            fits = current.pixels().is_ok();
                            if fits {
                                app.resize_graphics(*current)?;
                            } else {
                                app.suspend_graphics()?;
                                resize_notice()?;
                            }
                            terminal_probe::pixel_query(&mut io::stdout())
                                .map_err(|e| e.to_string())?;
                        } else {
                            host.resize_cells(c, r);
                            app.resized();
                        }
                    } else {
                        app.suspend_graphics()?;
                        resize_notice()?;
                    }
                }
            }
        }
        let message = if let Some(event) = queued.pop_front() {
            Some(Message::Input(event))
        } else {
            reader
                .read(Duration::from_millis(16))
                .map_err(|e| e.to_string())?
        };
        let event = match message {
            Some(Message::Input(event)) => event,
            Some(Message::Pixels {
                cell,
                width,
                height,
            }) => {
                if let Some(current) = &mut graphics {
                    if current.update_report(&mut precise_cell, cell, width, height) {
                        fits = self::size(dimensions.0, dimensions.1).is_ok()
                            && current.pixels().is_ok();
                        if fits {
                            app.resize_graphics(*current)?;
                        } else {
                            app.suspend_graphics()?;
                            resize_notice()?;
                        }
                    }
                }
                continue;
            }
            Some(Message::PixelMouse(available)) => {
                if available && graphics.is_some() {
                    io::stdout()
                        .write_all(b"\x1b[?1016h")
                        .and_then(|()| io::stdout().flush())
                        .map_err(|e| e.to_string())?;
                    pixel_mouse = true;
                }
                continue;
            }
            Some(Message::Notice(message)) => {
                app.notice(message);
                continue;
            }
            _ => continue,
        };
        match event {
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
            Event::Mouse(event) if fits => {
                app.set_modifiers(modifiers(event.modifiers));
                let (x, y) = if pixel_mouse {
                    (f32::from(event.column) + 0.5, f32::from(event.row) + 0.5)
                } else {
                    graphics.map_or(
                        (
                            f32::from(event.column) * 8. + 4.,
                            f32::from(event.row) * 16. + 8.,
                        ),
                        |size| size.pointer(event.column, event.row),
                    )
                };
                app.pointer_move(x, y);
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
            Event::FocusGained => {
                app.resized();
                app.focus_gained();
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
    fn press(app: &mut App, key: NamedKey) {
        let logical = Key::Named(key);
        app.key(&KeyInput {
            physical: PhysicalKey::Unidentified(NativeKeyCode::Unidentified),
            logical: &logical,
            text: None,
            modifiers: ModifiersState::empty(),
        });
    }
    fn settle(app: &mut App) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !app.settled() {
            app.tick();
            assert!(Instant::now() < deadline, "app never settled");
            std::thread::sleep(Duration::from_millis(2));
        }
        app.tick();
    }
    /// Ticks and redraws until the frame shows `expected`.
    fn wait_for_text(app: &mut App, expected: &str) -> String {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            app.tick();
            app.redraw();
            let text = app.rendered_grid().unwrap().text();
            if text.contains(expected) {
                return text;
            }
            assert!(
                Instant::now() < deadline,
                "never showed {expected:?}:\n{text}"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    #[test]
    fn search_switches_on_function_keys_change_what_matches() {
        let dir = std::env::temp_dir().join(format!("texted-search-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("cats.txt");
        std::fs::write(&path, "Кіт кіт кітки КІТ\n").unwrap();
        let (_, mut app) = application(100, 30, vec![path], None, None);
        settle(&mut app);
        let find = Key::Character("f".into());
        app.key(&KeyInput {
            physical: PhysicalKey::Code(KeyCode::KeyF),
            logical: &find,
            text: None,
            modifiers: ModifiersState::SUPER,
        });
        app.paste_text("кіт".into());
        let text = wait_for_text(&mut app, "/2");
        assert!(text.contains("Case") && text.contains("Part"), "{text}");
        press(&mut app, NamedKey::F5);
        let text = wait_for_text(&mut app, "/4");
        assert!(text.contains("NoCs"), "{text}");
        press(&mut app, NamedKey::F6);
        let text = wait_for_text(&mut app, "/3");
        assert!(text.contains("Word"), "{text}");
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn cmd_j_jumps_to_a_typed_line() {
        let dir = std::env::temp_dir().join(format!("texted-goto-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("lines.txt");
        let text: String = (1..=40).map(|n| format!("line {n}\n")).collect();
        std::fs::write(&path, text).unwrap();
        let (_, mut app) = application(100, 30, vec![path], None, None);
        settle(&mut app);
        let jump = Key::Character("j".into());
        app.key(&KeyInput {
            physical: PhysicalKey::Code(KeyCode::KeyJ),
            logical: &jump,
            text: None,
            modifiers: ModifiersState::SUPER,
        });
        app.paste_text("12".into());
        wait_for_text(&mut app, "Enter: line 12");
        press(&mut app, NamedKey::Enter);
        settle(&mut app);
        press(&mut app, NamedKey::F12);
        wait_for_text(&mut app, "Line 12");
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn quitting_keeps_unsaved_tabs_and_the_next_start_restores_them() {
        let dir = std::env::temp_dir().join(format!("texted-hot-exit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("notes.txt");
        std::fs::write(&path, "one\ntwo\n").unwrap();
        let store = dir.join("session");
        {
            let (_, mut app) = application(100, 30, vec![path.clone()], None, None);
            app.keep_session(store.clone());
            settle(&mut app);
            press(&mut app, NamedKey::ArrowDown);
            settle(&mut app);
            app.paste_text("mine ".into());
            settle(&mut app);
            press(&mut app, NamedKey::F6);
            settle(&mut app);
            app.paste_text("draft".into());
            settle(&mut app);
            let quit = Key::Character("q".into());
            app.key(&KeyInput {
                physical: PhysicalKey::Code(KeyCode::KeyQ),
                logical: &quit,
                text: None,
                modifiers: ModifiersState::SUPER,
            });
            let deadline = Instant::now() + Duration::from_secs(5);
            while !app.tick() {
                assert!(Instant::now() < deadline, "quit waited for an answer");
                std::thread::sleep(Duration::from_millis(2));
            }
        }
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "one\ntwo\n");
        let (_, mut app) = application(100, 30, vec![], None, None);
        app.keep_session(store);
        settle(&mut app);
        app.redraw();
        let text = app.rendered_grid().unwrap().text();
        assert!(text.contains("notes.txt *"), "{text}");
        assert!(text.contains("Untitled *"), "{text}");
        assert!(text.contains("draft"), "{text}");
        press(&mut app, NamedKey::F7);
        settle(&mut app);
        press(&mut app, NamedKey::F12);
        app.tick();
        app.redraw();
        let text = app.rendered_grid().unwrap().text();
        assert!(text.contains("mine two"), "{text}");
        assert!(text.contains("Line 2"), "{text}");
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn regaining_focus_reloads_a_file_changed_elsewhere_and_keeps_the_caret_line() {
        let dir = std::env::temp_dir().join(format!("texted-focus-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("notes.txt");
        std::fs::write(&path, "one\ntwo\nthree\n").unwrap();
        let (_, mut app) = application(100, 30, vec![path.clone()], None, None);
        settle(&mut app);
        press(&mut app, NamedKey::ArrowDown);
        press(&mut app, NamedKey::ArrowDown);
        settle(&mut app);
        std::thread::sleep(Duration::from_millis(20));
        std::fs::write(&path, "one\ntwo\nthree, edited elsewhere\nfour\n").unwrap();
        app.focus_gained();
        settle(&mut app);
        press(&mut app, NamedKey::F12);
        app.tick();
        app.redraw();
        let text = app.rendered_grid().unwrap().text();
        assert!(text.contains("three, edited elsewhere"), "{text}");
        assert!(text.contains("Line 3"), "{text}");
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn details_show_a_legacy_file_encoding_and_offer_a_reread() {
        let dir = std::env::temp_dir().join(format!("texted-details-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("legacy.txt");
        std::fs::write(&path, [191, 230, 224, 234, 10]).unwrap(); // "їжак\n" in Windows-1251
        let (_, mut app) = application(100, 30, vec![path], None, None);
        let deadline = Instant::now() + Duration::from_secs(5);
        while !app.settled() {
            app.tick();
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(2));
        }
        let details = Key::Named(NamedKey::F12);
        app.key(&KeyInput {
            physical: PhysicalKey::Unidentified(NativeKeyCode::Unidentified),
            logical: &details,
            text: None,
            modifiers: ModifiersState::empty(),
        });
        app.tick();
        app.redraw();
        let text = app.rendered_grid().unwrap().text();
        assert!(text.contains("їжак"), "{text}");
        assert!(text.contains("Encoding: Windows-1251"), "{text}");
        assert!(text.contains("Reread as KOI8-U"), "{text}");
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn a_frame_shorter_than_the_command_bar_still_renders() {
        // The window and graphics backends share this redraw; a 16 px frame is below the bar.
        let size = crate::kitty::Size {
            columns: 80,
            rows: 1,
            cell_width: 8,
            cell_height: 16,
        };
        let (_, mut app) = application_with_graphics(
            80,
            1,
            vec![],
            Some(Box::new(io::sink())),
            None,
            Some(size),
            crate::kitty::Registry::default(),
        );
        app.tick();
        app.redraw();
        assert_eq!(app.render_failure(), None);
        // Appearance and dialogs paint into the zero-height area above the bar.
        for key in [NamedKey::F9, NamedKey::F2] {
            let logical = Key::Named(key);
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
            assert_eq!(app.render_failure(), None);
        }
    }
    #[test]
    fn worker_panics_leave_the_terminal_to_its_owner() {
        let restores = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count = restores.clone();
        let hook = Arc::new(restore_on_terminal_thread(
            std::thread::current().id(),
            move || {
                count.fetch_add(1, Ordering::SeqCst);
            },
        ));
        // Worker panics are caught and the editor keeps running, so the terminal must stay set up.
        let worker = hook.clone();
        std::thread::spawn(move || worker()).join().unwrap();
        assert_eq!(restores.load(Ordering::SeqCst), 0);
        hook();
        assert_eq!(restores.load(Ordering::SeqCst), 1);
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
