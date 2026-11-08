//! Capability queries share the same input decoder as ordinary editing.
use crate::{
    kitty::Size,
    terminal_input::{Message, Reader},
};
use crossterm::event::Event;
use std::{
    collections::VecDeque,
    io::{self, Write},
    time::{Duration, Instant},
};
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Mode {
    #[default]
    Auto,
    Ansi,
    Kitty,
}
impl Mode {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "auto" => Ok(Self::Auto),
            "ansi" => Ok(Self::Ansi),
            "kitty" => Ok(Self::Kitty),
            _ => Err("--render must be auto, ansi or kitty".into()),
        }
    }
}
pub(crate) fn pixel_query(output: &mut impl Write) -> io::Result<()> {
    output.write_all(b"\x1b[16t\x1b[14t")?;
    output.flush()
}
pub(crate) fn cell_size(columns: u16, rows: u16) -> Option<(u16, u16)> {
    let s = crossterm::terminal::window_size().ok()?;
    let w = s.width.checked_div(columns)?;
    let h = s.height.checked_div(rows)?;
    (w > 0 && h > 0).then_some((w, h))
}
pub(crate) fn negotiate(
    reader: &mut Reader,
    output: &mut impl Write,
    mode: Mode,
    columns: u16,
    rows: u16,
) -> Result<(Option<Size>, VecDeque<Event>, bool), String> {
    let mut events = VecDeque::new();
    if mode == Mode::Ansi {
        return Ok((None, events, false));
    }
    let id = std::process::id()
        .wrapping_mul(65537)
        .wrapping_add(17)
        .max(1);
    write!(output, "\x1b_Gi={id},s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\").map_err(|e| e.to_string())?;
    pixel_query(output).map_err(|e| e.to_string())?;
    output
        .write_all(b"\x1b[c")
        .and_then(|()| output.flush())
        .map_err(|e| e.to_string())?;
    let deadline = Instant::now() + Duration::from_millis(200);
    let mut supported = false;
    let mut pixels = cell_size(columns, rows);
    let mut precise = false;
    while Instant::now() < deadline {
        match reader
            .read(Duration::from_millis(10))
            .map_err(|e| e.to_string())?
        {
            Some(Message::Graphics { id: reply, ok }) if reply == id => supported = ok,
            Some(Message::Pixels {
                cell,
                width,
                height,
            }) => {
                if cell {
                    pixels = Some((width, height));
                    precise = true;
                } else if !precise {
                    let w = width / columns;
                    let h = height / rows;
                    if w > 0 && h > 0 {
                        pixels = Some((w, h));
                    }
                }
            }
            Some(Message::Input(event)) => events.push_back(event),
            _ => {}
        }
        if supported && pixels.is_some() {
            break;
        }
    }
    if supported {
        if let Some((cell_width, cell_height)) = pixels {
            let size = Size {
                columns,
                rows,
                cell_width,
                cell_height,
            };
            if size.pixels().is_ok() {
                return Ok((Some(size), events, precise));
            }
        }
    }
    if mode == Mode::Kitty {
        Err("Kitty graphics or terminal pixel dimensions were not confirmed; use --render=ansi or a supporting terminal".into())
    } else {
        Ok((None, events, false))
    }
}
