//! Single-document file lifecycle. Native sheets are polled on main; file I/O runs on snapshots.
use crate::surface::Host;
use ropey::Rope;
use std::{path::PathBuf, sync::mpsc, thread};
use texted_app::buffer_service::{BufferClient, Command};
use texted_buffer::BufferSnapshot;
use texted_io::{IoManager, LineEnding, LoadedDocument, TextEncoding};
/// A user-requested file operation must not wait on a hidden or minimized sheet.
pub(crate) fn reveal_for_dialog(window: &Host) {
    window.set_visible(true);
    window.set_minimized(false);
    window.focus_window();
}
#[derive(Debug, Clone)]
pub enum Action {
    OpenDialog,
    OpenPath(PathBuf),
    New,
    Save {
        save_as: bool,
    },
    ReportError(String),
    Close,
    /// Rereads the current file in another encoding, asking first if it has changes.
    Reopen(TextEncoding),
}
pub enum Effect {
    None,
    Install(LoadedDocument),
    Close,
}
struct Saved {
    path: PathBuf,
    text: Rope,
    format: (TextEncoding, bool),
}
/// Save formats offered in Document details; UTF-16 always carries its byte-order mark.
const SAVE_FORMATS: [(TextEncoding, bool); 6] = [
    (TextEncoding::Utf8, false),
    (TextEncoding::Utf8, true),
    (TextEncoding::Utf16Le, true),
    (TextEncoding::Utf16Be, true),
    (TextEncoding::Windows1251, false),
    (TextEncoding::Koi8U, false),
];
enum Job {
    Save(mpsc::Receiver<Result<Saved, String>>),
    Load(mpsc::Receiver<Result<LoadedDocument, String>>),
    ChooseSave {
        ticket: crate::dialog_ui::Ticket,
        snapshot: BufferSnapshot,
    },
    ChooseOpen(crate::dialog_ui::Ticket),
    Confirm {
        ticket: crate::dialog_ui::Ticket,
        action: Action,
    },
    Error(crate::dialog_ui::Ticket),
}
pub struct Documents {
    pub path: Option<PathBuf>,
    pub bom: bool,
    pub ending: LineEnding,
    pub encoding: TextEncoding,
    saved_text: Rope,
    saved_format: (TextEncoding, bool),
    wake: crate::Wake,
    dialogs: crate::dialog_ui::Client,
    job: Option<Job>,
    next: Option<Action>,
}
impl Documents {
    pub fn new(
        text: Rope,
        path: Option<PathBuf>,
        bom: bool,
        ending: LineEnding,
        wake: crate::Wake,
        dialogs: crate::dialog_ui::Client,
    ) -> Self {
        Self {
            path,
            bom,
            ending,
            encoding: TextEncoding::Utf8,
            saved_text: text,
            saved_format: (TextEncoding::Utf8, bom),
            wake,
            dialogs,
            job: None,
            next: None,
        }
    }
    pub fn dirty(&self, snapshot: &BufferSnapshot) -> bool {
        !snapshot.content.is_instance(&self.saved_text)
            || (self.encoding, self.bom) != self.saved_format
    }
    pub fn format_name(&self) -> String {
        match (self.encoding, self.bom) {
            (TextEncoding::Utf8, true) => "UTF-8 with BOM".into(),
            (encoding, _) => encoding.name().into(),
        }
    }
    /// Cycles the encoding the next save writes; it counts as a change until saved.
    pub fn next_save_format(&mut self) {
        let next = SAVE_FORMATS
            .iter()
            .position(|&format| format == (self.encoding, self.bom))
            .map_or(0, |i| (i + 1) % SAVE_FORMATS.len());
        (self.encoding, self.bom) = SAVE_FORMATS[next];
    }
    /// The encoding Document details offers for rereading: the next after the current one.
    pub fn reread_target(&self) -> TextEncoding {
        const ORDER: [TextEncoding; 5] = [
            TextEncoding::Utf8,
            TextEncoding::Windows1251,
            TextEncoding::Koi8U,
            TextEncoding::Utf16Le,
            TextEncoding::Utf16Be,
        ];
        let next = ORDER
            .iter()
            .position(|&e| e == self.encoding)
            .map_or(0, |i| (i + 1) % ORDER.len());
        ORDER[next]
    }
    /// Accept external lifecycle actions only after a dialog, worker and its continuation finish.
    pub fn is_idle(&self) -> bool {
        self.job.is_none() && self.next.is_none()
    }
    pub fn locked(&self) -> bool {
        self.job
            .as_ref()
            .is_some_and(|j| !matches!(j, Job::Save(_)))
            || self.next.is_some()
    }
    pub fn title(&self, snapshot: &BufferSnapshot) -> String {
        let name = self
            .path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Untitled".into());
        format!(
            "{name}{}{} - Texted",
            if self.dirty(snapshot) { " *" } else { "" },
            match self.job {
                Some(Job::Load(_)) => " (Opening...)",
                Some(Job::Save(_)) => " (Saving...)",
                _ => "",
            }
        )
    }
    pub fn installed(&mut self, document: &LoadedDocument) {
        self.path = document.path.clone();
        self.bom = document.has_bom;
        self.ending = document.line_ending;
        self.encoding = document.encoding;
        self.saved_text = document.content.clone();
        self.saved_format = (document.encoding, document.has_bom);
    }
    fn error(&mut self, message: &str) {
        self.next = None;
        self.job = self
            .dialogs
            .request(crate::dialog_ui::Kind::Error {
                message: message.into(),
            })
            .ok()
            .map(Job::Error);
    }
    pub fn handle(
        &mut self,
        action: Action,
        snapshot: &BufferSnapshot,
        client: &BufferClient,
    ) -> Effect {
        if self.job.is_some() {
            if self.next.is_none() {
                self.next = Some(action);
            }
            return Effect::None;
        }
        if let Action::Save { save_as } = action {
            self.save(snapshot, save_as, client);
            return Effect::None;
        }
        if let Action::ReportError(message) = action {
            self.error(&message);
            return Effect::None;
        }
        if self.dirty(snapshot) {
            let name = self
                .path
                .as_ref()
                .and_then(|p| p.file_name())
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|| "Untitled".into());
            match self
                .dialogs
                .request(crate::dialog_ui::Kind::ConfirmSave { name })
            {
                Ok(ticket) => self.job = Some(Job::Confirm { ticket, action }),
                Err(error) => self.error(&error),
            }
            return Effect::None;
        }
        self.proceed(action)
    }
    fn proceed(&mut self, action: Action) -> Effect {
        match action {
            Action::Close => Effect::Close,
            Action::New => Effect::Install(LoadedDocument {
                content: Rope::new(),
                path: None,
                has_bom: false,
                line_ending: LineEnding::Lf,
                encoding: TextEncoding::Utf8,
                byte_size: 0,
            }),
            Action::OpenDialog => {
                let directory = self
                    .path
                    .as_ref()
                    .and_then(|p| p.parent())
                    .map(std::path::Path::to_owned)
                    .unwrap_or_else(|| {
                        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
                    });
                match self
                    .dialogs
                    .request(crate::dialog_ui::Kind::Open { directory })
                {
                    Ok(ticket) => self.job = Some(Job::ChooseOpen(ticket)),
                    Err(error) => self.error(&error),
                }
                Effect::None
            }
            Action::OpenPath(path) => {
                self.open(path, None);
                Effect::None
            }
            Action::Reopen(encoding) => {
                match self.path.clone() {
                    Some(path) => self.open(path, Some(encoding)),
                    None => self.error("Save the document before reading it in another encoding"),
                }
                Effect::None
            }
            Action::Save { .. } | Action::ReportError(_) => unreachable!(),
        }
    }
    fn open(&mut self, path: PathBuf, encoding: Option<TextEncoding>) {
        let (tx, rx) = mpsc::channel();
        self.job = Some(Job::Load(rx));
        let wake = self.wake.clone();
        thread::spawn(move || {
            let result = IoManager::open_file_as(&path, encoding)
                .map_err(|e| format!("Could not open {}: {e}", path.display()));
            let _ = tx.send(result);
            wake();
        });
    }
    fn save(&mut self, snapshot: &BufferSnapshot, save_as: bool, client: &BufferClient) {
        if client.submit(Command::BreakUndoGroup).is_err() {
            self.error("Buffer is busy; save was not accepted. Please retry.");
            return;
        }
        if save_as || self.path.is_none() {
            let path = self.path.clone().unwrap_or_else(|| {
                std::env::current_dir()
                    .unwrap_or_else(|_| PathBuf::from("."))
                    .join("Untitled.txt")
            });
            match self.dialogs.request(crate::dialog_ui::Kind::Save { path }) {
                Ok(ticket) => {
                    self.job = Some(Job::ChooseSave {
                        ticket,
                        snapshot: snapshot.clone(),
                    })
                }
                Err(error) => self.error(&error),
            }
        } else {
            self.write(snapshot, self.path.clone().unwrap());
        }
    }
    fn write(&mut self, snapshot: &BufferSnapshot, path: PathBuf) {
        let (tx, rx) = mpsc::channel();
        self.job = Some(Job::Save(rx));
        let content = snapshot.content.clone();
        let bom = self.bom;
        let ending = self.ending;
        let encoding = self.encoding;
        let wake = self.wake.clone();
        thread::spawn(move || {
            let result = IoManager::save_rope_as(&path, &content, bom, ending, encoding)
                .map_err(|e| {
                    let hint = if e.kind() == std::io::ErrorKind::InvalidData {
                        ". Choose another encoding in Document details (Cmd+Shift+I)"
                    } else {
                        ""
                    };
                    format!("Could not save {}: {e}{hint}", path.display())
                })
                .map(|()| Saved {
                    path,
                    text: content,
                    format: (encoding, bom),
                });
            let _ = tx.send(result);
            wake();
        });
    }
    pub fn poll(&mut self, snapshot: &BufferSnapshot, client: &BufferClient) -> Effect {
        use crate::dialog_ui::Answer;
        if let Some(job) = self.job.take() {
            match job {
                Job::Save(receive) => match receive.try_recv() {
                    Ok(Ok(saved)) => {
                        self.path = Some(saved.path);
                        self.saved_text = saved.text;
                        self.saved_format = saved.format;
                    }
                    Ok(Err(error)) => {
                        self.error(&error);
                        return Effect::None;
                    }
                    Err(mpsc::TryRecvError::Empty) => {
                        self.job = Some(Job::Save(receive));
                        return Effect::None;
                    }
                    Err(mpsc::TryRecvError::Disconnected) => {
                        self.error("The save worker stopped before completing");
                        return Effect::None;
                    }
                },
                Job::Load(receive) => match receive.try_recv() {
                    Ok(Ok(document)) => return Effect::Install(document),
                    Ok(Err(error)) => {
                        self.error(&error);
                        return Effect::None;
                    }
                    Err(mpsc::TryRecvError::Empty) => {
                        self.job = Some(Job::Load(receive));
                        return Effect::None;
                    }
                    Err(mpsc::TryRecvError::Disconnected) => {
                        self.error("The file loader stopped before completing");
                        return Effect::None;
                    }
                },
                Job::ChooseSave {
                    ticket,
                    snapshot: written,
                } => match ticket.poll() {
                    Some(Answer::Path(path)) => {
                        self.write(&written, path);
                        return Effect::None;
                    }
                    Some(_) => self.next = None,
                    None => {
                        self.job = Some(Job::ChooseSave {
                            ticket,
                            snapshot: written,
                        });
                        return Effect::None;
                    }
                },
                Job::ChooseOpen(ticket) => match ticket.poll() {
                    Some(Answer::Path(path)) => {
                        self.open(path, None);
                        return Effect::None;
                    }
                    Some(_) => {}
                    None => {
                        self.job = Some(Job::ChooseOpen(ticket));
                        return Effect::None;
                    }
                },
                Job::Confirm { ticket, action } => match ticket.poll() {
                    Some(Answer::Save) => {
                        self.next = Some(action);
                        self.save(snapshot, false, client);
                        return Effect::None;
                    }
                    Some(Answer::Discard) => return self.proceed(action),
                    Some(_) => self.next = None,
                    None => {
                        self.job = Some(Job::Confirm { ticket, action });
                        return Effect::None;
                    }
                },
                Job::Error(ticket) => {
                    if ticket.poll().is_none() {
                        self.job = Some(Job::Error(ticket));
                        return Effect::None;
                    }
                }
            }
        }
        if let Some(action) = self.next.take() {
            return self.handle(action, snapshot, client);
        }
        Effect::None
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use texted_buffer::TextBuffer;
    fn dialogs() -> crate::dialog_ui::Client {
        let (client, _) = crate::dialog_ui::channel(Arc::new(|| {}));
        client
    }
    #[test]
    fn actions_wait_for_file_work_and_its_continuation() {
        let mut doc = Documents::new(
            Rope::new(),
            None,
            false,
            LineEnding::Lf,
            Arc::new(|| {}),
            dialogs(),
        );
        assert!(doc.is_idle());
        let (_tx, rx) = mpsc::channel();
        doc.job = Some(Job::Save(rx));
        assert!(!doc.is_idle());
        doc.next = Some(Action::Close);
        doc.job = None;
        assert!(!doc.is_idle());
        doc.next = None;
        assert!(doc.is_idle());
    }
    #[test]
    fn undo_to_saved_root_is_clean_and_cursor_changes_are_not_edits() {
        let mut b = TextBuffer::from_string("original".into());
        let doc = Documents::new(
            b.create_snapshot().content,
            None,
            false,
            LineEnding::Lf,
            Arc::new(|| {}),
            dialogs(),
        );
        b.set_cursor_offset(3);
        assert!(!doc.dirty(&b.create_snapshot()));
        b.insert(3, "ї");
        assert!(doc.dirty(&b.create_snapshot()));
        assert!(b.undo());
        assert!(!doc.dirty(&b.create_snapshot()));
    }
    #[test]
    fn changing_the_save_encoding_counts_as_a_change_until_cycled_back() {
        let b = TextBuffer::from_string("їжак".into());
        let snap = b.create_snapshot();
        let mut doc = Documents::new(
            snap.content.clone(),
            Some("doc.txt".into()),
            false,
            LineEnding::Lf,
            Arc::new(|| {}),
            dialogs(),
        );
        assert_eq!(doc.format_name(), "UTF-8");
        assert!(!doc.dirty(&snap));
        let mut seen = vec![];
        for _ in 0..6 {
            doc.next_save_format();
            seen.push(doc.format_name());
            assert_eq!(doc.dirty(&snap), doc.format_name() != "UTF-8");
        }
        assert_eq!(
            seen,
            [
                "UTF-8 with BOM",
                "UTF-16 LE",
                "UTF-16 BE",
                "Windows-1251",
                "KOI8-U",
                "UTF-8"
            ]
        );
    }
    #[test]
    fn reopen_rereads_the_same_file_in_the_chosen_encoding() {
        let dir = std::env::temp_dir().join(format!("texted-reopen-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("legacy.txt");
        std::fs::write(&path, [191, 230, 224, 234, 10]).unwrap(); // "їжак\n" in Windows-1251
        let service = texted_app::buffer_service::BufferService::new(|| {});
        let snap = service.client.latest().snapshot.clone();
        let mut doc = Documents::new(
            snap.content.clone(),
            Some(path),
            false,
            LineEnding::Lf,
            Arc::new(|| {}),
            dialogs(),
        );
        assert!(matches!(
            doc.handle(Action::Reopen(TextEncoding::Koi8U), &snap, &service.client),
            Effect::None
        ));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let document = loop {
            if let Effect::Install(document) = doc.poll(&snap, &service.client) {
                break document;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "reopen never finished"
            );
            std::thread::sleep(std::time::Duration::from_millis(2));
        };
        assert_eq!(document.encoding, TextEncoding::Koi8U);
        assert_eq!(document.content.to_string(), "©ФЮЙ\n");
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn saving_old_snapshot_does_not_clean_newer_edits() {
        let mut b = TextBuffer::new();
        b.insert(0, "first");
        let written = b.create_snapshot().content;
        b.insert(5, "second");
        let doc = Documents::new(
            written,
            Some("file.txt".into()),
            false,
            LineEnding::Lf,
            Arc::new(|| {}),
            dialogs(),
        );
        assert!(doc.dirty(&b.create_snapshot()));
    }
}
