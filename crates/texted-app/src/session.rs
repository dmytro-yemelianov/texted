//! Per-tab ownership: workers, undo, selection, file lifecycle and viewport travel together.
use crate::{file_flow::FileFlow, search_ui::SearchUi, transfers::Transfers, viewport::Viewport};
use texted_app::buffer_service::BufferService;
pub(crate) struct Session {
    pub id: u64,
    pub buffer: BufferService,
    pub search: SearchUi,
    pub transfers: Transfers,
    pub files: FileFlow,
    pub viewport: Viewport,
    pub message: Option<String>,
    pub snap_after: Option<u64>,
}
impl Session {
    pub fn new(wake: crate::Wake, id: u64, dialogs: crate::dialog_ui::Client) -> Self {
        let w = wake.clone();
        let buffer = BufferService::new(move || w());
        let w = wake.clone();
        let search = SearchUi::new(move || w());
        let w = wake.clone();
        let transfers = Transfers::new(move || w());
        let w = wake.clone();
        let viewport = Viewport::new(move || w());
        let files = FileFlow::new(
            buffer.client.latest().snapshot.content.clone(),
            wake,
            dialogs,
        );
        Self {
            id,
            buffer,
            search,
            transfers,
            files,
            viewport,
            message: None,
            snap_after: None,
        }
    }
    pub fn settled(&self) -> bool {
        let p = self.buffer.client.latest();
        self.files.idle()
            && !self.transfers.busy()
            && p.busy.is_none()
            && p.completed_through >= self.buffer.client.barrier()
    }
    pub fn label(&self) -> String {
        let name = self
            .files
            .documents
            .path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Untitled".into());
        format!(
            "{}{}",
            name,
            if self
                .files
                .documents
                .dirty(&self.buffer.client.latest().snapshot)
            {
                " *"
            } else {
                ""
            }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn workers_shut_down_without_dispatching_native_ui_events() {
        use std::{
            sync::{mpsc, Arc},
            time::{Duration, SystemTime, UNIX_EPOCH},
        };
        use texted_app::{
            appearance::{Appearance, Theme},
            buffer_service::Command,
            commands::EditorCommand,
            document_stats::DocumentStats,
            preferences::{self, Preferences},
        };
        let directory = std::env::temp_dir().join(format!(
            "texted-exit-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = directory.join("appearance.json");
        let written = path.clone();
        let (events, event_queue) = mpsc::channel();
        let wake: crate::Wake = Arc::new(move || {
            let _ = events.send(());
        });
        let (finished, completion) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let session = Session::new(wake.clone(), 1, crate::dialog_ui::channel(wake.clone()).0);
            session
                .buffer
                .client
                .submit(Command::Edit(EditorCommand::Insert("Їжак".into())))
                .unwrap_or_else(|_| panic!("edit rejected"));
            let signal = wake.clone();
            let mut stats = DocumentStats::new(move || signal());
            stats.update(1, &session.buffer.client.latest().snapshot);
            let signal = wake.clone();
            let mut preferences = Preferences::new(written, move || signal());
            preferences
                .save(Appearance {
                    theme: Theme::Amber,
                    ..Default::default()
                })
                .unwrap();
            drop(session);
            drop(stats);
            drop(preferences);
            let _ = finished.send(());
        });
        // The native UI is deliberately not dispatching anything during worker teardown.
        completion
            .recv_timeout(Duration::from_secs(5))
            .expect("worker shutdown waited for UI dispatch");
        worker.join().unwrap();
        assert!(event_queue.try_iter().count() > 0);
        assert_eq!(preferences::load(&path).unwrap().theme, Theme::Amber);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
