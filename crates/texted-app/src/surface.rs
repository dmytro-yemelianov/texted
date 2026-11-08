//! Presentation adapters; the application controller never owns a second terminal UI.
use crate::{gpu, graphics::DrawList};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use texted_app::{
    cell_grid::{AnsiWriter, CellGrid},
    framebuffer::ScreenMapping,
};
use winit::{
    dpi::PhysicalSize,
    window::{CursorIcon, Window},
};
pub(crate) enum Host {
    Native(Arc<Window>),
    Cells {
        size: Mutex<PhysicalSize<u32>>,
        redraw: Arc<AtomicBool>,
        title: Mutex<String>,
    },
}
impl Host {
    pub fn native(window: Arc<Window>) -> Arc<Self> {
        Arc::new(Self::Native(window))
    }
    pub fn cells(columns: u16, rows: u16, redraw: Arc<AtomicBool>) -> Arc<Self> {
        Arc::new(Self::Cells {
            size: Mutex::new(PhysicalSize::new(
                u32::from(columns) * 8,
                u32::from(rows) * 16,
            )),
            redraw,
            title: Mutex::new(String::new()),
        })
    }
    pub fn is_cells(&self) -> bool {
        matches!(self, Self::Cells { .. })
    }
    pub fn native_window(&self) -> Option<&Window> {
        match self {
            Self::Native(w) => Some(w),
            _ => None,
        }
    }
    pub fn inner_size(&self) -> PhysicalSize<u32> {
        match self {
            Self::Native(w) => w.inner_size(),
            Self::Cells { size, .. } => *size.lock().unwrap(),
        }
    }
    pub fn resize_cells(&self, columns: u16, rows: u16) {
        if let Self::Cells { size, redraw, .. } = self {
            *size.lock().unwrap() =
                PhysicalSize::new(u32::from(columns.max(1)) * 8, u32::from(rows.max(1)) * 16);
            redraw.store(true, Ordering::Release);
        }
    }
    pub fn request_redraw(&self) {
        match self {
            Self::Native(w) => w.request_redraw(),
            Self::Cells { redraw, .. } => redraw.store(true, Ordering::Release),
        }
    }
    pub fn take_redraw(&self) -> bool {
        match self {
            Self::Cells { redraw, .. } => redraw.swap(false, Ordering::AcqRel),
            _ => false,
        }
    }
    pub fn set_title(&self, title: &str) {
        match self {
            Self::Native(w) => w.set_title(title),
            Self::Cells { title: current, .. } => {
                let mut current = current.lock().unwrap();
                if *current != title {
                    *current = title.into();
                }
            }
        }
    }
    pub fn title(&self) -> String {
        match self {
            Self::Cells { title, .. } => title.lock().unwrap().clone(),
            Self::Native(_) => String::new(),
        }
    }
    pub fn set_visible(&self, value: bool) {
        if let Self::Native(w) = self {
            w.set_visible(value);
        }
    }
    pub fn set_minimized(&self, value: bool) {
        if let Self::Native(w) = self {
            w.set_minimized(value);
        }
    }
    pub fn focus_window(&self) {
        if let Self::Native(w) = self {
            w.focus_window();
        }
    }
    pub fn set_cursor_visible(&self, value: bool) {
        if let Self::Native(w) = self {
            w.set_cursor_visible(value);
        }
    }
    pub fn set_cursor_icon(&self, value: CursorIcon) {
        if let Self::Native(w) = self {
            w.set_cursor_icon(value);
        }
    }
}
pub(crate) enum Renderer {
    Native(Box<gpu::Renderer>),
    Cells {
        ansi: AnsiWriter,
        output: Option<Box<dyn std::io::Write + Send>>,
        frame: Option<CellGrid>,
    },
}
impl Renderer {
    pub fn cells(output: Option<Box<dyn std::io::Write + Send>>) -> Self {
        Self::Cells {
            ansi: AnsiWriter::default(),
            output,
            frame: None,
        }
    }
    pub fn invalidate(&mut self) {
        if let Self::Cells { ansi, .. } = self {
            ansi.invalidate();
        }
    }
    pub fn description(&self) -> &str {
        match self {
            Self::Native(gpu) => &gpu.description,
            Self::Cells { .. } => "ANSI cell grid",
        }
    }
    pub fn frame(&self) -> Option<&CellGrid> {
        match self {
            Self::Cells { frame, .. } => frame.as_ref(),
            _ => None,
        }
    }
    pub fn render(
        &mut self,
        draws: &DrawList,
        width: u32,
        height: u32,
        mapping: ScreenMapping,
    ) -> Result<bool, String> {
        match self {
            Self::Native(gpu) => gpu.render(draws, width, height, mapping),
            Self::Cells {
                ansi,
                output,
                frame,
            } => {
                let grid = draws
                    .cell_grid(
                        (width / 8).max(1) as usize,
                        (height / 16).max(1) as usize,
                        [8., 16.],
                    )
                    .map_err(str::to_owned)?;
                if let Some(output) = output {
                    ansi.write_frame(output, &grid).map_err(|e| e.to_string())?;
                }
                *frame = Some(grid);
                Ok(true)
            }
        }
    }
}
