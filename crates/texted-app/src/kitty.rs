//! Bounded Kitty graphics delivery: compressed cell-aligned tiles, no shared files.
use base64::{engine::general_purpose::STANDARD, Engine};
use flate2::{write::ZlibEncoder, Compression};
use std::{
    collections::BTreeSet,
    io::{self, Write},
    sync::{Arc, Mutex},
};
pub(crate) type Registry = Arc<Mutex<BTreeSet<u32>>>;
const TILE_COLUMNS: u32 = 8;
const TILE_ROWS: u32 = 4;
const MAX_TILES: u32 = 32768;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Size {
    pub columns: u16,
    pub rows: u16,
    pub cell_width: u16,
    pub cell_height: u16,
}
impl Size {
    pub fn pixels(self) -> Result<(u32, u32), String> {
        let w = u32::from(self.columns) * u32::from(self.cell_width);
        let h = u32::from(self.rows) * u32::from(self.cell_height);
        if self.columns == 0
            || self.rows == 0
            || self.cell_width == 0
            || self.cell_height == 0
            || (w as usize)
                .checked_mul(h as usize)
                .is_none_or(|n| n > crate::raster::MAX_PIXELS)
            || u32::from(self.columns).div_ceil(TILE_COLUMNS)
                * u32::from(self.rows).div_ceil(TILE_ROWS)
                > MAX_TILES
        {
            Err("Invalid terminal pixel dimensions".into())
        } else {
            Ok((w, h))
        }
    }
    pub fn update_report(
        &mut self,
        precise: &mut bool,
        cell: bool,
        width: u16,
        height: u16,
    ) -> bool {
        if !cell && *precise {
            return false;
        }
        *precise |= cell;
        let (w, h) = if cell {
            (width, height)
        } else {
            (width / self.columns.max(1), height / self.rows.max(1))
        };
        if w == 0 || h == 0 || (w, h) == (self.cell_width, self.cell_height) {
            return false;
        }
        self.cell_width = w;
        self.cell_height = h;
        true
    }
    pub fn pointer(self, column: u16, row: u16) -> (f32, f32) {
        (
            (f32::from(column) + 0.5) * f32::from(self.cell_width),
            (f32::from(row) + 0.5) * f32::from(self.cell_height),
        )
    }
}
pub(crate) fn cleanup(output: &mut impl Write, registry: &Registry) -> io::Result<()> {
    let ids = std::mem::take(&mut *registry.lock().unwrap_or_else(|p| p.into_inner()));
    for id in ids {
        write!(output, "\x1b_Ga=d,d=I,i={id},q=2\x1b\\")?;
    }
    output.flush()
}
pub(crate) struct Writer {
    base: u32,
    previous: Vec<u8>,
    size: Option<Size>,
    registry: Registry,
}
impl Writer {
    pub fn new(registry: Registry) -> Self {
        use std::time::{SystemTime, UNIX_EPOCH};
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos()
            ^ std::process::id().rotate_left(13);
        Self {
            base: seed.wrapping_mul(2654435761) % (u32::MAX - MAX_TILES) + 1,
            previous: Vec::new(),
            size: None,
            registry,
        }
    }
    pub fn suspend(&mut self, output: &mut impl Write) -> io::Result<()> {
        self.previous.clear();
        self.size = None;
        cleanup(output, &self.registry)
    }
    pub fn invalidate(&mut self) {
        self.size = None;
    }
    pub fn write_frame(
        &mut self,
        output: &mut impl Write,
        size: Size,
        pixels: &[u8],
    ) -> io::Result<usize> {
        let (width, height) = size
            .pixels()
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
        if pixels.len() != width as usize * height as usize * 3 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Invalid RGB framebuffer",
            ));
        }
        let same = self.size == Some(size) && self.previous.len() == pixels.len();
        let mut changed = Vec::new();
        let tx = u32::from(size.columns).div_ceil(TILE_COLUMNS);
        let ty = u32::from(size.rows).div_ceil(TILE_ROWS);
        for row in 0..ty {
            for col in 0..tx {
                let x = col * TILE_COLUMNS * u32::from(size.cell_width);
                let y = row * TILE_ROWS * u32::from(size.cell_height);
                let w = (TILE_COLUMNS * u32::from(size.cell_width)).min(width - x);
                let h = (TILE_ROWS * u32::from(size.cell_height)).min(height - y);
                if !same
                    || (y..y + h).any(|py| {
                        let start = ((py * width + x) * 3) as usize;
                        pixels[start..start + w as usize * 3]
                            != self.previous[start..start + w as usize * 3]
                    })
                {
                    changed.push((row, col, x, y, w, h));
                }
            }
        }
        if changed.is_empty() {
            return Ok(0);
        }
        let result = (|| {
            output.write_all(b"\x1b[?2026h\x1b[?25l")?;
            if !same {
                cleanup(output, &self.registry)?;
                output.write_all(b"\x1b[2J")?;
            }
            let mut total = 0;
            for (row, col, x, y, w, h) in changed {
                let id = self.base + row * tx + col;
                {
                    let mut ids = self.registry.lock().unwrap_or_else(|p| p.into_inner());
                    ids.insert(id);
                }
                let mut z = ZlibEncoder::new(Vec::new(), Compression::fast());
                for py in y..y + h {
                    let start = ((py * width + x) * 3) as usize;
                    z.write_all(&pixels[start..start + w as usize * 3])?;
                }
                let data = STANDARD.encode(z.finish()?);
                write!(
                    output,
                    "\x1b[{};{}H\x1b_Ga=d,d=I,i={id},q=2\x1b\\",
                    row * TILE_ROWS + 1,
                    col * TILE_COLUMNS + 1
                )?;
                let cols = TILE_COLUMNS.min(u32::from(size.columns) - col * TILE_COLUMNS);
                let rows = TILE_ROWS.min(u32::from(size.rows) - row * TILE_ROWS);
                for (index, chunk) in data.as_bytes().chunks(4096).enumerate() {
                    let more = u8::from((index + 1) * 4096 < data.len());
                    if index == 0 {
                        write!(output,"\x1b_Ga=T,t=d,f=24,o=z,s={w},v={h},i={id},p=1,c={cols},r={rows},C=1,z=1,q=2,m={more};")?;
                    } else {
                        write!(output, "\x1b_Gm={more},q=2;")?;
                    }
                    output.write_all(chunk)?;
                    output.write_all(b"\x1b\\")?;
                    total += chunk.len();
                }
            }
            output.write_all(b"\x1b[?2026l")?;
            output.flush()?;
            Ok(total)
        })();
        if result.is_ok() {
            self.previous.clear();
            self.previous.extend_from_slice(pixels);
            self.size = Some(size);
        } else {
            self.size = None;
            let _ = output.write_all(b"\x1b[?2026l");
        }
        result
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "CPU and transfer profile; run explicitly"]
    fn profile_terminal_frames() {
        use crate::{
            framebuffer::ScreenMapping,
            graphics::{draw_rect, DrawList, FontEngine},
            raster::Rasterizer,
        };
        use std::time::Instant;
        for (columns, rows) in [(100, 30), (240, 67)] {
            let size = Size {
                columns,
                rows,
                cell_width: 8,
                cell_height: 16,
            };
            let (width, height) = size.pixels().unwrap();
            let mut draws = DrawList::default();
            draws.reset(0x1e1e1e);
            let mut font = FontEngine::new(16.);
            for row in 0..rows {
                font.draw_string(
                    &mut draws,
                    width as usize,
                    height as usize,
                    0,
                    usize::from(row) * 16,
                    "Ґава, їжак і єнот — bitmap fonts, unchanged terminal settings. 0123456789",
                    0xd4d4d4,
                );
            }
            draw_rect(
                &mut draws,
                width as usize,
                height as usize,
                0,
                0,
                2,
                16,
                0xffffff,
            );
            let mut raster = Rasterizer::default();
            let mut writer = Writer::new(Registry::default());
            let mapping = ScreenMapping::new((width, height), None);
            let mut output = Vec::new();
            let first = writer
                .write_frame(
                    &mut output,
                    size,
                    raster.render(&draws, width, height, mapping).unwrap(),
                )
                .unwrap();
            let start = Instant::now();
            let mut transfer = 0;
            for _ in 0..100 {
                output.clear();
                draws.draws.last_mut().unwrap().color ^= 0xffffff;
                transfer += writer
                    .write_frame(
                        &mut output,
                        size,
                        raster.render(&draws, width, height, mapping).unwrap(),
                    )
                    .unwrap();
            }
            println!("{width}x{height}: {:?}/frame CPU; initial {first} base64 bytes; caret update {} bytes/frame",start.elapsed()/100,transfer/100);
        }
    }
    #[test]
    fn precise_cell_pixels_win_over_window_margins_and_mouse_uses_the_same_geometry() {
        let mut s = Size {
            columns: 100,
            rows: 30,
            cell_width: 8,
            cell_height: 16,
        };
        let mut precise = false;
        assert!(s.update_report(&mut precise, true, 8, 17));
        assert!(!s.update_report(&mut precise, false, 800, 546));
        assert_eq!(s.pixels().unwrap(), (800, 510));
        assert_eq!(s.pointer(37, 11), (300., 195.5));
    }
    #[test]
    fn unchanged_frames_send_nothing_one_pixel_updates_one_tile_and_resize_reclaims_ids() {
        let registry = Registry::default();
        let mut w = Writer::new(registry.clone());
        let size = Size {
            columns: 16,
            rows: 8,
            cell_width: 8,
            cell_height: 16,
        };
        let mut p = vec![0; 128 * 128 * 3];
        let mut out = Vec::new();
        assert!(w.write_frame(&mut out, size, &p).unwrap() > 0);
        assert_eq!(registry.lock().unwrap().len(), 4);
        out.clear();
        assert_eq!(w.write_frame(&mut out, size, &p).unwrap(), 0);
        assert!(out.is_empty());
        p[0] = 255;
        w.write_frame(&mut out, size, &p).unwrap();
        assert_eq!(String::from_utf8_lossy(&out).matches("a=T").count(), 1);
        out.clear();
        let small = Size {
            columns: 8,
            rows: 4,
            ..size
        };
        w.write_frame(&mut out, small, &p[..64 * 64 * 3]).unwrap();
        assert_eq!(registry.lock().unwrap().len(), 1);
        w.suspend(&mut out).unwrap();
        assert!(registry.lock().unwrap().is_empty());
    }
    #[test]
    fn chunks_are_bounded_and_decode_to_exact_rgb() {
        use std::io::Read;
        let mut w = Writer::new(Registry::default());
        let size = Size {
            columns: 8,
            rows: 4,
            cell_width: 8,
            cell_height: 16,
        };
        let p: Vec<_> = (0..64 * 64 * 3)
            .map(|i| ((i * 97 + i / 7) % 256) as u8)
            .collect();
        let mut out = Vec::new();
        w.write_frame(&mut out, size, &p).unwrap();
        let text = String::from_utf8(out).unwrap();
        let mut encoded = String::new();
        for sequence in text.split("\x1b_G").skip(1) {
            let sequence = sequence.split("\x1b\\").next().unwrap();
            if let Some((_, data)) = sequence.split_once(';') {
                assert!(data.len() <= 4096);
                encoded.push_str(data);
            }
        }
        let compressed = STANDARD.decode(encoded).unwrap();
        let mut raw = Vec::new();
        flate2::read::ZlibDecoder::new(compressed.as_slice())
            .read_to_end(&mut raw)
            .unwrap();
        assert_eq!(raw, p);
    }
}
