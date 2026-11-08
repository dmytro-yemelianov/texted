//! Keeps open tabs and unsaved drafts between runs, so quitting never asks to save.
//! One running instance owns the folder; others fall back to asking at quit.
use ropey::Rope;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    path::PathBuf,
};
use texted_io::{DiskStamp, IoManager, LineEnding, TextEncoding};
const VERSION: u32 = 1;
const SESSION: &str = "session.json";
#[derive(Serialize, Deserialize)]
struct SessionFile {
    version: u32,
    active: usize,
    tabs: Vec<SavedTab>,
}
#[derive(Serialize, Deserialize)]
struct SavedTab {
    path: Option<PathBuf>,
    draft: Option<String>,
    encoding: String,
    bom: bool,
    crlf: bool,
    line: usize,
    column: usize,
    disk: Option<SavedStamp>,
}
#[derive(Serialize, Deserialize)]
struct SavedStamp {
    modified: Option<(u64, u32)>,
    len: u64,
    inode: u64,
}
/// Drafts are only ever `draft-N.txt` inside the session folder.
fn is_draft_name(name: &str) -> bool {
    name.strip_prefix("draft-")
        .and_then(|n| n.strip_suffix(".txt"))
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}
/// One open tab as the app hands it over at quit.
pub(crate) struct TabState {
    pub path: Option<PathBuf>,
    /// Unsaved text; `None` when the tab matches its file.
    pub draft: Option<Rope>,
    pub encoding: TextEncoding,
    pub bom: bool,
    pub ending: LineEnding,
    pub line: usize,
    pub column: usize,
    pub disk: Option<DiskStamp>,
}
/// A tab read back at start; `draft` names the file that holds its unsaved text.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RestoredTab {
    pub path: Option<PathBuf>,
    pub draft: Option<PathBuf>,
    pub encoding: TextEncoding,
    pub bom: bool,
    pub ending: LineEnding,
    pub line: usize,
    pub column: usize,
    pub disk: Option<DiskStamp>,
}
#[derive(Debug, Default)]
pub(crate) struct Restored {
    pub tabs: Vec<RestoredTab>,
    pub active: usize,
}
pub(crate) struct SessionStore {
    dir: PathBuf,
    _lock: Option<File>,
}
impl SessionStore {
    pub fn open(dir: PathBuf) -> Result<Self, String> {
        fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(dir.join("lock"))
            .map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            // The lock lasts as long as this file stays open, and ends with the process.
            if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
                return Err("another Texted is already keeping the session".into());
            }
        }
        Ok(Self {
            dir,
            _lock: Some(lock),
        })
    }
    pub fn load(&self) -> Result<Restored, String> {
        let text = match fs::read_to_string(self.dir.join(SESSION)) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Restored::default()),
            Err(e) => return Err(e.to_string()),
        };
        let file: SessionFile = serde_json::from_str(&text).map_err(|e| e.to_string())?;
        if file.version != VERSION {
            return Err(format!("unsupported session version {}", file.version));
        }
        let tabs = file
            .tabs
            .into_iter()
            .map(|tab| {
                let draft = match tab.draft {
                    Some(name) if is_draft_name(&name) => Some(self.dir.join(name)),
                    Some(name) => return Err(format!("unexpected draft file {name}")),
                    None => None,
                };
                let encoding = TextEncoding::ALL
                    .into_iter()
                    .find(|e| e.name() == tab.encoding)
                    .ok_or_else(|| format!("unknown encoding {}", tab.encoding))?;
                Ok(RestoredTab {
                    path: tab.path,
                    draft,
                    encoding,
                    bom: tab.bom,
                    ending: if tab.crlf {
                        LineEnding::Crlf
                    } else {
                        LineEnding::Lf
                    },
                    line: tab.line,
                    column: tab.column,
                    disk: tab
                        .disk
                        .map(|d| DiskStamp::from_parts(d.modified, d.len, d.inode)),
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok(Restored {
            active: file.active.min(tabs.len().saturating_sub(1)),
            tabs,
        })
    }
    pub fn save(&self, tabs: &[TabState], active: usize) -> Result<(), String> {
        let mut saved = Vec::with_capacity(tabs.len());
        for (index, tab) in tabs.iter().enumerate() {
            let draft = match &tab.draft {
                Some(text) => {
                    let name = format!("draft-{index}.txt");
                    IoManager::save_rope(self.dir.join(&name), text, false, LineEnding::Lf)
                        .map_err(|e| e.to_string())?;
                    Some(name)
                }
                None => None,
            };
            saved.push(SavedTab {
                path: tab.path.clone(),
                draft,
                encoding: tab.encoding.name().into(),
                bom: tab.bom,
                crlf: tab.ending == LineEnding::Crlf,
                line: tab.line,
                column: tab.column,
                disk: tab.disk.map(|d| {
                    let (modified, len, inode) = d.to_parts();
                    SavedStamp {
                        modified,
                        len,
                        inode,
                    }
                }),
            });
        }
        let kept: HashSet<String> = saved.iter().filter_map(|t| t.draft.clone()).collect();
        let json = serde_json::to_string_pretty(&SessionFile {
            version: VERSION,
            active,
            tabs: saved,
        })
        .map_err(|e| e.to_string())?;
        IoManager::save_atomic(self.dir.join(SESSION), &json, false, LineEnding::Lf)
            .map_err(|e| e.to_string())?;
        // Remove old drafts only once the saved session no longer names them.
        for entry in fs::read_dir(&self.dir)
            .map_err(|e| e.to_string())?
            .flatten()
        {
            let name = entry.file_name().to_string_lossy().into_owned();
            if is_draft_name(&name) && !kept.contains(&name) {
                let _ = fs::remove_file(entry.path());
            }
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    fn folder(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("texted-session-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }
    fn drafts(dir: &std::path::Path) -> Vec<String> {
        let mut names: Vec<_> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with("draft-"))
            .collect();
        names.sort();
        names
    }
    fn tab(path: &str, draft: Option<&str>) -> TabState {
        TabState {
            path: Some(path.into()),
            draft: draft.map(Rope::from_str),
            encoding: TextEncoding::Utf8,
            bom: false,
            ending: LineEnding::Lf,
            line: 0,
            column: 0,
            disk: None,
        }
    }
    #[test]
    fn quit_state_round_trips_with_verbatim_drafts() {
        let dir = folder("round");
        let store = SessionStore::open(dir.clone()).unwrap();
        let stamp = DiskStamp::from_parts(Some((1_700_000_000, 5)), 42, 7);
        store
            .save(
                &[
                    TabState {
                        encoding: TextEncoding::Windows1251,
                        ending: LineEnding::Crlf,
                        line: 1,
                        column: 3,
                        disk: Some(stamp),
                        ..tab("/docs/notes.txt", Some("mine\r\nчернетка"))
                    },
                    TabState {
                        bom: true,
                        line: 9,
                        ..tab("/docs/clean.txt", None)
                    },
                    TabState {
                        path: None,
                        ..tab("", Some("untitled"))
                    },
                ],
                1,
            )
            .unwrap();
        let restored = store.load().unwrap();
        assert_eq!(restored.active, 1);
        assert_eq!(restored.tabs.len(), 3);
        let notes = &restored.tabs[0];
        assert_eq!(notes.path, Some("/docs/notes.txt".into()));
        let text = fs::read_to_string(notes.draft.as_ref().unwrap()).unwrap();
        assert_eq!(text, "mine\r\nчернетка");
        assert_eq!(
            (
                notes.encoding,
                notes.ending,
                notes.line,
                notes.column,
                notes.disk
            ),
            (
                TextEncoding::Windows1251,
                LineEnding::Crlf,
                1,
                3,
                Some(stamp)
            )
        );
        let clean = &restored.tabs[1];
        assert_eq!(
            (clean.draft.as_ref(), clean.bom, clean.line),
            (None, true, 9)
        );
        assert_eq!(restored.tabs[2].path, None);
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn only_one_running_instance_holds_the_session() {
        let dir = folder("lock");
        let first = SessionStore::open(dir.clone()).unwrap();
        assert!(SessionStore::open(dir.clone()).is_err());
        drop(first);
        assert!(SessionStore::open(dir.clone()).is_ok());
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn drafts_that_are_no_longer_needed_are_removed() {
        let dir = folder("prune");
        let store = SessionStore::open(dir.clone()).unwrap();
        store
            .save(&[tab("/a", Some("a")), tab("/b", Some("b"))], 0)
            .unwrap();
        assert_eq!(drafts(&dir).len(), 2);
        store.save(&[tab("/b", Some("b"))], 0).unwrap();
        assert_eq!(drafts(&dir).len(), 1);
        store.save(&[], 0).unwrap();
        assert!(drafts(&dir).is_empty());
        assert!(store.load().unwrap().tabs.is_empty());
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn a_session_naming_files_outside_its_folder_is_refused() {
        let dir = folder("escape");
        let store = SessionStore::open(dir.clone()).unwrap();
        store.save(&[tab("/a", Some("a"))], 0).unwrap();
        let json = fs::read_to_string(dir.join("session.json")).unwrap();
        let name = drafts(&dir).remove(0);
        fs::write(
            dir.join("session.json"),
            json.replace(&name, "../escape.txt"),
        )
        .unwrap();
        assert!(store.load().is_err());
        fs::remove_dir_all(dir).unwrap();
    }
}
