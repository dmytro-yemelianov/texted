//! File actions wait for accepted edits and clipboard continuations before dirty checks.
use crate::{
    documents::{Action, Documents, Effect},
    transfers::submit,
};
use std::{collections::VecDeque, sync::Arc};
use texted_app::{
    buffer_service::{BufferClient, Command, Published},
    input::{FileIntent, Intent},
};
use texted_io::LineEnding;
use winit::window::Window;
pub(crate) struct FileFlow {
    pub documents: Documents,
    queue: VecDeque<Action>,
    install: Option<u64>,
    pub reset: bool,
}
impl FileFlow {
    pub fn new(content: ropey::Rope) -> Self {
        Self {
            documents: Documents::new(content, None, false, LineEnding::Lf),
            queue: VecDeque::new(),
            install: None,
            reset: false,
        }
    }
    pub fn locked(&self) -> bool {
        self.install.is_some() || !self.queue.is_empty() || self.documents.locked()
    }
    pub fn enqueue(&mut self, action: Action) -> Result<(), String> {
        if self.queue.len() >= 16 {
            return Err("File action queue is full; request was not accepted".into());
        }
        self.queue.push_back(action);
        Ok(())
    }
    pub fn route(&mut self, intent: &Intent) -> Option<Result<(), String>> {
        let action = match intent {
            Intent::File(FileIntent::New) => Action::New,
            Intent::File(FileIntent::Open) => Action::OpenDialog,
            Intent::File(FileIntent::Save { as_new }) => Action::Save { save_as: *as_new },
            Intent::File(FileIntent::Close) => Action::Close,
            _ => return None,
        };
        Some(self.enqueue(action))
    }
    fn next_ready(&mut self, p: &Published, accepted: u64, transferring: bool) -> Option<Action> {
        if self.documents.is_idle() && ready(p, accepted, transferring) {
            self.queue.pop_front()
        } else {
            None
        }
    }
    pub fn poll(
        &mut self,
        client: &BufferClient,
        transferring: bool,
        window: &Arc<Window>,
    ) -> Result<bool, String> {
        let p = client.latest();
        if let Some(sequence) = self.install {
            if p.completed_through < sequence {
                return Ok(false);
            }
            self.install = None;
            self.reset = true;
        }
        if !ready(&p, client.barrier(), transferring) {
            return Ok(false);
        }
        let mut effect = self.documents.poll(&p.snapshot, client, window);
        if matches!(effect, Effect::None) {
            if let Some(action) = self.next_ready(&p, client.barrier(), transferring) {
                effect = self.documents.handle(action, &p.snapshot, client, window);
            }
        }
        match effect {
            Effect::None => {}
            Effect::Close => return Ok(true),
            Effect::Install(document) => {
                let sequence = submit(client, Command::Load(document.content.clone()))?;
                self.documents.installed(&document);
                self.install = Some(sequence);
                window.request_redraw();
            }
        }
        Ok(false)
    }
}

fn ready(p: &texted_app::buffer_service::Published, accepted: u64, transferring: bool) -> bool {
    !transferring && p.completed_through >= accepted && p.busy.is_none()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn keyboard_save_waits_for_clipboard_capture_and_its_accepted_paste() {
        use texted_app::commands::EditorCommand;
        let (wake, events) = std::sync::mpsc::sync_channel(1);
        let service = texted_app::buffer_service::BufferService::new(move || {
            let _ = wake.try_send(());
        });
        let client = &service.client;
        let mut flow = FileFlow::new(client.latest().snapshot.content.clone());
        client
            .submit(Command::Edit(EditorCommand::Insert("prior".into())))
            .unwrap_or_else(|_| panic!("rejected"));
        flow.route(&Intent::File(FileIntent::Save { as_new: false }))
            .unwrap()
            .unwrap();
        flow.route(&Intent::File(FileIntent::Close))
            .unwrap()
            .unwrap();
        assert!(flow.locked());
        assert!(flow
            .next_ready(&client.latest(), client.barrier(), true)
            .is_none());
        while client.latest().completed_through < client.barrier() {
            events
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
        }
        let context = client.latest().context();
        client
            .submit(Command::PasteContext {
                context,
                text: "+clipboard".into(),
            })
            .unwrap_or_else(|_| panic!("rejected"));
        while client.latest().completed_through < client.barrier() {
            events
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
        }
        let p = client.latest();
        assert_eq!(p.snapshot.content.to_string(), "prior+clipboard");
        assert!(matches!(
            flow.next_ready(&p, client.barrier(), false),
            Some(Action::Save { save_as: false })
        ));
        assert!(matches!(
            flow.next_ready(&p, client.barrier(), false),
            Some(Action::Close)
        ));
    }
    #[test]
    fn file_barrier_waits_for_worker_failure_and_clipboard_continuations() {
        let service = texted_app::buffer_service::BufferService::new(|| {});
        let mut p = (*service.client.latest()).clone();
        assert!(ready(&p, 0, false));
        p.busy = Some(1);
        assert!(!ready(&p, 1, false));
        p.completed_through = 2;
        assert!(!ready(&p, 2, false));
        p.busy = None;
        assert!(ready(&p, 2, false));
        assert!(!ready(&p, 3, false));
        assert!(!ready(&p, 2, true));
    }
    #[test]
    fn initial_saved_root_is_the_actual_published_root() {
        let service = texted_app::buffer_service::BufferService::new(|| {});
        let p = service.client.latest();
        let flow = FileFlow::new(p.snapshot.content.clone());
        assert!(!flow.documents.dirty(&p.snapshot));
    }
}
