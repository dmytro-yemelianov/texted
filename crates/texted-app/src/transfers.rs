//! Clipboard capture barriers and completion fencing, independent of the native event router.
use crate::search_ui::SearchUi;
use std::sync::Arc;
use texted_app::{
    buffer_service::{BufferClient, Command},
    clipboard::{Clipboard, ClipboardAction, ClipboardPayload, FieldStamp, SystemClipboard},
};
#[derive(Clone, Copy)]
enum Request {
    Copy { cut: bool },
    Paste { field: Option<FieldStamp> },
}
pub(crate) struct Transfers {
    clipboard: Clipboard,
    capture: Option<(u64, Request)>,
}
impl Transfers {
    pub fn new(wake: impl Fn() + Send + Sync + 'static) -> Self {
        Self {
            clipboard: Clipboard::new(Arc::new(SystemClipboard), wake),
            capture: None,
        }
    }
    pub fn busy(&self) -> bool {
        self.capture.is_some() || self.clipboard.busy()
    }
    pub fn capturing(&self) -> bool {
        self.capture.is_some()
    }
    pub fn cancel(&mut self) {
        self.capture = None;
        self.clipboard.cancel();
    }
    fn request(&mut self, client: &BufferClient, request: Request) -> Result<(), String> {
        if self.busy() {
            return Err("Clipboard is busy; request was not accepted".into());
        }
        self.capture = Some((client.barrier(), request));
        Ok(())
    }
    pub fn copy(&mut self, client: &BufferClient, cut: bool) -> Result<(), String> {
        self.request(client, Request::Copy { cut })
    }
    pub fn paste(
        &mut self,
        client: &BufferClient,
        field: Option<FieldStamp>,
    ) -> Result<(), String> {
        self.request(client, Request::Paste { field })
    }
    pub fn poll(&mut self, client: &BufferClient, search: &mut SearchUi) -> Result<(), String> {
        let published = client.latest();
        if let Some((barrier, request)) = self.capture {
            if published.completed_through >= barrier {
                self.capture = None;
                let context = published.context();
                let action = match request {
                    Request::Copy { cut } => ClipboardAction::Copy { context, cut },
                    Request::Paste { field } => ClipboardAction::Paste { context, field },
                };
                self.clipboard.start(action, published.snapshot.clone())?;
            }
        }
        if let Some(done) = self.clipboard.poll() {
            match (done.action, done.result?) {
                (ClipboardAction::Copy { context, .. }, ClipboardPayload::Copied { cut: true }) => {
                    submit(client, Command::DeleteContext(context))?;
                }
                (
                    ClipboardAction::Paste {
                        context,
                        field: None,
                    },
                    ClipboardPayload::Pasted(text),
                ) => {
                    submit(client, Command::PasteContext { context, text })?;
                }
                (
                    ClipboardAction::Paste {
                        context,
                        field: Some(field),
                    },
                    ClipboardPayload::Pasted(text),
                ) => {
                    if context != published.context()
                        || published.completed_through < client.barrier()
                    {
                        return Err("Paste skipped: document or edit context changed".into());
                    }
                    search.paste(field, text, published.snapshot.clone())?;
                }
                _ => {}
            }
        }
        Ok(())
    }
}
pub(crate) fn submit(client: &BufferClient, command: Command) -> Result<u64, String> {
    client.submit(command).map_err(|e| match e {
        texted_app::buffer_service::SubmitError::Busy(_) => {
            "Buffer queue is full; request was not accepted".into()
        }
        texted_app::buffer_service::SubmitError::Closed(_) => {
            "Buffer service stopped; request was not accepted".into()
        }
    })
}
