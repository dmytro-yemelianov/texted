//! Ordered buffer service, isolated from native events and rendering.
use std::thread;
use texted_buffer::{BufferSnapshot, TextBuffer};
use texted_kernel::{ChannelClient, ChannelEvent, ChannelPair, CH_BUFFER};
/// Messages sent to the Buffer Server
#[derive(Debug)]
pub enum BufferRequest {
    Shutdown,
    BreakUndoGroup,
    LoadDocument(ropey::Rope),
    Insert {
        offset: usize,
        text: String,
    },
    DeleteRange {
        start: usize,
        end: usize,
    },
    GetSnapshot,
    Undo,
    Redo,
    MoveLeft,
    MoveRight,
    MoveUp,
    MoveDown,
    MoveHome,
    MoveEnd,
    MoveWordLeft,
    MoveWordRight,
    Backspace,
    DeleteForward,
    DeleteWordLeft,
    DeleteWordRight,
    DeleteToLineStart,
    DeleteToLineEnd,
    SetCursorOffset(usize),
    BlockInsert {
        start_line: usize,
        end_line: usize,
        col: usize,
        text: String,
    },
    BlockDelete {
        start_line: usize,
        end_line: usize,
        start_col: usize,
        end_col: usize,
    },
    BlockExtract {
        start_line: usize,
        end_line: usize,
        start_col: usize,
        end_col: usize,
    },
    CommitReplacement {
        revision: u64,
        cursor: usize,
        edited: BufferSnapshot,
    },
}

/// Responses from the Buffer Server
#[derive(Debug)]
pub enum BufferResponse {
    Snapshot(BufferSnapshot),
    Text(String),
    Committed(bool),
    Success,
}

