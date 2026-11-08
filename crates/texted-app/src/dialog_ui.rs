//! One cell-aligned dialog implementation for both window and terminal backends.
//! File work runs off-thread; tickets feed the existing save/close state machine.
use crate::{
    appearance_ui::Rect,
    graphics::{draw_box, draw_rect, DrawList, FontEngine},
    Wake,
};
use std::{
    path::PathBuf,
    sync::{mpsc, Arc},
    thread,
    time::{Duration, Instant},
};
use texted_app::{
    appearance::Palette,
    buffer_service::EditContext,
    clipboard::{Clipboard, ClipboardAction, ClipboardPayload, SystemClipboard},
    commands::EditorCommand,
    editing::EditorModel,
    input::{self, Intent, KeyInput, SearchContext},
};
use winit::keyboard::{Key, NamedKey};
#[derive(Clone, Debug)]
pub(crate) enum Kind {
    Open { directory: PathBuf },
    Save { path: PathBuf },
    ConfirmSave { name: String },
    Error { message: String },
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Answer {
    Path(PathBuf),
    Save,
    Discard,
    Cancel,
    Ok,
}
struct Request {
    kind: Kind,
    reply: mpsc::SyncSender<Answer>,
}
pub(crate) struct Ticket(mpsc::Receiver<Answer>);
impl Ticket {
    pub fn poll(&self) -> Option<Answer> {
        match self.0.try_recv() {
            Ok(a) => Some(a),
            Err(mpsc::TryRecvError::Disconnected) => Some(Answer::Cancel),
            Err(mpsc::TryRecvError::Empty) => None,
        }
    }
}
#[derive(Clone)]
pub(crate) struct Client {
    send: mpsc::SyncSender<Request>,
    wake: Wake,
}
impl Client {
    pub fn request(&self, kind: Kind) -> Result<Ticket, String> {
        let (reply, receive) = mpsc::sync_channel(1);
        self.send
            .try_send(Request { kind, reply })
            .map_err(|_| "Dialog queue is unavailable".to_owned())?;
        (self.wake)();
        Ok(Ticket(receive))
    }
}
#[derive(Clone)]
struct Entry {
    path: PathBuf,
    name: String,
    directory: bool,
}
enum PathResult {
    Directory(PathBuf, Vec<Entry>, bool),
    File(PathBuf, bool),
}
type Inspection = (
    mpsc::Receiver<Result<PathResult, String>>,
    bool,
    EditContext,
);
struct Active {
    request: Request,
    field: EditorModel,
    focus: usize,
    selected: usize,
    first: usize,
    entries: Vec<Entry>,
    error: String,
    checking: Option<Inspection>,
    replace: Option<PathBuf>,
    serial: u64,
    position: Option<(usize, usize)>,
}
struct Geometry {
    rect: Rect,
    field: Rect,
    list: Rect,
    buttons: Vec<Rect>,
    close: Rect,
    cw: usize,
    ch: usize,
}
impl Geometry {
    fn new(
        size: (usize, usize),
        compact: bool,
        path: bool,
        buttons: usize,
        position: Option<(usize, usize)>,
    ) -> Self {
        let (cw, ch) = (8, if compact { 8 } else { 16 });
        let cols = (size.0 / cw).saturating_sub(4).clamp(16, 90);
        let rows = if path {
            (size.1 / ch).saturating_sub(4).clamp(11, 22)
        } else {
            10
        };
        let w = (cols * cw).min(size.0);
        let h = (rows * ch).min(size.1);
        let (x, y) = position.unwrap_or(((size.0 - w) / 2 / cw * cw, (size.1 - h) / 2 / ch * ch));
        let rect = Rect {
            x: x.min(size.0 - w),
            y: y.min(size.1 - h),
            w,
            h,
        };
        let field = Rect {
            x: rect.x + 2 * cw,
            y: rect.y + 3 * ch,
            w: w.saturating_sub(4 * cw),
            h: 3 * ch,
        };
        let list = Rect {
            x: rect.x + 2 * cw,
            y: rect.y + 7 * ch,
            w: w.saturating_sub(4 * cw),
            h: h.saturating_sub(10 * ch),
        };
        let bw = (12 * cw).min(w.saturating_sub(4 * cw) / buttons.max(1));
        let button_y = rect.y + h.saturating_sub(3 * ch);
        let buttons = (0..buttons)
            .map(|i| Rect {
                x: rect.x + 2 * cw + i * bw,
                y: button_y,
                w: bw.saturating_sub(cw),
                h: ch,
            })
            .collect();
        let close = Rect {
            x: rect.x + w.saturating_sub(3 * cw),
            y: rect.y + ch,
            w: 2 * cw,
            h: ch,
        };
        Self {
            rect,
            field,
            list,
            buttons,
            close,
            cw,
            ch,
        }
    }
}
pub(crate) struct DialogUi {
    receive: mpsc::Receiver<Request>,
    active: Option<Active>,
    wake: Wake,
    serial: u64,
    clipboard: Clipboard,
    hover: Option<usize>,
    drag: Option<(f32, f32, usize, usize)>,
    last_click: Option<(usize, Instant)>,
}
pub(crate) fn channel(wake: Wake) -> (Client, DialogUi) {
    let (send, receive) = mpsc::sync_channel(4);
    let signal = wake.clone();
    let clipboard = Clipboard::new(Arc::new(SystemClipboard), move || signal());
    (
        Client {
            send,
            wake: wake.clone(),
        },
        DialogUi {
            receive,
            active: None,
            wake,
            serial: 0,
            clipboard,
            hover: None,
            drag: None,
            last_click: None,
        },
    )
}
impl DialogUi {
    pub fn open(&self) -> bool {
        self.active.is_some()
    }
    pub fn command_label(&self) -> &'static str {
        let labels = self.labels();
        let focus = self.active.as_ref().map_or(0, |a| {
            if self.path_mode() {
                a.focus.saturating_sub(2)
            } else {
                a.focus
            }
        });
        labels.get(focus).copied().unwrap_or("OK")
    }
    pub fn title(&self) -> &'static str {
        match self.active.as_ref().map(|a| &a.request.kind) {
            Some(Kind::Open { .. }) => "Open file",
            Some(Kind::Save { .. }) => "Save file",
            Some(Kind::ConfirmSave { .. }) => "Save changes?",
            Some(Kind::Error { .. }) => "Texted",
            None => "",
        }
    }
    fn path_mode(&self) -> bool {
        self.active.as_ref().is_some_and(|a| {
            matches!(a.request.kind, Kind::Open { .. } | Kind::Save { .. }) && a.replace.is_none()
        })
    }
    fn labels(&self) -> Vec<&'static str> {
        let Some(a) = &self.active else {
            return vec![];
        };
        if a.replace.is_some() {
            return vec!["Replace", "Back"];
        }
        match a.request.kind {
            Kind::ConfirmSave { .. } => vec!["Save", "Discard", "Cancel"],
            Kind::Error { .. } => vec!["OK"],
            Kind::Open { .. } => vec!["Open", "Cancel"],
            Kind::Save { .. } => vec!["Save", "Cancel"],
        }
    }
    fn geometry(&self, size: (usize, usize), compact: bool) -> Geometry {
        Geometry::new(
            size,
            compact,
            self.path_mode(),
            self.labels().len(),
            self.active.as_ref().and_then(|a| a.position),
        )
    }
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        if self.active.is_none() {
            if let Ok(request) = self.receive.try_recv() {
                self.serial += 1;
                let initial = match &request.kind {
                    Kind::Open { directory } => format!("{}/", directory.display()),
                    Kind::Save { path } => path.to_string_lossy().into_owned(),
                    _ => String::new(),
                };
                let directory = match &request.kind {
                    Kind::Open { directory } => Some(directory.clone()),
                    Kind::Save { path } => Some(
                        path.parent()
                            .unwrap_or(std::path::Path::new("."))
                            .to_owned(),
                    ),
                    _ => None,
                };
                let mut field = EditorModel::new(ropey::Rope::from_str(&initial));
                field.buffer.set_cursor_offset(initial.len());
                self.active = Some(Active {
                    request,
                    field,
                    focus: 0,
                    selected: 0,
                    first: 0,
                    entries: vec![],
                    error: String::new(),
                    checking: None,
                    replace: None,
                    serial: self.serial,
                    position: None,
                });
                if let Some(directory) = directory {
                    self.check(directory, false, false);
                }
                changed = true;
            }
        }
        if let Some(a) = &mut self.active {
            if let Some((receive, replace_field, context)) = &a.checking {
                let changed_field = *context != field_context(a);
                match receive.try_recv() {
                    Ok(result) => {
                        let replace_field = *replace_field;
                        a.checking = None;
                        if replace_field && changed_field {
                            a.error = "Path changed; press Enter to use the current path".into();
                        } else {
                            match result {
                                Ok(PathResult::Directory(path, entries, truncated)) => {
                                    a.entries = entries;
                                    a.selected = 0;
                                    a.first = 0;
                                    if replace_field {
                                        set_field(a, format!("{}/", path.display()));
                                    }
                                    a.error = if truncated {
                                        "More entries: type the full path to reach them".into()
                                    } else {
                                        String::new()
                                    };
                                }
                                Ok(PathResult::File(path, exists)) => {
                                    if matches!(a.request.kind, Kind::Save { .. }) && exists {
                                        a.replace = Some(path);
                                        a.focus = 0;
                                    } else {
                                        self.finish(Answer::Path(path));
                                    }
                                }
                                Err(error) => a.error = error,
                            }
                        }
                        changed = true;
                    }
                    Err(mpsc::TryRecvError::Disconnected) => {
                        a.checking = None;
                        a.error = "File inspection stopped".into();
                        changed = true;
                    }
                    Err(mpsc::TryRecvError::Empty) => {}
                }
            }
        }
        if let Some(done) = self.clipboard.poll() {
            if let Some(a) = &mut self.active {
                let (ClipboardAction::Paste { context, .. }
                | ClipboardAction::Copy { context, .. }) = done.action;
                {
                    if context == field_context(a) {
                        match done.result {
                            Ok(ClipboardPayload::Pasted(text)) => self.insert(&text),
                            Ok(ClipboardPayload::Copied { cut: true }) => {
                                let _ = a.field.apply(EditorCommand::Delete {
                                    backward: true,
                                    unit: texted_app::commands::DeleteUnit::Character,
                                });
                            }
                            Err(error) => a.error = error,
                            _ => {}
                        }
                    }
                }
                changed = true;
            }
        }
        changed
    }
    fn finish(&mut self, answer: Answer) {
        if let Some(a) = self.active.take() {
            let _ = a.request.reply.try_send(answer);
        }
        self.drag = None;
        self.hover = None;
        self.clipboard.cancel();
        (self.wake)();
    }
    pub fn cancel(&mut self) {
        if let Some(a) = &mut self.active {
            if a.replace.take().is_some() {
                a.focus = 0;
                return;
            }
        }
        self.finish(Answer::Cancel);
    }
    pub fn navigate(&mut self, forward: bool) {
        if let Some(a) = &mut self.active {
            if matches!(a.request.kind, Kind::Open { .. } | Kind::Save { .. })
                && a.replace.is_none()
            {
                a.focus = 1;
                if !a.entries.is_empty() {
                    a.selected = if forward {
                        (a.selected + 1) % a.entries.len()
                    } else {
                        (a.selected + a.entries.len() - 1) % a.entries.len()
                    };
                }
            } else {
                let count = self.labels().len();
                let a = self.active.as_mut().unwrap();
                a.focus = if forward {
                    (a.focus + 1) % count
                } else {
                    (a.focus + count - 1) % count
                };
            }
        }
    }
    pub fn accept(&mut self) {
        let Some(a) = &mut self.active else {
            return;
        };
        if a.checking.is_some() {
            return;
        }
        if let Some(path) = a.replace.clone() {
            if a.focus == 0 {
                self.finish(Answer::Path(path));
            } else {
                a.replace = None;
                a.focus = 0;
            }
            return;
        }
        match &a.request.kind {
            Kind::ConfirmSave { .. } => {
                let answer =
                    [Answer::Save, Answer::Discard, Answer::Cancel][a.focus.min(2)].clone();
                self.finish(answer);
            }
            Kind::Error { .. } => self.finish(Answer::Ok),
            Kind::Open { .. } | Kind::Save { .. } => {
                if a.focus == 3 {
                    self.cancel();
                    return;
                }
                let text = if a.focus == 1 {
                    a.entries
                        .get(a.selected)
                        .map(|e| e.path.to_string_lossy().into_owned())
                        .unwrap_or_default()
                } else {
                    a.field.buffer.create_snapshot().content.to_string()
                };
                if text.is_empty() {
                    return;
                }
                let save = matches!(a.request.kind, Kind::Save { .. });
                self.check(expand_path(&text), true, save);
            }
        }
    }
    fn check(&mut self, path: PathBuf, submit: bool, save: bool) {
        let (send, receive) = mpsc::channel();
        let wake = self.wake.clone();
        if let Some(a) = &mut self.active {
            a.checking = Some((receive, submit, field_context(a)));
        }
        thread::spawn(move || {
            let result = inspect_path(path, submit, save);
            let _ = send.send(result);
            wake();
        });
    }
    pub fn insert(&mut self, text: &str) {
        if let Some(a) = &mut self.active {
            if !matches!(a.request.kind, Kind::Open { .. } | Kind::Save { .. })
                || a.replace.is_some()
            {
                return;
            }
            let snapshot = a.field.buffer.create_snapshot();
            let (start, end) = a.field.selection.offsets(&snapshot);
            let removed = if a.field.selection.mode == texted_app::selection::SelectionMode::Stream
            {
                end - start
            } else {
                0
            };
            if text.contains(['\n', '\r', '\0'])
                || a.field
                    .buffer
                    .create_snapshot()
                    .total_bytes
                    .saturating_sub(removed)
                    .saturating_add(text.len())
                    > 4096
            {
                a.error = "A file path must be one line and at most 4096 bytes".into();
                return;
            }
            a.focus = 0;
            let _ = a.field.apply(EditorCommand::Insert(text.into()));
            a.error.clear();
        }
    }
    pub fn key(&mut self, input: &KeyInput<'_>) {
        if *input.logical == Key::Named(NamedKey::Escape) {
            self.cancel();
            return;
        }
        if *input.logical == Key::Named(NamedKey::Enter) {
            self.accept();
            return;
        }
        let path = self.path_mode();
        let count = if path { 4 } else { self.labels().len() };
        if *input.logical == Key::Named(NamedKey::Tab) {
            if let Some(a) = &mut self.active {
                a.focus = (a.focus as i32 + if input.modifiers.shift_key() { -1 } else { 1 })
                    .rem_euclid(count as i32) as usize;
            }
            return;
        }
        if !path {
            if matches!(
                input.logical,
                Key::Named(NamedKey::ArrowLeft | NamedKey::ArrowUp)
            ) {
                self.navigate(false);
            } else if matches!(
                input.logical,
                Key::Named(NamedKey::ArrowRight | NamedKey::ArrowDown)
            ) {
                self.navigate(true);
            }
            return;
        }
        if matches!(
            input.logical,
            Key::Named(NamedKey::ArrowUp | NamedKey::ArrowDown)
        ) {
            self.navigate(*input.logical == Key::Named(NamedKey::ArrowDown));
            return;
        }
        if let Some(intent) = input::decode(input, SearchContext::default()) {
            self.intent(intent);
        }
    }
    pub fn menu_state(&self) -> crate::menus::MenuState {
        let editable = self.path_mode();
        let field = self.active.as_ref().map(|a| &a.field);
        crate::menus::MenuState {
            editable,
            clipboard_available: editable,
            selected: editable
                && field.is_some_and(|f| {
                    f.selection.mode != texted_app::selection::SelectionMode::None
                }),
            can_undo: editable && field.is_some_and(|f| f.buffer.can_undo()),
            can_redo: editable && field.is_some_and(|f| f.buffer.can_redo()),
            ..Default::default()
        }
    }
    pub fn intent(&mut self, intent: Intent) {
        if !self.path_mode() {
            return;
        }
        match intent {
            Intent::Copy { .. } | Intent::Paste => {
                if let Some(a) = &mut self.active {
                    let context = field_context(a);
                    let action = if matches!(intent, Intent::Paste) {
                        ClipboardAction::Paste {
                            context,
                            field: None,
                        }
                    } else {
                        ClipboardAction::Copy {
                            context,
                            cut: matches!(intent, Intent::Copy { cut: true }),
                        }
                    };
                    if let Err(error) = self
                        .clipboard
                        .start(action, a.field.buffer.create_snapshot())
                    {
                        a.error = error;
                    }
                }
            }
            Intent::Edit(EditorCommand::Insert(text)) => self.insert(&text),
            Intent::Edit(other) => {
                if let Some(a) = &mut self.active {
                    a.focus = 0;
                    let _ = a.field.apply(other);
                }
            }
            _ => {}
        }
    }
    pub fn release(&mut self) {
        self.drag = None;
    }
    pub fn hover(&mut self, size: (usize, usize), compact: bool, x: f32, y: f32) -> bool {
        if let Some((sx, sy, px, py)) = self.drag {
            if let Some(a) = &mut self.active {
                let ch = if compact { 8 } else { 16 };
                a.position = Some((
                    ((px as f32 + x - sx).max(0.0) as usize / 8) * 8,
                    ((py as f32 + y - sy).max(0.0) as usize / ch) * ch,
                ));
            }
            return true;
        }
        let g = self.geometry(size, compact);
        let next = g.buttons.iter().position(|r| r.contains(x, y));
        let changed = next != self.hover;
        self.hover = next;
        changed
    }
    pub fn cursor(
        &self,
        size: (usize, usize),
        compact: bool,
        x: f32,
        y: f32,
    ) -> crate::cursor_ui::Shape {
        use crate::cursor_ui::Shape;
        let g = self.geometry(size, compact);
        if self.drag.is_some() {
            Shape::Grabbing
        } else if self.path_mode() && g.field.contains(x, y) {
            Shape::Text
        } else if g.buttons.iter().any(|r| r.contains(x, y))
            || g.close.contains(x, y)
            || g.list.contains(x, y)
        {
            Shape::Pointer
        } else if g.rect.contains(x, y) && y < (g.rect.y + 2 * g.ch) as f32 {
            Shape::Grab
        } else {
            Shape::Arrow
        }
    }
    pub fn click(&mut self, size: (usize, usize), compact: bool, x: f32, y: f32) {
        let path = self.path_mode();
        let g = self.geometry(size, compact);
        if g.close.contains(x, y) {
            self.cancel();
            return;
        }
        if let Some(button) = g.buttons.iter().position(|r| r.contains(x, y)) {
            if let Some(a) = &mut self.active {
                a.focus = if path { 2 + button } else { button };
            }
            self.accept();
            return;
        }
        if path && g.field.contains(x, y) {
            if let Some(a) = &mut self.active {
                a.focus = 0;
            }
            return;
        }
        if path && g.list.contains(x, y) {
            let index =
                self.active.as_ref().map_or(0, |a| a.first) + (y as usize - g.list.y) / g.ch;
            if let Some(a) = &mut self.active {
                if index < a.entries.len() {
                    a.focus = 1;
                    a.selected = index;
                    let text = a.entries[index].path.to_string_lossy().into_owned();
                    set_field(a, text);
                    a.focus = 1;
                    let double = self.last_click.is_some_and(|(i, time)| {
                        i == index && time.elapsed() < Duration::from_millis(400)
                    });
                    self.last_click = Some((index, Instant::now()));
                    if double {
                        self.accept();
                    }
                }
            }
            return;
        }
        if g.rect.contains(x, y) && y < (g.rect.y + 2 * g.ch) as f32 {
            self.drag = Some((x, y, g.rect.x, g.rect.y));
        }
    }
    pub fn paint(
        &mut self,
        list: &mut DrawList,
        font: &mut FontEngine,
        size: (usize, usize),
        compact: bool,
        p: &Palette,
    ) {
        if !self.open() {
            return;
        }
        list.cells
            .push(texted_app::cell_grid::CellCommand::HideCursor);
        let labels = self.labels();
        let path = self.path_mode();
        let g = self.geometry(size, compact);
        let title = self.title();
        let a = self.active.as_mut().unwrap();
        let (w, h) = size;
        draw_box(
            list,
            w,
            h,
            g.rect.x,
            g.rect.y,
            g.rect.w,
            g.rect.h,
            p.panel_bg,
            p.panel_border,
        );
        font.draw_string(
            list,
            w,
            h,
            g.rect.x + 2 * g.cw,
            g.rect.y + g.ch,
            title,
            p.text,
        );
        font.draw_string(list, w, h, g.close.x, g.close.y, "x", p.status_text);
        if path {
            let status = if a.checking.is_some() {
                "Reading directory..."
            } else {
                &a.error
            };
            font.draw_string(
                list,
                g.rect.x + g.rect.w - 2 * g.cw,
                h,
                g.rect.x + 2 * g.cw,
                g.rect.y + 2 * g.ch,
                status,
                p.status_accent,
            );
            draw_box(
                list,
                w,
                h,
                g.field.x,
                g.field.y,
                g.field.w,
                g.field.h,
                p.input_bg,
                if a.focus == 0 {
                    p.input_active
                } else {
                    p.input_border
                },
            );
            let snap = a.field.buffer.create_snapshot();
            let col = snap.content.byte_to_char(snap.cursor_offset);
            let visible = g.field.w / g.cw - 2;
            let first = col.saturating_sub(visible.saturating_sub(1));
            let text = snap
                .content
                .chars()
                .skip(first)
                .take(visible)
                .collect::<String>();
            if a.field.selection.mode == texted_app::selection::SelectionMode::Stream {
                let (start, end) = a.field.selection.offsets(&snap);
                let start = snap.content.byte_to_char(start).max(first);
                let end = snap.content.byte_to_char(end).min(first + visible);
                if start < end {
                    draw_rect(
                        list,
                        w,
                        h,
                        g.field.x + g.cw + (start - first) * g.cw,
                        g.field.y + g.ch,
                        (end - start) * g.cw,
                        g.ch,
                        p.selection,
                    );
                }
            }
            font.draw_string(
                list,
                g.field.x + g.field.w - g.cw,
                h,
                g.field.x + g.cw,
                g.field.y + g.ch,
                &text,
                p.text,
            );
            if a.focus == 0 {
                let x = g.field.x + g.cw + (col - first) * g.cw;
                list.cells.push(texted_app::cell_grid::CellCommand::Cursor {
                    at: [x as f32, (g.field.y + g.ch) as f32],
                });
                draw_rect(list, w, h, x, g.field.y + g.ch, 2, g.ch, p.caret);
            }
            let rows = (g.list.h / g.ch).max(1);
            if a.selected < a.first {
                a.first = a.selected;
            }
            if a.selected >= a.first + rows {
                a.first = a.selected + 1 - rows;
            }
            for (row, entry) in a.entries.iter().skip(a.first).take(rows).enumerate() {
                let selected = a.selected == a.first + row && a.focus == 1;
                let y = g.list.y + row * g.ch;
                if selected {
                    draw_rect(list, w, h, g.list.x, y, g.list.w, g.ch, p.input_active);
                }
                let text = format!(
                    "{}{}",
                    if entry.directory { "/ " } else { "  " },
                    entry.name
                );
                font.draw_string(
                    list,
                    g.list.x + g.list.w,
                    h,
                    g.list.x,
                    y,
                    &text,
                    if selected { p.text } else { p.status_text },
                );
            }
        } else {
            let message = if let Some(path) = &a.replace {
                format!("{} already exists. Replace it?", path.display())
            } else {
                match &a.request.kind {
                    Kind::ConfirmSave { name } => format!("Save changes to {name} before closing?"),
                    Kind::Error { message } => message.clone(),
                    _ => String::new(),
                }
            };
            let cols = (g.rect.w / g.cw).saturating_sub(4).max(1);
            for (row, chunk) in message
                .chars()
                .collect::<Vec<_>>()
                .chunks(cols)
                .take(3)
                .enumerate()
            {
                font.draw_string(
                    list,
                    w,
                    h,
                    g.rect.x + 2 * g.cw,
                    g.rect.y + (3 + row) * g.ch,
                    &chunk.iter().collect::<String>(),
                    p.status_text,
                );
            }
        }
        for (index, (rect, label)) in g.buttons.iter().zip(labels).enumerate() {
            let selected = if path {
                a.focus == 2 + index
            } else {
                a.focus == index
            };
            draw_rect(
                list,
                w,
                h,
                rect.x,
                rect.y,
                rect.w,
                rect.h,
                if selected {
                    p.input_active
                } else if self.hover == Some(index) {
                    p.panel_border
                } else {
                    p.button_bg
                },
            );
            font.draw_string(
                list,
                rect.x + rect.w,
                h,
                rect.x + g.cw,
                rect.y,
                &format!("[{label}]"),
                p.text,
            );
        }
    }
}
fn set_field(active: &mut Active, text: String) {
    active.field = EditorModel::new(ropey::Rope::from_str(&text));
    active.field.buffer.set_cursor_offset(text.len());
    active.focus = 0;
}
fn field_context(active: &Active) -> EditContext {
    let snapshot = active.field.buffer.create_snapshot();
    EditContext {
        document: active.serial,
        revision: snapshot.revision,
        cursor: snapshot.cursor_offset,
        selection: active.field.selection,
    }
}
fn expand_path(text: &str) -> PathBuf {
    let path = if text == "~" {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(text))
    } else if let Some(rest) = text.strip_prefix("~/") {
        std::env::var_os("HOME")
            .map(|home| PathBuf::from(home).join(rest))
            .unwrap_or_else(|| PathBuf::from(text))
    } else {
        PathBuf::from(text)
    };
    if path.is_absolute() {
        path
    } else {
        std::env::current_dir().unwrap_or_default().join(path)
    }
}
fn inspect_path(path: PathBuf, submit: bool, save: bool) -> Result<PathResult, String> {
    if path.is_dir() {
        let mut entries = Vec::new();
        if let Some(parent) = path.parent() {
            entries.push(Entry {
                path: parent.to_owned(),
                name: "..".into(),
                directory: true,
            });
        }
        let mut truncated = false;
        for entry in std::fs::read_dir(&path).map_err(|e| e.to_string())? {
            if entries.len() >= 1000 {
                truncated = true;
                break;
            }
            let entry = entry.map_err(|e| e.to_string())?;
            let directory = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
            entries.push(Entry {
                path: entry.path(),
                name: entry.file_name().to_string_lossy().into_owned(),
                directory,
            });
        }
        entries.sort_by_cached_key(|e| (!e.directory, e.name.to_lowercase()));
        Ok(PathResult::Directory(path, entries, truncated))
    } else if submit
        && (path.is_file() || save && path.parent().unwrap_or(std::path::Path::new(".")).is_dir())
    {
        let exists = path.exists();
        Ok(PathResult::File(path, exists))
    } else {
        Err(format!("Cannot open {}", path.display()))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_changed_path_rejects_a_late_submission_result() {
        let (client, mut ui) = channel(Arc::new(|| {}));
        let ticket = client
            .request(Kind::Save {
                path: PathBuf::from("untitled.txt"),
            })
            .unwrap();
        ui.poll();
        let (send, receive) = mpsc::channel();
        let a = ui.active.as_mut().unwrap();
        let context = field_context(a);
        a.checking = Some((receive, true, context));
        ui.insert("changed");
        send.send(Ok(PathResult::File(PathBuf::from("stale.txt"), false)))
            .unwrap();
        ui.poll();
        assert_eq!(ticket.poll(), None);
        assert!(ui.active.as_ref().unwrap().error.contains("Path changed"));
    }
    #[test]
    fn confirmation_tickets_cancel_and_complete_without_native_ui() {
        let (client, mut ui) = channel(Arc::new(|| {}));
        let ticket = client
            .request(Kind::ConfirmSave {
                name: "Ї.txt".into(),
            })
            .unwrap();
        assert!(ui.poll());
        assert!(ui.open());
        ui.navigate(true);
        ui.accept();
        assert_eq!(ticket.poll(), Some(Answer::Discard));
        assert!(!ui.open());
    }
    #[test]
    fn queued_dialogs_are_serial_and_cancellation_does_not_leak_to_the_next() {
        let (client, mut ui) = channel(Arc::new(|| {}));
        let a = client
            .request(Kind::Error {
                message: "one".into(),
            })
            .unwrap();
        let b = client
            .request(Kind::Error {
                message: "two".into(),
            })
            .unwrap();
        ui.poll();
        ui.cancel();
        ui.poll();
        ui.accept();
        assert_eq!(a.poll(), Some(Answer::Cancel));
        assert_eq!(b.poll(), Some(Answer::Ok));
    }
    #[test]
    fn dialog_geometry_fits_a_small_character_grid() {
        for size in [(640, 384), (256, 176)] {
            let g = Geometry::new(size, size.0 < 400, true, 2, None);
            assert!(g.rect.x + g.rect.w <= size.0);
            assert!(g.rect.y + g.rect.h <= size.1);
            for b in g.buttons {
                assert!(b.x + b.w <= size.0);
                assert!(b.y + b.h <= size.1);
            }
        }
    }
}
