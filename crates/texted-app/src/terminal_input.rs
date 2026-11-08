//! One bounded byte-stream owner: terminal replies never become document input.
use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, ModifierKeyCode, MouseButton, MouseEvent,
    MouseEventKind,
};
use std::{
    collections::VecDeque,
    io::{self, Read},
    time::{Duration, Instant},
};
#[derive(Debug)]
pub(crate) enum Message {
    PixelMouse(bool),
    Input(Event),
    Graphics { id: u32, ok: bool },
    Pixels { cell: bool, width: u16, height: u16 },
    Notice(&'static str),
}
#[derive(Default)]
pub(crate) struct Decoder {
    pending: Vec<u8>,
    paste: Option<Vec<u8>>,
    discard_paste: bool,
    escape: Option<Instant>,
    messages: VecDeque<Message>,
}
fn number(s: Option<&str>) -> u32 {
    s.and_then(|s| s.parse().ok()).unwrap_or(0)
}
fn modifiers(n: u32) -> KeyModifiers {
    let n = n.saturating_sub(1);
    let mut m = KeyModifiers::empty();
    for (bit, value) in [
        (1, KeyModifiers::SHIFT),
        (2, KeyModifiers::ALT),
        (4, KeyModifiers::CONTROL),
        (8, KeyModifiers::SUPER),
        (16, KeyModifiers::HYPER),
        (32, KeyModifiers::META),
    ] {
        if n & bit != 0 {
            m |= value;
        }
    }
    m
}
fn character(c: char, mut mods: KeyModifiers) -> KeyEvent {
    let code = match c {
        '\r' => KeyCode::Enter,
        '\t' => KeyCode::Tab,
        '\x1b' => KeyCode::Esc,
        '\x7f' => KeyCode::Backspace,
        '\0' => {
            mods |= KeyModifiers::CONTROL;
            KeyCode::Char(' ')
        }
        c @ '\x01'..='\x1a' => {
            mods |= KeyModifiers::CONTROL;
            KeyCode::Char(char::from(c as u8 - 1 + b'a'))
        }
        c @ '\x1c'..='\x1f' => {
            mods |= KeyModifiers::CONTROL;
            KeyCode::Char(char::from(c as u8 - 0x1c + b'4'))
        }
        c => {
            if c.is_uppercase() {
                mods |= KeyModifiers::SHIFT;
            }
            KeyCode::Char(c)
        }
    };
    KeyEvent::new(code, mods)
}
fn functional(n: u32) -> Option<KeyCode> {
    Some(match n {
        57344 => KeyCode::Esc,
        57345 => KeyCode::Enter,
        57346 => KeyCode::Tab,
        57347 => KeyCode::Backspace,
        57348 => KeyCode::Insert,
        57349 => KeyCode::Delete,
        57350 => KeyCode::Left,
        57351 => KeyCode::Right,
        57352 => KeyCode::Up,
        57353 => KeyCode::Down,
        57354 => KeyCode::PageUp,
        57355 => KeyCode::PageDown,
        57356 => KeyCode::Home,
        57357 => KeyCode::End,
        57364..=57398 => KeyCode::F((n - 57363) as u8),
        57441 => KeyCode::Modifier(ModifierKeyCode::LeftShift),
        57442 => KeyCode::Modifier(ModifierKeyCode::LeftControl),
        57443 => KeyCode::Modifier(ModifierKeyCode::LeftAlt),
        57444 => KeyCode::Modifier(ModifierKeyCode::LeftSuper),
        57447 => KeyCode::Modifier(ModifierKeyCode::RightShift),
        57448 => KeyCode::Modifier(ModifierKeyCode::RightControl),
        57449 => KeyCode::Modifier(ModifierKeyCode::RightAlt),
        57450 => KeyCode::Modifier(ModifierKeyCode::RightSuper),
        _ => return None,
    })
}
impl Decoder {
    pub fn feed(&mut self, bytes: &[u8]) {
        self.pending.extend_from_slice(bytes);
        loop {
            if self.paste.is_some() {
                let end = self.pending.windows(6).position(|w| w == b"\x1b[201~");
                let take = end.unwrap_or_else(|| self.pending.len().saturating_sub(5));
                let paste = self.paste.as_mut().unwrap();
                if paste.len().saturating_add(take) > crate::clipboard::CLIPBOARD_LIMIT {
                    self.discard_paste = true;
                    paste.clear();
                }
                if !self.discard_paste {
                    paste.extend_from_slice(&self.pending[..take]);
                }
                self.pending.drain(..take);
                if end.is_some() {
                    self.pending.drain(..6);
                    self.escape = None;
                    let paste = self.paste.take().unwrap();
                    if self.discard_paste {
                        self.messages
                            .push_back(Message::Notice("Paste exceeds 128 MiB"));
                    } else {
                        self.messages.push_back(Message::Input(Event::Paste(
                            String::from_utf8_lossy(&paste).into_owned(),
                        )));
                    }
                    self.discard_paste = false;
                    continue;
                }
                break;
            }
            if self.pending.is_empty() {
                self.escape = None;
                break;
            }
            if self.pending[0] == 0x1b {
                self.escape.get_or_insert_with(Instant::now);
                if self.pending.len() == 1 {
                    break;
                }
                match self.pending[1] {
                    b'[' => {
                        let end = self.pending[2..]
                            .iter()
                            .position(|b| (0x40..=0x7e).contains(b))
                            .map(|i| i + 2);
                        let Some(end) = end else {
                            if self.pending.len() > 4096 {
                                self.pending.clear();
                            }
                            break;
                        };
                        let body = String::from_utf8_lossy(&self.pending[2..end]).into_owned();
                        let final_byte = self.pending[end];
                        self.pending.drain(..=end);
                        self.escape = None;
                        self.csi(&body, final_byte);
                        continue;
                    }
                    b'_' | b']' | b'P' => {
                        let end = self.pending[2..]
                            .windows(2)
                            .position(|w| w == b"\x1b\\")
                            .map(|i| i + 2);
                        let bell = if self.pending[1] == b']' {
                            self.pending[2..]
                                .iter()
                                .position(|b| *b == 7)
                                .map(|i| i + 2)
                        } else {
                            None
                        };
                        let end = match (end, bell) {
                            (Some(a), Some(b)) => Some((a.min(b), if b < a { 1 } else { 2 })),
                            (Some(a), None) => Some((a, 2)),
                            (None, Some(b)) => Some((b, 1)),
                            _ => None,
                        };
                        if let Some((end, terminator)) = end {
                            let body = String::from_utf8_lossy(&self.pending[2..end]).into_owned();
                            if self.pending[1] == b'_' {
                                self.reply(&body);
                            }
                            self.pending.drain(..end + terminator);
                            self.escape = None;
                            continue;
                        }
                        // A broken control string stays quarantined until its terminator.
                        // Keep its start and bounded tail; never leak its payload as keys.
                        if self.pending.len() > 8192 {
                            let len = self.pending.len();
                            self.pending.drain(2..len - 2);
                        }
                        break;
                    }
                    b'O' => {
                        if self.pending.len() < 3 {
                            break;
                        }
                        let code = match self.pending[2] {
                            b'A' => Some(KeyCode::Up),
                            b'B' => Some(KeyCode::Down),
                            b'C' => Some(KeyCode::Right),
                            b'D' => Some(KeyCode::Left),
                            b'H' => Some(KeyCode::Home),
                            b'F' => Some(KeyCode::End),
                            v @ b'P'..=b'S' => Some(KeyCode::F(v - b'P' + 1)),
                            _ => None,
                        };
                        self.pending.drain(..3);
                        self.escape = None;
                        if let Some(code) = code {
                            self.key(KeyEvent::new(code, KeyModifiers::empty()));
                        }
                        continue;
                    }
                    0x1b => {
                        self.pending.remove(0);
                        self.escape = None;
                        self.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::empty()));
                        continue;
                    }
                    _ => {}
                }
            }
            let alt = self.pending[0] == 0x1b;
            let offset = usize::from(alt);
            let first = self.pending[offset];
            let len = if first < 128 {
                1
            } else if first < 224 {
                2
            } else if first < 240 {
                3
            } else {
                4
            };
            if self.pending.len() < offset + len {
                break;
            }
            let c = std::str::from_utf8(&self.pending[offset..offset + len])
                .ok()
                .and_then(|s| s.chars().next())
                .unwrap_or('�');
            self.pending.drain(..offset + len);
            self.escape = None;
            self.key(character(
                c,
                if alt {
                    KeyModifiers::ALT
                } else {
                    KeyModifiers::empty()
                },
            ));
        }
    }
    fn key(&mut self, key: KeyEvent) {
        self.messages.push_back(Message::Input(Event::Key(key)));
    }
    fn reply(&mut self, body: &str) {
        if let Some(body) = body.strip_prefix('G') {
            if let Some((params, result)) = body.split_once(';') {
                let id = params
                    .split(',')
                    .find_map(|s| s.strip_prefix("i="))
                    .and_then(|s| s.parse().ok());
                if let Some(id) = id {
                    self.messages.push_back(Message::Graphics {
                        id,
                        ok: result == "OK",
                    });
                }
            }
        }
    }
    fn csi(&mut self, body: &str, final_byte: u8) {
        if final_byte == b't' {
            let mut p = body.split(';');
            let kind = number(p.next());
            let h = number(p.next());
            let w = number(p.next());
            if matches!(kind, 4 | 6)
                && w > 0
                && h > 0
                && w <= u16::MAX as u32
                && h <= u16::MAX as u32
            {
                self.messages.push_back(Message::Pixels {
                    cell: kind == 6,
                    width: w as u16,
                    height: h as u16,
                });
            }
            return;
        }
        if final_byte == b'y' {
            if let Some(state) = body
                .strip_prefix("?1016;")
                .and_then(|s| s.strip_suffix('$'))
            {
                self.messages
                    .push_back(Message::PixelMouse(matches!(state, "1" | "2")));
                return;
            }
        }
        if body.starts_with(['?', '>', '=']) {
            return;
        }
        if matches!(final_byte, b'M' | b'm') && body.starts_with('<') {
            let mut p = body[1..].split(';');
            let cb = number(p.next());
            let x = number(p.next());
            let y = number(p.next());
            if x == 0 || y == 0 || x > 65536 || y > 65536 {
                return;
            }
            let mut mods = KeyModifiers::empty();
            for (bit, m) in [
                (4, KeyModifiers::SHIFT),
                (8, KeyModifiers::ALT),
                (16, KeyModifiers::CONTROL),
            ] {
                if cb & bit != 0 {
                    mods |= m;
                }
            }
            let button = match cb & 3 {
                0 => MouseButton::Left,
                1 => MouseButton::Middle,
                _ => MouseButton::Right,
            };
            let kind = if cb & 64 != 0 {
                match cb & 3 {
                    0 => MouseEventKind::ScrollUp,
                    1 => MouseEventKind::ScrollDown,
                    2 => MouseEventKind::ScrollLeft,
                    _ => MouseEventKind::ScrollRight,
                }
            } else if final_byte == b'm' {
                MouseEventKind::Up(button)
            } else if cb & 32 != 0 {
                if cb & 3 == 3 {
                    MouseEventKind::Moved
                } else {
                    MouseEventKind::Drag(button)
                }
            } else {
                MouseEventKind::Down(button)
            };
            self.messages
                .push_back(Message::Input(Event::Mouse(MouseEvent {
                    kind,
                    column: (x - 1) as u16,
                    row: (y - 1) as u16,
                    modifiers: mods,
                })));
            return;
        }
        let p: Vec<_> = body.split(';').collect();
        let first = number(p.first().copied());
        let mk: Vec<_> = p.get(1).unwrap_or(&"1").split(':').collect();
        let mut mods = modifiers(number(mk.first().copied()));
        let kind = match number(mk.get(1).copied()) {
            2 => KeyEventKind::Repeat,
            3 => KeyEventKind::Release,
            _ => KeyEventKind::Press,
        };
        let code = match final_byte {
            b'u' => {
                let mut codes = p[0].split(':');
                let base = number(codes.next());
                let shifted = number(codes.next());
                functional(base).or_else(|| {
                    if (57344..=57454).contains(&base) {
                        return None;
                    }
                    char::from_u32(if mods.contains(KeyModifiers::SHIFT) && shifted > 0 {
                        shifted
                    } else {
                        base
                    })
                    .map(|mut c| {
                        if mods.contains(KeyModifiers::SHIFT) && shifted == 0 {
                            c = c.to_uppercase().next().unwrap_or(c);
                        }
                        character(c, mods).code
                    })
                })
            }
            b'A' => Some(KeyCode::Up),
            b'B' => Some(KeyCode::Down),
            b'C' => Some(KeyCode::Right),
            b'D' => Some(KeyCode::Left),
            b'H' => Some(KeyCode::Home),
            b'F' => Some(KeyCode::End),
            b'P'..=b'S' => Some(KeyCode::F(final_byte - b'P' + 1)),
            b'Z' => {
                mods |= KeyModifiers::SHIFT;
                Some(KeyCode::BackTab)
            }
            b'~' => match first {
                200 => {
                    self.paste = Some(Vec::new());
                    return;
                }
                1 | 7 => Some(KeyCode::Home),
                2 => Some(KeyCode::Insert),
                3 => Some(KeyCode::Delete),
                4 | 8 => Some(KeyCode::End),
                5 => Some(KeyCode::PageUp),
                6 => Some(KeyCode::PageDown),
                11..=15 => Some(KeyCode::F((first - 10) as u8)),
                17..=21 => Some(KeyCode::F((first - 11) as u8)),
                23..=24 => Some(KeyCode::F((first - 12) as u8)),
                _ => None,
            },
            b'I' => {
                self.messages.push_back(Message::Input(Event::FocusGained));
                return;
            }
            b'O' => {
                self.messages.push_back(Message::Input(Event::FocusLost));
                return;
            }
            _ => None,
        };
        if let Some(code) = code {
            self.key(KeyEvent::new_with_kind(code, mods, kind));
        }
    }
    pub fn next(&mut self) -> Option<Message> {
        self.messages.pop_front()
    }
    fn expire_escape(&mut self) {
        if self.paste.is_none()
            && self.pending == [0x1b]
            && self
                .escape
                .is_some_and(|t| t.elapsed() > Duration::from_millis(30))
        {
            self.pending.clear();
            self.escape = None;
            self.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::empty()));
        }
    }
}
#[derive(Default)]
pub(crate) struct Reader {
    decoder: Decoder,
}
impl Reader {
    pub fn read(&mut self, timeout: Duration) -> io::Result<Option<Message>> {
        self.decoder.expire_escape();
        if let Some(m) = self.decoder.next() {
            return Ok(Some(m));
        }
        #[cfg(unix)]
        {
            let mut fd = libc::pollfd {
                fd: 0,
                events: libc::POLLIN,
                revents: 0,
            };
            // stdin is owned by this reader, in raw mode. No crossterm event reader runs.
            let ready =
                unsafe { libc::poll(&mut fd, 1, timeout.as_millis().min(i32::MAX as u128) as i32) };
            if ready < 0 {
                let e = io::Error::last_os_error();
                return if e.kind() == io::ErrorKind::Interrupted {
                    Ok(None)
                } else {
                    Err(e)
                };
            }
            if ready > 0 {
                let mut bytes = [0; 32768];
                let n = io::stdin().read(&mut bytes)?;
                if n == 0 {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "Terminal input closed",
                    ));
                }
                self.decoder.feed(&bytes[..n]);
            }
            self.decoder.expire_escape();
            Ok(self.decoder.next())
        }
        #[cfg(not(unix))]
        {
            let _ = timeout;
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "Terminal graphics transport requires a Unix TTY",
            ))
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pixel_mouse_capability_replies_are_not_key_input() {
        let mut d = Decoder::default();
        d.feed(b"\x1b[?1016;2$y\x1b[?1016;0$y");
        assert!(matches!(d.next(), Some(Message::PixelMouse(true))));
        assert!(matches!(d.next(), Some(Message::PixelMouse(false))));
        assert!(d.next().is_none());
    }
    #[test]
    fn a_slow_paste_keeps_literal_escape_bytes_opaque() {
        let mut d = Decoder::default();
        d.feed(b"\x1b[200~\x1b");
        d.escape = Some(Instant::now() - Duration::from_millis(100));
        d.expire_escape();
        assert!(d.next().is_none());
        d.feed(b"A\x1b[201~");
        assert!(matches!(d.next(),Some(Message::Input(Event::Paste(s))) if s=="\x1bA"));
    }
    #[test]
    fn shift_text_and_unknown_control_strings_do_not_corrupt_the_document() {
        let mut d = Decoder::default();
        d.feed(b"\x1b[97;2u\x1b[57360u\x1b]0;private title\x07");
        assert!(matches!(
            d.next(),
            Some(Message::Input(Event::Key(KeyEvent {
                code: KeyCode::Char('A'),
                ..
            })))
        ));
        assert!(d.next().is_none());
        d.feed(b"\x1b_G");
        d.feed(&vec![b'1'; 20000]);
        assert!(d.pending.len() <= 8192);
        d.feed(b"\x1b\\x");
        assert!(matches!(
            d.next(),
            Some(Message::Input(Event::Key(KeyEvent {
                code: KeyCode::Char('x'),
                ..
            })))
        ));
        assert!(d.next().is_none());
    }
    #[test]
    fn fragmented_replies_are_quarantined_and_real_keys_are_preserved() {
        let mut d = Decoder::default();
        for b in b"\x1b_Gi=42;OK\x1b\\\x1b[6;24;12t\x1b[?1;2c" {
            d.feed(&[*b]);
        }
        assert!(matches!(
            d.next(),
            Some(Message::Graphics { id: 42, ok: true })
        ));
        assert!(matches!(
            d.next(),
            Some(Message::Pixels {
                cell: true,
                width: 12,
                height: 24
            })
        ));
        assert!(d.next().is_none());
        d.feed("ї".as_bytes());
        assert!(matches!(
            d.next(),
            Some(Message::Input(Event::Key(KeyEvent {
                code: KeyCode::Char('ї'),
                ..
            })))
        ));
    }
    #[test]
    fn paste_is_opaque_and_preserves_escape_bytes_and_unicode() {
        let mut d = Decoder::default();
        for b in "\x1b[200~Ґава\x1b_Gi=42;OK\x1b\\\x1b[201~".as_bytes() {
            d.feed(&[*b]);
        }
        assert!(
            matches!(d.next(),Some(Message::Input(Event::Paste(s))) if s=="Ґава\x1b_Gi=42;OK\x1b\\")
        );
        assert!(d.next().is_none());
    }
    #[test]
    fn kitty_releases_legacy_functions_and_mouse_modifiers_survive() {
        let mut d = Decoder::default();
        d.feed(b"\x1b[57442;5u\x1b[57370;5:3u\x1b[21;2~\x1b[<20;10;4M");
        assert!(matches!(
            d.next(),
            Some(Message::Input(Event::Key(KeyEvent {
                code: KeyCode::Modifier(_),
                ..
            })))
        ));
        assert!(matches!(
            d.next(),
            Some(Message::Input(Event::Key(KeyEvent {
                code: KeyCode::F(7),
                kind: KeyEventKind::Release,
                ..
            })))
        ));
        assert!(matches!(
            d.next(),
            Some(Message::Input(Event::Key(KeyEvent {
                code: KeyCode::F(10),
                modifiers: KeyModifiers::SHIFT,
                ..
            })))
        ));
        assert!(
            matches!(d.next(),Some(Message::Input(Event::Mouse(MouseEvent{column:9,row:3,modifiers,..}))) if modifiers==KeyModifiers::SHIFT|KeyModifiers::CONTROL)
        );
    }
}
