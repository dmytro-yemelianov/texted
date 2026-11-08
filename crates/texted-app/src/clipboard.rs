//! Snapshot-based clipboard work with deadlines; native events never wait for pipes.
use crate::{buffer_service::EditContext, selection::SelectionMode};
use std::{
    io::{Read, Write},
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};
use texted_buffer::BufferSnapshot;
pub const CLIPBOARD_LIMIT: usize = 128 * 1024 * 1024;
pub const CLIPBOARD_DEADLINE: Duration = Duration::from_secs(5);
#[derive(Clone, Copy, Debug)]
pub struct FieldStamp {
    pub field: usize,
    pub serial: u64,
}
#[derive(Clone, Copy, Debug)]
pub enum ClipboardAction {
    Copy {
        context: EditContext,
        cut: bool,
    },
    Paste {
        context: EditContext,
        field: Option<FieldStamp>,
    },
}
pub enum ClipboardPayload {
    Copied { cut: bool },
    Pasted(String),
    NoSelection,
}
pub struct ClipboardDone {
    pub action: ClipboardAction,
    pub result: Result<ClipboardPayload, String>,
}
pub trait ClipboardBackend: Send + Sync + 'static {
    fn read(&self, cancel: &AtomicBool, deadline: Instant) -> Result<String, String>;
    fn write(&self, text: String, cancel: &AtomicBool, deadline: Instant) -> Result<(), String>;
}
pub struct SystemClipboard;
/// pbcopy/pbpaste pick their text encoding from the locale, and apps opened from Finder have none.
fn utf8_locale(command: &mut Command) -> &mut Command {
    command.env_remove("LC_ALL").env("LC_CTYPE", "UTF-8")
}
fn subprocess(
    input: Option<String>,
    cancel: &AtomicBool,
    deadline: Instant,
) -> Result<String, String> {
    let writing = input.is_some();
    let mut command = Command::new(if writing {
        "/usr/bin/pbcopy"
    } else {
        "/usr/bin/pbpaste"
    });
    utf8_locale(&mut command)
        .stdin(if writing {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(if writing {
            Stdio::null()
        } else {
            Stdio::piped()
        })
        .stderr(Stdio::null());
    let child = command
        .spawn()
        .map_err(|e| format!("Could not start clipboard helper: {e}"))?;
    communicate(child, input, cancel, deadline)
}
fn communicate(
    mut child: Child,
    input: Option<String>,
    cancel: &AtomicBool,
    deadline: Instant,
) -> Result<String, String> {
    let stdin = child.stdin.take();
    let stdout = child.stdout.take();
    let (tx, rx) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let result = if let Some(text) = input {
            stdin
                .ok_or_else(|| "Clipboard input pipe unavailable".to_owned())
                .and_then(|mut pipe| {
                    pipe.write_all(text.as_bytes())
                        .map(|()| String::new())
                        .map_err(|e| e.to_string())
                })
        } else {
            stdout
                .ok_or_else(|| "Clipboard output pipe unavailable".to_owned())
                .and_then(|pipe| {
                    let mut bytes = Vec::new();
                    pipe.take((CLIPBOARD_LIMIT + 1) as u64)
                        .read_to_end(&mut bytes)
                        .map_err(|e| e.to_string())?;
                    if bytes.len() > CLIPBOARD_LIMIT {
                        return Err("Clipboard exceeds 128 MiB".into());
                    }
                    String::from_utf8(bytes).map_err(|_| "Clipboard is not valid UTF-8 text".into())
                })
        };
        let _ = tx.send(result);
    });
    let mut status = None;
    let mut output = None;
    loop {
        if cancel.load(Ordering::Acquire) || Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(if cancel.load(Ordering::Acquire) {
                "Clipboard operation cancelled"
            } else {
                "Clipboard operation timed out"
            }
            .into());
        }
        if output.is_none() {
            match rx.try_recv() {
                Ok(result) => output = Some(result),
                Err(mpsc::TryRecvError::Disconnected) => {
                    output = Some(Err("Clipboard pipe worker stopped".into()))
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if output.as_ref().is_some_and(Result::is_err) {
            let _ = child.kill();
            let _ = child.wait();
            return output.unwrap();
        }
        if status.is_none() {
            match child.try_wait() {
                Ok(value) => status = value,
                Err(e) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(e.to_string());
                }
            }
        }
        if let Some(status) = status {
            if !status.success() {
                return Err(format!("Clipboard helper failed: {status}"));
            }
            if let Some(output) = output {
                return output;
            }
        }
        thread::sleep(Duration::from_millis(2));
    }
}
impl ClipboardBackend for SystemClipboard {
    fn read(&self, cancel: &AtomicBool, deadline: Instant) -> Result<String, String> {
        subprocess(None, cancel, deadline)
    }
    fn write(&self, text: String, cancel: &AtomicBool, deadline: Instant) -> Result<(), String> {
        subprocess(Some(text), cancel, deadline).map(|_| ())
    }
}
fn append(out: &mut String, text: &str, cancelled: &impl Fn() -> bool) -> Result<(), String> {
    if out.len().saturating_add(text.len()) > CLIPBOARD_LIMIT {
        return Err("Selected text exceeds 128 MiB".into());
    }
    let mut rest = text;
    while !rest.is_empty() {
        if cancelled() {
            return Err("Clipboard operation cancelled".into());
        }
        let mut end = rest.len().min(4096);
        while !rest.is_char_boundary(end) {
            end -= 1;
        }
        out.push_str(&rest[..end]);
        rest = &rest[end..];
    }
    Ok(())
}
fn extract(
    snapshot: &BufferSnapshot,
    context: EditContext,
    cancelled: &impl Fn() -> bool,
) -> Result<Option<String>, String> {
    let selection = context.selection;
    let mut out = String::new();
    match selection.mode {
        SelectionMode::None => return Ok(None),
        SelectionMode::Stream => {
            let (a, b) = selection.offsets(snapshot);
            if a == b {
                return Ok(None);
            }
            if b - a > CLIPBOARD_LIMIT {
                return Err("Selected text exceeds 128 MiB".into());
            }
            out.reserve(b - a);
            for chunk in snapshot.content.byte_slice(a..b).chunks() {
                append(&mut out, chunk, cancelled)?;
            }
        }
        SelectionMode::Block => {
            let (a, b, c, d) = selection.block();
            if c == d {
                return Ok(None);
            }
            if b.saturating_sub(a)
                .saturating_add(1)
                .checked_mul(d - c)
                .is_none_or(|n| n > CLIPBOARD_LIMIT)
            {
                return Err("Block selection exceeds clipboard capacity".into());
            }
            for line in a..=b.min(snapshot.total_lines - 1) {
                if line > a {
                    append(&mut out, "\n", cancelled)?;
                }
                let text = snapshot.line(line);
                let start = c.min(text.len_chars());
                let end = d.min(text.len_chars());
                for chunk in text.slice(start..end).chunks() {
                    append(&mut out, chunk, cancelled)?;
                }
                let mut padding = (d - c).saturating_sub(end - start);
                let spaces = " ".repeat(4096);
                while padding > 0 {
                    let take = padding.min(spaces.len());
                    append(&mut out, &spaces[..take], cancelled)?;
                    padding -= take;
                }
            }
        }
    }
    Ok(Some(out))
}
pub struct Clipboard {
    backend: Arc<dyn ClipboardBackend>,
    pending: Option<(ClipboardAction, Receiver<ClipboardDone>)>,
    cancel: Arc<AtomicBool>,
    wake: Arc<dyn Fn() + Send + Sync>,
}
impl Clipboard {
    pub fn new(
        backend: Arc<dyn ClipboardBackend>,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        Self {
            backend,
            pending: None,
            cancel: Arc::new(AtomicBool::new(false)),
            wake: Arc::new(wake),
        }
    }
    pub fn busy(&self) -> bool {
        self.pending.is_some()
    }
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Release);
    }
    pub fn start(
        &mut self,
        action: ClipboardAction,
        snapshot: BufferSnapshot,
    ) -> Result<(), String> {
        if self.busy() {
            return Err("Clipboard is busy; the new request was not accepted".into());
        }
        let (tx, rx) = mpsc::sync_channel(1);
        self.pending = Some((action, rx));
        self.cancel = Arc::new(AtomicBool::new(false));
        let token = self.cancel.clone();
        let backend = self.backend.clone();
        let wake = self.wake.clone();
        thread::spawn(move || {
            let deadline = Instant::now() + CLIPBOARD_DEADLINE;
            let cancelled = || token.load(Ordering::Acquire) || Instant::now() >= deadline;
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match action {
                ClipboardAction::Copy { context, cut } => {
                    match extract(&snapshot, context, &cancelled)? {
                        Some(text) => backend
                            .write(text, &token, deadline)
                            .map(|()| ClipboardPayload::Copied { cut }),
                        None => Ok(ClipboardPayload::NoSelection),
                    }
                }
                ClipboardAction::Paste { .. } => backend.read(&token, deadline).and_then(|text| {
                    if text.len() > CLIPBOARD_LIMIT {
                        Err("Clipboard exceeds 128 MiB".into())
                    } else {
                        Ok(ClipboardPayload::Pasted(text))
                    }
                }),
            }))
            .unwrap_or_else(|_| Err("Clipboard worker failed".into()));
            let result = if cancelled() {
                Err("Clipboard operation cancelled or timed out".into())
            } else {
                result
            };
            let _ = tx.send(ClipboardDone { action, result });
            wake();
        });
        Ok(())
    }
    pub fn poll(&mut self) -> Option<ClipboardDone> {
        let result = match self.pending.as_ref()?.1.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return None,
            Err(mpsc::TryRecvError::Disconnected) => ClipboardDone {
                action: self.pending.as_ref()?.0,
                result: Err("Clipboard worker stopped".into()),
            },
        };
        self.pending = None;
        Some(result)
    }
}
impl Drop for Clipboard {
    fn drop(&mut self) {
        self.cancel();
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer_service::EditContext;
    use crate::selection::SelectionState;
    use std::sync::Mutex;
    struct Fake {
        writes: Mutex<Vec<String>>,
        fail: bool,
    }
    impl ClipboardBackend for Fake {
        fn read(&self, _: &AtomicBool, _: Instant) -> Result<String, String> {
            Ok("ї".into())
        }
        fn write(&self, text: String, _: &AtomicBool, _: Instant) -> Result<(), String> {
            if self.fail {
                Err("injected write failure".into())
            } else {
                self.writes.lock().unwrap().push(text);
                Ok(())
            }
        }
    }
    fn context(snapshot: &BufferSnapshot) -> EditContext {
        EditContext {
            document: 0,
            revision: snapshot.revision,
            cursor: snapshot.cursor_offset,
            selection: SelectionState {
                mode: SelectionMode::Stream,
                anchor_line: 0,
                anchor_col: 0,
                active_line: 0,
                active_col: 2,
            },
        }
    }
    #[cfg(target_os = "macos")]
    #[test]
    fn pasteboard_helpers_exchange_utf8_without_an_inherited_locale() {
        // The ruler pasteboard keeps this test away from the user's clipboard.
        fn pasteboard(mut command: Command, input: Option<&str>) -> String {
            command
                .args(["-pboard", "ruler"])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped());
            let mut child = command.spawn().unwrap();
            let stdin = child.stdin.take().unwrap();
            if let Some(text) = input {
                (&stdin).write_all(text.as_bytes()).unwrap();
            }
            drop(stdin);
            String::from_utf8_lossy(&child.wait_with_output().unwrap().stdout).into_owned()
        }
        // Another app stores real Unicode text on the pasteboard.
        let foreign = |program| {
            let mut command = Command::new(program);
            command.env_clear().env("LC_CTYPE", "UTF-8");
            command
        };
        // Apps opened from Finder inherit no usable locale.
        let ours = |program| {
            let mut command = Command::new(program);
            command.env_clear().env("LC_ALL", "C");
            utf8_locale(&mut command);
            command
        };
        pasteboard(foreign("/usr/bin/pbcopy"), Some("їжак"));
        assert_eq!(pasteboard(ours("/usr/bin/pbpaste"), None), "їжак");
        pasteboard(ours("/usr/bin/pbcopy"), Some("ґрунт"));
        assert_eq!(pasteboard(foreign("/usr/bin/pbpaste"), None), "ґрунт");
    }
    #[test]
    fn an_expired_pipe_operation_terminates_and_reaps_its_child() {
        let child = Command::new("/bin/sleep")
            .arg("30")
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let now = Instant::now();
        assert!(communicate(child, None, &AtomicBool::new(false), now)
            .unwrap_err()
            .contains("timed out"));
        assert!(now.elapsed() < Duration::from_secs(5));
    }
    struct Paused {
        ready: mpsc::SyncSender<()>,
        release: Mutex<Receiver<()>>,
    }
    impl ClipboardBackend for Paused {
        fn read(&self, _: &AtomicBool, _: Instant) -> Result<String, String> {
            self.ready.send(()).unwrap();
            self.release.lock().unwrap().recv().unwrap();
            Ok("late".into())
        }
        fn write(&self, _: String, c: &AtomicBool, d: Instant) -> Result<(), String> {
            self.read(c, d).map(|_| ())
        }
    }
    #[test]
    fn slow_clipboard_is_bounded_and_late_success_cannot_complete_a_cancelled_cut() {
        let (ready, started) = mpsc::sync_channel(1);
        let (release, wait) = mpsc::sync_channel(1);
        let (wake, completed) = mpsc::sync_channel(1);
        let mut clipboard = Clipboard::new(
            Arc::new(Paused {
                ready,
                release: Mutex::new(wait),
            }),
            move || {
                let _ = wake.try_send(());
            },
        );
        let snapshot = texted_buffer::TextBuffer::from_string("їжак".into()).create_snapshot();
        let action = ClipboardAction::Copy {
            context: context(&snapshot),
            cut: true,
        };
        clipboard.start(action, snapshot.clone()).unwrap();
        started.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(clipboard.start(action, snapshot).is_err());
        assert!(clipboard.poll().is_none());
        clipboard.cancel();
        release.send(()).unwrap();
        completed.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(clipboard.poll().unwrap().result.is_err());
        assert!(!clipboard.busy());
    }
    #[test]
    fn cut_reports_write_failure_without_mutating_the_source() {
        let snapshot = texted_buffer::TextBuffer::from_string("їжак".into()).create_snapshot();
        let backend = Arc::new(Fake {
            writes: Mutex::new(Vec::new()),
            fail: true,
        });
        let (wake, ready) = mpsc::sync_channel(1);
        let mut c = Clipboard::new(backend, move || {
            let _ = wake.try_send(());
        });
        c.start(
            ClipboardAction::Copy {
                context: context(&snapshot),
                cut: true,
            },
            snapshot.clone(),
        )
        .unwrap();
        ready.recv_timeout(Duration::from_secs(5)).unwrap();
        let result = c.poll().unwrap();
        assert!(result.result.is_err());
        assert_eq!(snapshot.content.to_string(), "їжак");
    }
    #[test]
    fn block_extraction_preserves_padding_and_utf8() {
        let snap = texted_buffer::TextBuffer::from_string("їжак\nx".into()).create_snapshot();
        let mut context = context(&snap);
        context.selection = SelectionState {
            mode: SelectionMode::Block,
            anchor_line: 0,
            active_line: 1,
            anchor_col: 1,
            active_col: 4,
        };
        assert_eq!(
            extract(&snap, context, &|| false).unwrap().unwrap(),
            "жак\n   "
        );
        assert!(extract(&snap, context, &|| true).is_err());
    }
}
