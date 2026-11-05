//! Single-document file lifecycle. Native sheets are polled on main; file I/O runs on snapshots.
use crate::{BufferRequest, BufferResponse};
use ropey::Rope;
use std::{
    future::Future,
    path::PathBuf,
    pin::Pin,
    sync::{mpsc, Arc},
    task::{Context, Poll, Wake, Waker},
    thread,
};
use texted_buffer::BufferSnapshot;
use texted_io::{IoManager, LineEnding, LoadedDocument};
use texted_kernel::ChannelClient;
use winit::window::Window;
#[derive(Debug, Clone)]
pub enum Action {
    OpenDialog,
    OpenPath(PathBuf),
    New,
    Save { save_as: bool },
    Close,
}
pub enum Effect {
    None,
    Install(LoadedDocument),
    Close,
}
type NativeFuture<T> = Pin<Box<dyn Future<Output = T>>>;
struct Saved {
    path: PathBuf,
    text: Rope,
}
enum Job {
    Save(mpsc::Receiver<Result<Saved, String>>),
    Load(mpsc::Receiver<Result<LoadedDocument, String>>),
    ChooseSave {
        future: NativeFuture<Option<rfd::FileHandle>>,
        snapshot: BufferSnapshot,
    },
    ChooseOpen(NativeFuture<Option<rfd::FileHandle>>),
    Confirm {
        future: NativeFuture<rfd::MessageDialogResult>,
        action: Action,
    },
    Error(NativeFuture<rfd::MessageDialogResult>),
}
struct WindowWake(Arc<Window>);
impl Wake for WindowWake {
    fn wake(self: Arc<Self>) {
        self.0.request_redraw();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.0.request_redraw();
    }
}
fn poll_native<T>(future: &mut NativeFuture<T>, window: &Arc<Window>) -> Poll<T> {
    let waker = Waker::from(Arc::new(WindowWake(window.clone())));
    future.as_mut().poll(&mut Context::from_waker(&waker))
}
pub struct Documents {
    pub path: Option<PathBuf>,
    pub bom: bool,
    pub ending: LineEnding,
    saved_text: Rope,
    job: Option<Job>,
    next: Option<Action>,
}
impl Documents {
    pub fn new(text: Rope, path: Option<PathBuf>, bom: bool, ending: LineEnding) -> Self {
        Self {
            path,
            bom,
            ending,
            saved_text: text,
            job: None,
            next: None,
        }
    }
    pub fn dirty(&self, snapshot: &BufferSnapshot) -> bool {
        !snapshot.content.is_instance(&self.saved_text)
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
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Untitled".into());
        format!(
            "{name}{}{} - Texted",
            if self.dirty(snapshot) { " *" } else { "" },
            match self.job {
                Some(Job::Load(_)) => " (Opening…)",
                Some(Job::Save(_)) => " (Saving…)",
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
    fn error(&mut self, window: &Arc<Window>, message: &str) {
        eprintln!("{message}");
        self.next = None;
        self.job = Some(Job::Error(Box::pin(
            rfd::AsyncMessageDialog::new()
                .set_parent(window.as_ref())
                .set_title("Texted")
                .set_description(message)
                .set_level(rfd::MessageLevel::Error)
                .show(),
        )));
        window.request_redraw();
    }
    pub fn handle(
        &mut self,
        action: Action,
        snapshot: &BufferSnapshot,
        client: &ChannelClient<BufferRequest, BufferResponse>,
        window: &Arc<Window>,
    ) -> Effect {
        if self.job.is_some() {
            if self.next.is_none() {
                self.next = Some(action);
            }
            return Effect::None;
        }
        if let Action::Save { save_as } = action {
            self.save(snapshot, save_as, client, window);
            return Effect::None;
        }
        if self.dirty(snapshot) {
            let future = Box::pin(
                rfd::AsyncMessageDialog::new()
                    .set_parent(window.as_ref())
                    .set_title("Save changes?")
                    .set_description("Your changes will be lost if you don't save them.")
                    .set_buttons(rfd::MessageButtons::YesNoCancelCustom(
                        "Save".into(),
                        "Discard".into(),
                        "Cancel".into(),
                    ))
                    .show(),
            );
            self.job = Some(Job::Confirm { future, action });
            window.request_redraw();
            return Effect::None;
        }
        self.proceed(action, window)
    }
    fn proceed(&mut self, action: Action, window: &Arc<Window>) -> Effect {
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
                let mut dialog = rfd::AsyncFileDialog::new()
                    .set_parent(window.as_ref())
                    .set_title("Open plain-text file");
                if let Some(parent) = self.path.as_ref().and_then(|p| p.parent()) {
                    dialog = dialog.set_directory(parent);
                }
                self.job = Some(Job::ChooseOpen(Box::pin(dialog.pick_file())));
                window.request_redraw();
                Effect::None
            }
            Action::OpenPath(path) => {
                self.open(path, window);
                Effect::None
            }
            Action::Save { .. } => unreachable!(),
        }
    }
    fn open(&mut self, path: PathBuf, window: &Arc<Window>) {
        let (tx, rx) = mpsc::channel();
        self.job = Some(Job::Load(rx));
        window.request_redraw();
        let window = window.clone();
        thread::spawn(move || {
            let result = IoManager::open_file(&path)
                .map_err(|e| format!("Could not open {}: {e}", path.display()));
            let _ = tx.send(result);
            window.request_redraw();
        });
    }
    fn save(
        &mut self,
        snapshot: &BufferSnapshot,
        save_as: bool,
        client: &ChannelClient<BufferRequest, BufferResponse>,
        window: &Arc<Window>,
    ) {
        let _ = client.send(BufferRequest::BreakUndoGroup);
        if save_as || self.path.is_none() {
            let mut dialog = rfd::AsyncFileDialog::new()
                .set_parent(window.as_ref())
                .set_title("Save plain-text file")
                .set_file_name(
                    self.path
                        .as_ref()
                        .and_then(|p| p.file_name())
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "Untitled.txt".into()),
                );
            if let Some(parent) = self.path.as_ref().and_then(|p| p.parent()) {
                dialog = dialog.set_directory(parent);
            }
            self.job = Some(Job::ChooseSave {
                future: Box::pin(dialog.save_file()),
                snapshot: snapshot.clone(),
            });
            window.request_redraw();
        } else {
            self.write(snapshot, self.path.clone().unwrap(), window);
        }
    }
    fn write(&mut self, snapshot: &BufferSnapshot, path: PathBuf, window: &Arc<Window>) {
        let (tx, rx) = mpsc::channel();
        self.job = Some(Job::Save(rx));
        window.request_redraw();
        let content = snapshot.content.clone();
        let bom = self.bom;
        let ending = self.ending;
        let window = window.clone();
        thread::spawn(move || {
            let result = IoManager::save_rope(&path, &content, bom, ending)
                .map_err(|e| format!("Could not save {}: {e}", path.display()))
                .map(|()| Saved {
                    path,
                    text: content,
                });
            let _ = tx.send(result);
            window.request_redraw();
        });
    }
    pub fn poll(
        &mut self,
        snapshot: &BufferSnapshot,
        client: &ChannelClient<BufferRequest, BufferResponse>,
        window: &Arc<Window>,
    ) -> Effect {
        if let Some(mut job) = self.job.take() {
            match &mut job {
                Job::Save(rx) => match rx.try_recv() {
                    Ok(Ok(saved)) => {
                        self.path = Some(saved.path);
                        self.saved_text = saved.text;
                    }
                    Ok(Err(error)) => {
                        self.error(window, &error);
                        return Effect::None;
                    }
                    Err(mpsc::TryRecvError::Disconnected) => {
                        self.error(window, "The save worker stopped before completing.");
                        return Effect::None;
                    }
                    Err(mpsc::TryRecvError::Empty) => {
                        self.job = Some(job);
                        return Effect::None;
                    }
                },
                Job::Load(rx) => match rx.try_recv() {
                    Ok(Ok(document)) => return Effect::Install(document),
                    Ok(Err(error)) => {
                        self.error(window, &error);
                        return Effect::None;
                    }
                    Err(mpsc::TryRecvError::Disconnected) => {
                        self.error(window, "The file loader stopped before completing.");
                        return Effect::None;
                    }
                    Err(mpsc::TryRecvError::Empty) => {
                        self.job = Some(job);
                        return Effect::None;
                    }
                },
                Job::ChooseSave {
                    future,
                    snapshot: written,
                } => match poll_native(future, window) {
                    Poll::Ready(Some(file)) => {
                        self.write(written, file.path().into(), window);
                        return Effect::None;
                    }
                    Poll::Ready(None) => {
                        self.next = None;
                    }
                    Poll::Pending => {
                        self.job = Some(job);
                        return Effect::None;
                    }
                },
                Job::ChooseOpen(future) => match poll_native(future, window) {
                    Poll::Ready(Some(file)) => {
                        self.open(file.path().into(), window);
                        return Effect::None;
                    }
                    Poll::Ready(None) => {}
                    Poll::Pending => {
                        self.job = Some(job);
                        return Effect::None;
                    }
                },
                Job::Confirm { future, action } => match poll_native(future, window) {
                    Poll::Ready(choice) => {
                        if choice == rfd::MessageDialogResult::Yes
                            || choice == rfd::MessageDialogResult::Custom("Save".into())
                        {
                            self.next = Some(action.clone());
                            self.save(snapshot, false, client, window);
                            return Effect::None;
                        }
                        if choice == rfd::MessageDialogResult::No
                            || choice == rfd::MessageDialogResult::Custom("Discard".into())
                        {
                            return self.proceed(action.clone(), window);
                        }
                        self.next = None;
                    }
                    Poll::Pending => {
                        self.job = Some(job);
                        return Effect::None;
                    }
                },
                Job::Error(future) => match poll_native(future, window) {
                    Poll::Ready(_) => {}
                    Poll::Pending => {
                        self.job = Some(job);
                        return Effect::None;
                    }
                },
            }
        }
        if let Some(action) = self.next.take() {
            return self.handle(action, snapshot, client, window);
        }
        Effect::None
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use texted_buffer::TextBuffer;
    #[test]
    fn undo_to_saved_root_is_clean_and_cursor_changes_are_not_edits() {
        let mut b = TextBuffer::from_string("original".into());
        let doc = Documents::new(b.create_snapshot().content, None, false, LineEnding::Lf);
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
        let doc = Documents::new(written, Some("file.txt".into()), false, LineEnding::Lf);
        assert!(doc.dirty(&b.create_snapshot()));
    }
}
