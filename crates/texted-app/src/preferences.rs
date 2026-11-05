//! Bounded, coalescing preference writes. Disk I/O never runs in an input/redraw callback.
use crate::appearance::Appearance;
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Condvar, Mutex},
    thread,
};
pub const MAX_SETTINGS_BYTES: usize = 64 * 1024;
pub fn default_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("TEXTED_SETTINGS_PATH") {
        return Some(path.into());
    }
    std::env::var_os("HOME")
        .map(|home| PathBuf::from(home).join("Library/Application Support/Texted/appearance.json"))
}
pub fn load(path: &Path) -> Result<Appearance, String> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Appearance::default()),
        Err(e) => return Err(e.to_string()),
    };
    let mut bytes = Vec::new();
    file.take((MAX_SETTINGS_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > MAX_SETTINGS_BYTES {
        return Err("Appearance settings exceed 64 KiB".into());
    }
    let mut settings: Appearance = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    settings.normalize()?;
    Ok(settings)
}
fn write(path: &Path, sequence: u64, settings: &Appearance) -> Result<(), String> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let name = path
        .file_name()
        .ok_or("Preferences path has no filename")?
        .to_string_lossy();
    let temp = parent.join(format!(".{name}.{}.{sequence}.tmp", std::process::id()));
    let result = (|| {
        let bytes = serde_json::to_vec_pretty(settings).map_err(|e| e.to_string())?;
        if bytes.len() > MAX_SETTINGS_BYTES {
            return Err("Appearance settings exceed 64 KiB".into());
        }
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temp).map_err(|e| e.to_string())?;
        file.write_all(&bytes)
            .and_then(|()| file.write_all(b"\n"))
            .and_then(|()| file.sync_all())
            .map_err(|e| e.to_string())?;
        fs::rename(&temp, path).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        File::open(parent)
            .and_then(|f| f.sync_all())
            .map_err(|e| e.to_string())?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}
#[derive(Default)]
struct Slots {
    pending: Option<(u64, Appearance)>,
    result: Option<(u64, Result<(), String>)>,
    stop: bool,
}
struct Shared {
    slots: Mutex<Slots>,
    ready: Condvar,
}
pub struct Preferences {
    shared: Arc<Shared>,
    worker: Option<thread::JoinHandle<()>>,
    sequence: u64,
}
impl Preferences {
    pub fn new(path: PathBuf, wake: impl Fn() + Send + 'static) -> Self {
        let shared = Arc::new(Shared {
            slots: Mutex::new(Slots::default()),
            ready: Condvar::new(),
        });
        let state = shared.clone();
        let worker = thread::spawn(move || loop {
            let job = {
                let mut slots = state.slots.lock().unwrap_or_else(|p| p.into_inner());
                while slots.pending.is_none() && !slots.stop {
                    slots = state.ready.wait(slots).unwrap_or_else(|p| p.into_inner());
                }
                match slots.pending.take() {
                    Some(job) => job,
                    None => break,
                }
            };
            let result = write(&path, job.0, &job.1);
            state.slots.lock().unwrap_or_else(|p| p.into_inner()).result = Some((job.0, result));
            wake();
        });
        Self {
            shared,
            worker: Some(worker),
            sequence: 0,
        }
    }
    pub fn save(&mut self, mut settings: Appearance) -> Result<(), String> {
        settings.normalize()?;
        self.sequence += 1;
        self.shared
            .slots
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .pending = Some((self.sequence, settings));
        self.shared.ready.notify_one();
        Ok(())
    }
    pub fn poll(&mut self) -> Option<Result<(), String>> {
        let (sequence, result) = self
            .shared
            .slots
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .result
            .take()?;
        (sequence == self.sequence).then_some(result)
    }
}
impl Drop for Preferences {
    fn drop(&mut self) {
        self.shared
            .slots
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .stop = true;
        self.shared.ready.notify_one();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::appearance::Theme;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    fn path() -> PathBuf {
        std::env::temp_dir()
            .join(format!(
                "texted-appearance-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ))
            .join("appearance.json")
    }
    #[test]
    fn invalid_settings_are_rejected_before_replacing_a_saved_preference() {
        let path = path();
        {
            let mut store = Preferences::new(path.clone(), || {});
            store.save(Appearance::default()).unwrap();
            assert!(store
                .save(Appearance {
                    version: 99,
                    ..Default::default()
                })
                .is_err());
        }
        assert_eq!(load(&path).unwrap().version, 1);
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
    #[test]
    fn latest_accepted_settings_are_durable_on_shutdown() {
        let path = path();
        {
            let mut store = Preferences::new(path.clone(), || {});
            for i in 0..100 {
                store
                    .save(Appearance {
                        theme: Theme::ALL[i % Theme::ALL.len()],
                        ..Default::default()
                    })
                    .unwrap();
            }
            store
                .save(Appearance {
                    theme: Theme::Amber,
                    ..Default::default()
                })
                .unwrap();
        }
        assert_eq!(load(&path).unwrap().theme, Theme::Amber);
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
    #[test]
    fn malformed_oversized_and_future_files_are_left_intact() {
        let path = path();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        for bytes in [
            b"{".to_vec(),
            vec![b' '; MAX_SETTINGS_BYTES + 1],
            br#"{"version":99}"#.to_vec(),
        ] {
            fs::write(&path, &bytes).unwrap();
            assert!(load(&path).is_err());
            assert_eq!(fs::read(&path).unwrap(), bytes);
        }
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}