pub struct BufferHost {
    pub client: ChannelClient<BufferRequest, BufferResponse>,
    thread: Option<thread::JoinHandle<()>>,
}
impl BufferHost {
    pub fn spawn(init_text: ropey::Rope) -> Self {
        let buffer_channel: ChannelPair<BufferRequest, BufferResponse> =
            ChannelPair::new(CH_BUFFER);
        let buffer_client = buffer_channel.connect();
        let buffer_thread = thread::Builder::new()
            .name("server-buffer".to_string())
            .spawn(move || {
                let mut buffer = TextBuffer::from_rope(init_text);
                let server = buffer_channel.server;

                while let Ok(event) = server.receive() {
                    match event {
                        ChannelEvent::Pulse(_) => {}
                        ChannelEvent::Message(req) => match req.payload {
                            BufferRequest::BreakUndoGroup => {
                                buffer.break_undo_group();
                                req.reply(BufferResponse::Success);
                            }
                            BufferRequest::LoadDocument(ref text) => {
                                buffer.load_document(text.clone());
                                req.reply(BufferResponse::Success);
                            }
                            BufferRequest::Shutdown => {
                                req.reply(BufferResponse::Success);
                                break;
                            }
                            BufferRequest::Insert { offset, ref text } => {
                                let insert_pos = if offset == usize::MAX {
                                    buffer.cursor_offset()
                                } else {
                                    offset
                                };
                                buffer.insert(insert_pos, text);
                                req.reply(BufferResponse::Success);
                            }
                            BufferRequest::DeleteRange { start, end } => {
                                buffer.delete(start, end);
                                req.reply(BufferResponse::Success);
                            }
                            BufferRequest::GetSnapshot => {
                                let snap = buffer.create_snapshot();
                                req.reply(BufferResponse::Snapshot(snap));
                            }
                            BufferRequest::Undo => {
                                buffer.undo();
                                req.reply(BufferResponse::Success);
                            }
                            BufferRequest::Redo => {
                                buffer.redo();
                                req.reply(BufferResponse::Success);
                            }
                            BufferRequest::MoveLeft => {
                                buffer.move_cursor_left();
                                req.reply(BufferResponse::Success);
                            }
                            BufferRequest::MoveRight => {
                                buffer.move_cursor_right();
                                req.reply(BufferResponse::Success);
                            }
                            BufferRequest::MoveWordLeft => {
                                buffer.move_word_left();
                                req.reply(BufferResponse::Success);
                            }
                            BufferRequest::MoveWordRight => {
                                buffer.move_word_right();
                                req.reply(BufferResponse::Success);
                            }
                            BufferRequest::MoveUp => {
                                buffer.move_cursor_up();
                                req.reply(BufferResponse::Success);
                            }
                            BufferRequest::MoveDown => {
                                buffer.move_cursor_down();
                                req.reply(BufferResponse::Success);
                            }
                            BufferRequest::MoveHome => {
                                buffer.move_cursor_home();
                                req.reply(BufferResponse::Success);
                            }
                            BufferRequest::MoveEnd => {
                                buffer.move_cursor_end();
                                req.reply(BufferResponse::Success);
                            }
                            BufferRequest::Backspace => {
                                buffer.backspace();
                                req.reply(BufferResponse::Success);
                            }
                            BufferRequest::DeleteForward => {
                                buffer.delete_forward();
                                req.reply(BufferResponse::Success);
                            }
                            BufferRequest::DeleteWordLeft => {
                                buffer.delete_word_left();
                                req.reply(BufferResponse::Success);
                            }
                            BufferRequest::DeleteWordRight => {
                                buffer.delete_word_right();
                                req.reply(BufferResponse::Success);
                            }
                            BufferRequest::DeleteToLineStart => {
                                buffer.delete_to_line_start();
                                req.reply(BufferResponse::Success);
                            }
                            BufferRequest::DeleteToLineEnd => {
                                buffer.delete_to_line_end();
                                req.reply(BufferResponse::Success);
                            }
                            BufferRequest::SetCursorOffset(offset) => {
                                buffer.set_cursor_offset(offset);
                                req.reply(BufferResponse::Success);
                            }
                            BufferRequest::BlockInsert {
                                start_line,
                                end_line,
                                col,
                                ref text,
                            } => {
                                buffer.block_insert(start_line, end_line, col, text);
                                req.reply(BufferResponse::Success);
                            }
                            BufferRequest::BlockDelete {
                                start_line,
                                end_line,
                                start_col,
                                end_col,
                            } => {
                                buffer.block_delete(start_line, end_line, start_col, end_col);
                                req.reply(BufferResponse::Success);
                            }
                            BufferRequest::BlockExtract {
                                start_line,
                                end_line,
                                start_col,
                                end_col,
                            } => {
                                let extracted =
                                    buffer.block_extract(start_line, end_line, start_col, end_col);
                                req.reply(BufferResponse::Text(extracted));
                            }
                            BufferRequest::CommitReplacement {
                                revision,
                                cursor,
                                ref edited,
                            } => {
                                let committed =
                                    buffer.commit_snapshot(revision, cursor, edited.clone());
                                req.reply(BufferResponse::Committed(committed));
                            }
                        },
                    }
                }
            })
            .expect("Failed to spawn buffer server");

        Self {
            client: buffer_client,
            thread: Some(buffer_thread),
        }
    }
    pub fn shutdown(&mut self) {
        if let Some(thread) = self.thread.take() {
            let _ = self.client.send(BufferRequest::Shutdown);
            if thread.join().is_err() {
                eprintln!("Buffer worker stopped unexpectedly");
            }
        }
    }
}
impl Drop for BufferHost {
    fn drop(&mut self) {
        self.shutdown();
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn extracted_host_keeps_history_and_document_revisions_ordered() {
        let host = BufferHost::spawn(ropey::Rope::new());
        for text in ["a", "ї"] {
            host.client
                .send(BufferRequest::Insert {
                    offset: usize::MAX,
                    text: text.into(),
                })
                .unwrap();
        }
        let BufferResponse::Snapshot(old) = host.client.send(BufferRequest::GetSnapshot).unwrap()
        else {
            panic!()
        };
        assert_eq!(old.content.to_string(), "aї");
        host.client.send(BufferRequest::Undo).unwrap();
        let BufferResponse::Snapshot(empty) = host.client.send(BufferRequest::GetSnapshot).unwrap()
        else {
            panic!()
        };
        assert_eq!(empty.total_bytes, 0);
        host.client
            .send(BufferRequest::LoadDocument(ropey::Rope::from_str("new")))
            .unwrap();
        let BufferResponse::Committed(applied) = host
            .client
            .send(BufferRequest::CommitReplacement {
                revision: old.revision,
                cursor: old.cursor_offset,
                edited: old,
            })
            .unwrap()
        else {
            panic!()
        };
        assert!(!applied);
    }
}
