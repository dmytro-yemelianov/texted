//! File actions wait for accepted edits and clipboard continuations before dirty checks.
use crate::{
    documents::{Action, Documents, Effect},
    transfers::submit,
};
use std::{collections::VecDeque, sync::Arc};
use texted_app::buffer_service::{BufferClient, Command};
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
        if matches!(effect, Effect::None) && self.documents.is_idle() {
            if let Some(action) = self.queue.pop_front() {
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
