//! Fast I/O Subsystem for Texted
//!
//! Provides memory-mapped file loading for multi-megabyte files,
//! crash-resilient atomic file persistence, and encoding line-ending detection.

use memmap2::Mmap;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineEnding {
    Lf,
    Crlf,
}

#[derive(Debug, Clone)]
pub struct LoadedDocument {
    pub content: String,
    pub path: Option<PathBuf>,
    pub line_ending: LineEnding,
    pub has_bom: bool,
    pub byte_size: usize,
}

pub struct IoManager;

impl IoManager {
    /// Read a file, using memory mapping for large files to eliminate memory copying overhead
    pub fn open_file<P: AsRef<Path>>(path: P) -> io::Result<LoadedDocument> {
        let path_ref = path.as_ref();
        let file = File::open(path_ref)?;
        let metadata = file.metadata()?;
        let byte_size = metadata.len() as usize;

        let content_bytes: Vec<u8> = if byte_size > 1024 * 1024 {
            // > 1 MB: use memory-map
            let mmap = unsafe { Mmap::map(&file)? };
            mmap.to_vec()
        } else {
            // Small file: standard fast read
            fs::read(path_ref)?
        };

        // Detect BOM (0xEF, 0xBB, 0xBF)
        let (has_bom, slice) = if content_bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
            (true, &content_bytes[3..])
        } else {
            (false, &content_bytes[..])
        };

        let content = String::from_utf8(slice.to_vec())
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

        // Detect dominant line ending
        let line_ending = if content.contains("\r\n") {
            LineEnding::Crlf
        } else {
            LineEnding::Lf
        };

        Ok(LoadedDocument {
            content,
            path: Some(path_ref.to_path_buf()),
            line_ending,
            has_bom,
            byte_size,
        })
    }

    /// Safely writes content to target path atomically via a sibling temp file
    pub fn save_atomic<P: AsRef<Path>>(
        path: P,
        content: &str,
        has_bom: bool,
        line_ending: LineEnding,
    ) -> io::Result<()> {
        let path_ref = path.as_ref();
        let parent = path_ref.parent().unwrap_or_else(|| Path::new("."));
        let file_name = path_ref
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("document.txt");

        let tmp_file_name = format!(".{}_{}.texted_tmp", file_name, std::process::id());
        let tmp_path = parent.join(tmp_file_name);

        {
            let mut file = File::create(&tmp_path)?;

            if has_bom {
                file.write_all(&[0xEF, 0xBB, 0xBF])?;
            }

            match line_ending {
                LineEnding::Lf => {
                    file.write_all(content.as_bytes())?;
                }
                LineEnding::Crlf => {
                    // Normalize to CRLF if requested
                    for line in content.split('\n') {
                        let clean_line = line.strip_suffix('\r').unwrap_or(line);
                        file.write_all(clean_line.as_bytes())?;
                        file.write_all(b"\r\n")?;
                    }
                }
            }

            file.sync_all()?;
        }

        // Atomic rename over target path
        fs::rename(&tmp_path, path_ref)?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_atomic_save_and_read() {
        let dir = std::env::temp_dir().join("texted_test");
        let _ = fs::create_dir_all(&dir);
        let test_file = dir.join("test_note.txt");

        let content = "Hello from Texted!\nLine 2 with LF";
        IoManager::save_atomic(&test_file, content, false, LineEnding::Lf).unwrap();

        let doc = IoManager::open_file(&test_file).unwrap();
        assert_eq!(doc.content, content);
        assert_eq!(doc.line_ending, LineEnding::Lf);
        assert!(!doc.has_bom);

        let _ = fs::remove_file(test_file);
    }
}
