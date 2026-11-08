//! Streaming UTF-8 loading and atomic, metadata-preserving persistence.
use ropey::Rope;
use std::{
    fs::{self, File, OpenOptions},
    io::{self, BufRead, BufReader, BufWriter, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineEnding {
    Lf,
    Crlf,
}
#[derive(Debug, Clone)]
pub struct LoadedDocument {
    pub content: Rope,
    pub path: Option<PathBuf>,
    pub line_ending: LineEnding,
    pub has_bom: bool,
    pub byte_size: usize,
}
pub struct IoManager;
// Normalize CRLF incrementally, including pairs split across read boundaries.
struct NormalizedReader<R> {
    inner: R,
    pending_cr: bool,
    crlf: bool,
    // Either makes CRLF saving differ from the file: a bare LF gains a CR, and CR+CRLF loses one.
    bare_lf: bool,
    doubled_cr: bool,
    emitted_cr: bool,
}
impl<R> NormalizedReader<R> {
    fn new(inner: R) -> Self {
        Self {
            inner,
            pending_cr: false,
            crlf: false,
            bare_lf: false,
            doubled_cr: false,
            emitted_cr: false,
        }
    }
}
impl<R: BufRead> Read for NormalizedReader<R> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }
        let mut n = 0;
        while n < out.len() {
            let input = self.inner.fill_buf()?;
            if input.is_empty() {
                if self.pending_cr {
                    out[n] = b'\r';
                    n += 1;
                    self.pending_cr = false;
                    self.emitted_cr = true;
                }
                break;
            }
            let mut consumed = 0;
            for &b in input {
                if n == out.len() {
                    break;
                }
                if self.pending_cr {
                    self.pending_cr = false;
                    if b == b'\n' {
                        self.crlf = true;
                        self.doubled_cr |= self.emitted_cr;
                        self.emitted_cr = false;
                        out[n] = b'\n';
                        n += 1;
                        consumed += 1;
                        continue;
                    }
                    out[n] = b'\r';
                    n += 1;
                    self.emitted_cr = true;
                    if n == out.len() {
                        break;
                    }
                }
                consumed += 1;
                if b == b'\r' {
                    self.pending_cr = true;
                } else {
                    self.bare_lf |= b == b'\n';
                    self.emitted_cr = false;
                    out[n] = b;
                    n += 1;
                }
            }
            self.inner.consume(consumed);
        }
        Ok(n)
    }
}
static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
/// Renaming over a symlink would replace the link and orphan its target, so save to the target.
fn resolve_links(path: &Path) -> io::Result<PathBuf> {
    let mut target = path.to_path_buf();
    for _ in 0..40 {
        match fs::symlink_metadata(&target) {
            Ok(meta) if meta.file_type().is_symlink() => {
                let link = fs::read_link(&target)?;
                target = match target.parent() {
                    Some(parent) => parent.join(link),
                    None => link,
                };
            }
            _ => return Ok(target),
        }
    }
    Err(io::Error::other("Too many levels of symbolic links"))
}
impl IoManager {
    pub fn open_file<P: AsRef<Path>>(path: P) -> io::Result<LoadedDocument> {
        Self::read(path.as_ref(), true)
    }
    fn read(path: &Path, normalize: bool) -> io::Result<LoadedDocument> {
        let file = File::open(path)?;
        let byte_size = file.metadata()?.len() as usize;
        let mut input = BufReader::new(file);
        let has_bom = input.fill_buf()?.starts_with(&[0xef, 0xbb, 0xbf]);
        if has_bom {
            input.consume(3);
        }
        let (content, line_ending) = if normalize {
            let mut reader = NormalizedReader::new(input);
            let content = Rope::from_reader(&mut reader)?;
            if reader.crlf && (reader.bare_lf || reader.doubled_cr) {
                // One ending setting cannot restore these lines; keep every byte as written.
                return Self::read(path, false);
            }
            let ending = if reader.crlf {
                LineEnding::Crlf
            } else {
                LineEnding::Lf
            };
            (content, ending)
        } else {
            (Rope::from_reader(&mut input)?, LineEnding::Lf)
        };
        Ok(LoadedDocument {
            content,
            path: Some(path.into()),
            line_ending,
            has_bom,
            byte_size,
        })
    }
    pub fn save_atomic<P: AsRef<Path>>(
        path: P,
        content: &str,
        has_bom: bool,
        ending: LineEnding,
    ) -> io::Result<()> {
        Self::save_chunks(path, std::iter::once(content), has_bom, ending)
    }
    pub fn save_rope<P: AsRef<Path>>(
        path: P,
        content: &Rope,
        has_bom: bool,
        ending: LineEnding,
    ) -> io::Result<()> {
        Self::save_chunks(path, content.chunks(), has_bom, ending)
    }
    fn save_chunks<'a, P: AsRef<Path>>(
        path: P,
        chunks: impl Iterator<Item = &'a str>,
        has_bom: bool,
        ending: LineEnding,
    ) -> io::Result<()> {
        let path = resolve_links(path.as_ref())?;
        let path = path.as_path();
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let name = path
            .file_name()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "Missing filename"))?;
        let mut tmp_name = std::ffi::OsString::from(".");
        tmp_name.push(name);
        tmp_name.push(format!(
            ".{}.{}.tmp",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        let tmp = parent.join(tmp_name);
        let mut created = false;
        let result = (|| {
            let file = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
            created = true;
            if let Ok(meta) = fs::metadata(path) {
                file.set_permissions(meta.permissions())?;
            }
            let mut writer = BufWriter::new(file);
            if has_bom {
                writer.write_all(&[0xef, 0xbb, 0xbf])?;
            }
            let mut previous_cr = false;
            for chunk in chunks {
                if ending == LineEnding::Lf {
                    writer.write_all(chunk.as_bytes())?;
                    continue;
                }
                for &b in chunk.as_bytes() {
                    if b == b'\n' && !previous_cr {
                        writer.write_all(b"\r")?;
                    }
                    writer.write_all(&[b])?;
                    previous_cr = b == b'\r';
                }
            }
            writer.flush()?;
            writer.get_ref().sync_all()?;
            drop(writer);
            fs::rename(&tmp, path)?;
            #[cfg(unix)]
            File::open(parent)?.sync_all()?;
            Ok(())
        })();
        if result.is_err() && created {
            let _ = fs::remove_file(&tmp);
        }
        result
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn failed_atomic_rename_cleans_only_its_own_temp_file() {
        let parent = std::env::temp_dir().join(format!("texted-io-failure-{}", std::process::id()));
        let target = parent.join("directory");
        fs::create_dir_all(&target).unwrap();
        assert!(IoManager::save_atomic(&target, "data", false, LineEnding::Lf).is_err());
        assert!(target.is_dir());
        assert_eq!(fs::read_dir(&parent).unwrap().count(), 1);
        fs::remove_dir_all(parent).unwrap();
    }
    #[cfg(unix)]
    #[test]
    fn existing_file_permissions_survive_atomic_save() {
        use std::os::unix::fs::PermissionsExt;
        let path = std::env::temp_dir().join(format!("texted-io-mode-{}", std::process::id()));
        fs::write(&path, "old").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
        IoManager::save_atomic(&path, "new", false, LineEnding::Lf).unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o640
        );
        fs::remove_file(path).unwrap();
    }
    #[test]
    fn round_trip_endings_bom_and_trailing_newlines() {
        let dir = std::env::temp_dir().join(format!("texted-io-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.txt");
        for ending in [LineEnding::Lf, LineEnding::Crlf] {
            for bom in [false, true] {
                for text in ["", "abc", "abc\n", "abc\n\n", "їжак\nґрунт"] {
                    IoManager::save_atomic(&path, text, bom, ending).unwrap();
                    let doc = IoManager::open_file(&path).unwrap();
                    assert_eq!(doc.content.to_string(), text);
                    assert_eq!(doc.has_bom, bom);
                    let raw = fs::read(&path).unwrap();
                    let expected = if ending == LineEnding::Crlf {
                        text.replace('\n', "\r\n")
                    } else {
                        text.to_owned()
                    };
                    assert_eq!(&raw[if bom { 3 } else { 0 }..], expected.as_bytes());
                }
            }
        }
        fs::remove_dir_all(dir).unwrap();
    }
    #[cfg(unix)]
    #[test]
    fn saving_through_a_symlink_updates_its_target_and_keeps_the_link() {
        let dir = std::env::temp_dir().join(format!("texted-io-link-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("dotfiles")).unwrap();
        fs::write(dir.join("dotfiles/vimrc"), "old").unwrap();
        // Relative targets resolve against the link's directory, not the process directory.
        std::os::unix::fs::symlink("dotfiles/vimrc", dir.join(".vimrc")).unwrap();
        IoManager::save_atomic(dir.join(".vimrc"), "new", false, LineEnding::Lf).unwrap();
        let link = fs::symlink_metadata(dir.join(".vimrc")).unwrap();
        assert!(link.file_type().is_symlink());
        assert_eq!(
            fs::read_to_string(dir.join("dotfiles/vimrc")).unwrap(),
            "new"
        );
        assert_eq!(fs::read_dir(dir.join("dotfiles")).unwrap().count(), 1);
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn files_with_mixed_or_doubled_carriage_returns_save_back_byte_for_byte() {
        let dir = std::env::temp_dir().join(format!("texted-io-mixed-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("mixed.txt");
        for raw in [
            &b"a\r\r\nb\r\n"[..],
            b"x\r\r\n",
            b"lf\nlf\ncrlf\r\nlf\n",
            b"crlf\r\ncrlf\r\nlf\n",
        ] {
            fs::write(&path, raw).unwrap();
            let doc = IoManager::open_file(&path).unwrap();
            IoManager::save_rope(&path, &doc.content, doc.has_bom, doc.line_ending).unwrap();
            assert_eq!(
                fs::read(&path).unwrap(),
                raw,
                "{:?}",
                String::from_utf8_lossy(raw)
            );
        }
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn normalization_handles_read_boundaries_and_lone_cr() {
        for capacity in 1..6 {
            let input = BufReader::with_capacity(capacity, &b"a\r\nb\rc\r\n\r"[..]);
            let mut reader = NormalizedReader::new(input);
            let mut text = String::new();
            reader.read_to_string(&mut text).unwrap();
            assert_eq!(text, "a\nb\rc\n\r");
            assert!(reader.crlf && !reader.bare_lf && !reader.doubled_cr);
            let input = BufReader::with_capacity(capacity, &b"a\r\r\nb"[..]);
            let mut reader = NormalizedReader::new(input);
            reader.read_to_string(&mut text).unwrap();
            assert!(reader.doubled_cr, "capacity {capacity}");
        }
    }
}
