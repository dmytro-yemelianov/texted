//! Single-document file lifecycle. Native sheets are polled on main; file I/O runs on snapshots.
use crate::surface::Host;
use ropey::Rope;
use std::{path::PathBuf, sync::mpsc, thread};
use texted_app::buffer_service::{BufferClient, Command};
use texted_buffer::BufferSnapshot;
use texted_io::{IoManager, LineEnding, LoadedDocument};
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
    Save { save_as: bool },
    ReportError(String),
    Close,
}
pub enum Effect {
    None,
    Install(LoadedDocument),
    Close,
}
struct Saved {
    path: PathBuf,
    text: Rope,
}
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
    saved_text: Rope,
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
            saved_text: text,
            wake,
            dialogs,
            job: None,
            next: None,
        }
    }
    pub fn dirty(&self, snapshot: &BufferSnapshot) -> bool {
        !snapshot.content.is_instance(&self.saved_text)
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
        self.saved_text = document.content.clone();
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
                self.open(path);
                Effect::None
            }
            Action::Save { .. } | Action::ReportError(_) => unreachable!(),
        }
    }
    fn open(&mut self, path: PathBuf) {
        let (tx, rx) = mpsc::channel();
        self.job = Some(Job::Load(rx));
        let wake = self.wake.clone();
        thread::spawn(move || {
            let result = IoManager::open_file(&path)
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
        let wake = self.wake.clone();
        thread::spawn(move || {
            let result = IoManager::save_rope(&path, &content, bom, ending)
                .map_err(|e| format!("Could not save {}: {e}", path.display()))
                .map(|()| Saved {
                    path,
                    text: content,
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
                        self.open(path);
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
