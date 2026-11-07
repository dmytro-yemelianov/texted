//! Per-tab ownership: workers, undo, selection, file lifecycle and viewport travel together.
use crate::{file_flow::FileFlow, search_ui::SearchUi, transfers::Transfers, viewport::Viewport};
use std::sync::Arc;
use texted_app::buffer_service::BufferService;
use winit::window::Window;
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
    pub fn new(window: &Arc<Window>, id: u64) -> Self {
        let w = window.clone();
        let buffer = BufferService::new(move || w.request_redraw());
        let w = window.clone();
        let search = SearchUi::new(move || w.request_redraw());
        let w = window.clone();
        let transfers = Transfers::new(move || w.request_redraw());
        let w = window.clone();
        let viewport = Viewport::new(move || w.request_redraw());
        let files = FileFlow::new(buffer.client.latest().snapshot.content.clone());
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
