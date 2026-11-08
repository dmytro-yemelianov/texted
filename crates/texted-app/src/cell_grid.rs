//! Backend-neutral character scene, bounded cell frames and incremental ANSI delivery.
//! Source characters survive bitmap rasterization; document bytes never become ANSI commands.
use serde::{Deserialize, Serialize};
use std::io::{self, Write};
use unicode_width::UnicodeWidthChar;
pub const CELL_GRID_VERSION: u32 = 1;
pub const MAX_CELLS: usize = 1024 * 1024;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cell {
    pub symbol: char,
    pub foreground: u32,
    pub background: u32,
}
impl Cell {
    fn blank(background: u32) -> Self {
        Self {
            symbol: ' ',
            foreground: 0xffffff,
            background,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cursor {
    pub column: usize,
    pub row: usize,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CellGrid {
    pub version: u32,
    pub columns: usize,
    pub rows: usize,
    pub cells: Vec<Cell>,
    pub cursor: Option<Cursor>,
}
impl CellGrid {
    pub fn new(columns: usize, rows: usize, background: u32) -> Result<Self, &'static str> {
        if columns == 0
            || rows == 0
            || columns
                .checked_mul(rows)
                .is_none_or(|size| size > MAX_CELLS)
        {
            return Err("Cell frame dimensions exceed the limit");
        }
        Ok(Self {
            version: CELL_GRID_VERSION,
            columns,
            rows,
            cells: vec![Cell::blank(background); columns * rows],
            cursor: None,
        })
    }
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.version != CELL_GRID_VERSION
            || self.columns == 0
            || self.rows == 0
            || self.columns.checked_mul(self.rows) != Some(self.cells.len())
            || self.cells.len() > MAX_CELLS
            || self
                .cursor
                .is_some_and(|c| c.column >= self.columns || c.row >= self.rows)
        {
            return Err("Invalid cell frame");
        }
        Ok(())
    }
    pub fn get(&self, column: usize, row: usize) -> Option<&Cell> {
        (column < self.columns && row < self.rows).then(|| &self.cells[row * self.columns + column])
    }
    fn put(&mut self, column: usize, row: usize, symbol: char, foreground: u32) {
        if column < self.columns && row < self.rows {
            let cell = &mut self.cells[row * self.columns + column];
            cell.symbol = symbol;
            cell.foreground = foreground;
        }
    }
    pub fn text(&self) -> String {
        let mut text = String::new();
        for row in self.cells.chunks(self.columns) {
            text.extend(row.iter().map(|c| c.symbol));
            text.push('\n');
        }
        text
    }
}
#[derive(Clone, Debug)]
pub enum CellCommand {
    Fill {
        rect: [f32; 4],
        bounds: [f32; 2],
        color: u32,
    },
    Glyph {
        at: [f32; 2],
        bounds: [f32; 2],
        symbol: char,
        color: u32,
    },
    Border {
        rect: [f32; 4],
        bounds: [f32; 2],
        color: u32,
    },
    HideCursor,
    Cursor {
        at: [f32; 2],
    },
}
impl CellCommand {
    pub fn translate(&mut self, x: f32, y: f32) {
        match self {
            Self::Fill { rect, bounds, .. } | Self::Border { rect, bounds, .. } => {
                rect[0] += x;
                rect[1] += y;
                bounds[0] += x;
                bounds[1] += y;
            }
            Self::Glyph { at, bounds, .. } => {
                at[0] += x;
                at[1] += y;
                bounds[0] += x;
                bounds[1] += y;
            }
            Self::HideCursor => {}
            Self::Cursor { at } => {
                at[0] += x;
                at[1] += y;
            }
        }
    }
}
pub fn compose(
    columns: usize,
    rows: usize,
    cell_size: [f32; 2],
    clear: u32,
    commands: &[CellCommand],
) -> Result<CellGrid, &'static str> {
    let [cw, ch] = cell_size;
    if !cw.is_finite() || !ch.is_finite() || cw <= 0.0 || ch <= 0.0 {
        return Err("Invalid cell metrics");
    }
    let mut grid = CellGrid::new(columns, rows, clear)?;
    for command in commands {
        match *command {
            CellCommand::Fill {
                rect: [x, y, w, h],
                bounds,
                color,
            } => {
                if w < cw || h < ch {
                    continue;
                }
                // Thin native decoration does not replace an entire terminal character.
                let start_x = ((x / cw) - 0.5).ceil().max(0.0) as usize;
                let start_y = ((y / ch) - 0.5).ceil().max(0.0) as usize;
                let end_x = (((x + w).min(bounds[0]) / cw) - 0.5).ceil().max(0.0) as usize;
                let end_y = (((y + h).min(bounds[1]) / ch) - 0.5).ceil().max(0.0) as usize;
                for row in start_y.min(rows)..end_y.min(rows) {
                    for col in start_x.min(columns)..end_x.min(columns) {
                        grid.cells[row * columns + col] = Cell::blank(color);
                    }
                }
            }
            CellCommand::Glyph {
                at: [x, y],
                bounds,
                symbol,
                color,
            } => {
                if x >= 0.0 && y >= 0.0 && x < bounds[0] && y < bounds[1] {
                    grid.put(
                        (x / cw).floor() as usize,
                        (y / ch).floor() as usize,
                        symbol,
                        color,
                    );
                }
            }
            CellCommand::Border {
                rect: [x, y, w, h],
                bounds,
                color,
            } => {
                if x < 0.0 || y < 0.0 || w < cw * 2.0 || h < ch * 3.0 {
                    continue;
                }
                let left = (x / cw).floor() as usize;
                let top = (y / ch).floor() as usize;
                let right = (((x + w).min(bounds[0]) / cw).ceil() as usize)
                    .saturating_sub(1)
                    .min(columns - 1);
                let bottom = (((y + h).min(bounds[1]) / ch).ceil() as usize)
                    .saturating_sub(1)
                    .min(rows - 1);
                if left >= right || top >= bottom {
                    continue;
                }
                for col in left + 1..right {
                    grid.put(col, top, '─', color);
                    grid.put(col, bottom, '─', color);
                }
                for row in top + 1..bottom {
                    grid.put(left, row, '│', color);
                    grid.put(right, row, '│', color);
                }
                for (col, row, symbol) in [
                    (left, top, '┌'),
                    (right, top, '┐'),
                    (left, bottom, '└'),
                    (right, bottom, '┘'),
                ] {
                    grid.put(col, row, symbol, color);
                }
            }
            CellCommand::HideCursor => grid.cursor = None,
            CellCommand::Cursor { at: [x, y] } => {
                if x >= 0.0 && y >= 0.0 && (x / cw) < columns as f32 && (y / ch) < rows as f32 {
                    grid.cursor = Some(Cursor {
                        column: (x / cw).floor() as usize,
                        row: (y / ch).floor() as usize,
                    });
                }
            }
        }
    }
    Ok(grid)
}
/// This editor's native bitmap layout gives each scalar one cell. Keep that geometry
/// on terminals too: non-single-width scalars use a visible replacement, without
/// changing the source text or allowing terminal control sequences through.
fn terminal_symbol(symbol: char) -> char {
    if !symbol.is_control() && symbol.width() == Some(1) {
        symbol
    } else {
        '�'
    }
}
#[derive(Default)]
pub struct AnsiWriter {
    previous: Option<CellGrid>,
}
impl AnsiWriter {
    pub fn invalidate(&mut self) {
        self.previous = None;
    }
    pub fn write_frame(&mut self, writer: &mut impl Write, grid: &CellGrid) -> io::Result<usize> {
        grid.validate()
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
        let same_size = self
            .previous
            .as_ref()
            .is_some_and(|p| p.columns == grid.columns && p.rows == grid.rows);
        let mut bytes = Vec::new();
        let mut style = None;
        if !same_size {
            bytes.extend_from_slice(b"\x1b[2J");
        }
        for row in 0..grid.rows {
            let mut positioned = false;
            for col in 0..grid.columns {
                let index = row * grid.columns + col;
                let cell = grid.cells[index];
                let changed = !same_size || self.previous.as_ref().unwrap().cells[index] != cell;
                if !changed {
                    positioned = false;
                    continue;
                }
                if !positioned {
                    write!(&mut bytes, "\x1b[{};{}H", row + 1, col + 1)?;
                    positioned = true;
                }
                let colors = (cell.foreground & 0xffffff, cell.background & 0xffffff);
                if style != Some(colors) {
                    write!(
                        &mut bytes,
                        "\x1b[38;2;{};{};{}m\x1b[48;2;{};{};{}m",
                        (colors.0 >> 16) & 255,
                        (colors.0 >> 8) & 255,
                        colors.0 & 255,
                        (colors.1 >> 16) & 255,
                        (colors.1 >> 8) & 255,
                        colors.1 & 255
                    )?;
                    style = Some(colors);
                }
                let mut encoded = [0u8; 4];
                bytes.extend_from_slice(
                    terminal_symbol(cell.symbol)
                        .encode_utf8(&mut encoded)
                        .as_bytes(),
                );
            }
        }
        if !same_size
            || self
                .previous
                .as_ref()
                .is_none_or(|p| p.cursor != grid.cursor)
            || !bytes.is_empty()
        {
            match grid.cursor {
                Some(cursor) if cursor.column < grid.columns && cursor.row < grid.rows => write!(
                    &mut bytes,
                    "\x1b[{};{}H\x1b[?25h",
                    cursor.row + 1,
                    cursor.column + 1
                )?,
                _ => bytes.extend_from_slice(b"\x1b[?25l"),
            }
        }
        if let Err(error) = writer.write_all(&bytes).and_then(|()| writer.flush()) {
            self.previous = None;
            return Err(error);
        }
        self.previous = Some(grid.clone());
        Ok(bytes.len())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn thin_pixel_caret_does_not_erase_cells_and_overlays_own_the_cursor() {
        let grid = compose(
            4,
            3,
            [8., 16.],
            0,
            &[
                CellCommand::Glyph {
                    at: [0., 0.],
                    bounds: [32., 48.],
                    symbol: 'Ї',
                    color: 0xffffff,
                },
                CellCommand::Fill {
                    rect: [4., 0., 2., 16.],
                    bounds: [32., 48.],
                    color: 0xffffff,
                },
                CellCommand::Cursor { at: [0., 0.] },
                CellCommand::HideCursor,
            ],
        )
        .unwrap();
        assert_eq!(grid.get(0, 0).unwrap().symbol, 'Ї');
        assert_eq!(grid.cursor, None);
        let json = serde_json::to_vec(&grid).unwrap();
        let mut restored: CellGrid = serde_json::from_slice(&json).unwrap();
        assert_eq!(restored, grid);
        restored.version += 1;
        assert!(restored.validate().is_err());
    }
    #[test]
    fn bounds_and_metrics_are_checked() {
        assert!(CellGrid::new(0, 1, 0).is_err());
        assert!(CellGrid::new(MAX_CELLS, 2, 0).is_err());
        assert!(compose(1, 1, [0., 16.], 0, &[]).is_err());
    }
    #[test]
    fn source_symbols_styles_clipping_and_translation_survive_rasterization() {
        let mut commands = vec![
            CellCommand::Fill {
                rect: [0., 0., 24., 16.],
                bounds: [24., 16.],
                color: 0x123456,
            },
            CellCommand::Glyph {
                at: [8., 0.],
                bounds: [24., 16.],
                symbol: 'Ї',
                color: 0xabcdef,
            },
            CellCommand::Cursor { at: [8., 0.] },
        ];
        for c in &mut commands {
            c.translate(0., 16.);
        }
        let grid = compose(3, 2, [8., 16.], 0, &commands).unwrap();
        let cell = grid.get(1, 1).unwrap();
        assert_eq!(cell.symbol, 'Ї');
        assert_eq!(cell.background, 0x123456);
        assert_eq!(cell.foreground, 0xabcdef);
        assert_eq!(grid.cursor, Some(Cursor { column: 1, row: 1 }));
    }
    #[test]
    fn modal_fill_erases_covered_text_and_border_is_semantic() {
        let commands = [
            CellCommand::Glyph {
                at: [8., 16.],
                bounds: [40., 64.],
                symbol: 'X',
                color: 1,
            },
            CellCommand::Fill {
                rect: [0., 0., 40., 64.],
                bounds: [40., 64.],
                color: 7,
            },
            CellCommand::Border {
                rect: [0., 0., 40., 64.],
                bounds: [40., 64.],
                color: 9,
            },
        ];
        let grid = compose(5, 4, [8., 16.], 0, &commands).unwrap();
        assert_eq!(grid.get(1, 1).unwrap().symbol, ' ');
        assert_eq!(grid.get(0, 0).unwrap().symbol, '┌');
        assert_eq!(grid.get(4, 3).unwrap().symbol, '┘');
    }
    #[test]
    fn ansi_is_incremental_and_document_controls_cannot_escape_the_grid() {
        let mut grid = CellGrid::new(3, 1, 0).unwrap();
        grid.put(0, 0, 'Ї', 0xffffff);
        grid.put(1, 0, '\x1b', 0xffffff);
        grid.put(2, 0, '界', 0xffffff);
        let mut ansi = AnsiWriter::default();
        let mut bytes = Vec::new();
        assert!(ansi.write_frame(&mut bytes, &grid).unwrap() > 0);
        assert!(String::from_utf8(bytes).unwrap().contains("Ї��"));
        let mut bytes = Vec::new();
        assert_eq!(ansi.write_frame(&mut bytes, &grid).unwrap(), 0);
        grid.put(1, 0, 'Ґ', 1);
        ansi.write_frame(&mut bytes, &grid).unwrap();
        let output = String::from_utf8(bytes).unwrap();
        assert!(output.starts_with("\x1b[1;2H"));
        assert!(!output.contains('Ї'));
        assert!(output.contains('Ґ'));
    }
}
