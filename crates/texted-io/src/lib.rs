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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextEncoding {
    Utf8,
    Utf16Le,
    Utf16Be,
    Windows1251,
    Koi8U,
}
impl TextEncoding {
    pub const ALL: [Self; 5] = [
        Self::Utf8,
        Self::Utf16Le,
        Self::Utf16Be,
        Self::Windows1251,
        Self::Koi8U,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Self::Utf8 => "UTF-8",
            Self::Utf16Le => "UTF-16 LE",
            Self::Utf16Be => "UTF-16 BE",
            Self::Windows1251 => "Windows-1251",
            Self::Koi8U => "KOI8-U",
        }
    }
    fn codec(self) -> &'static encoding_rs::Encoding {
        match self {
            Self::Utf8 => encoding_rs::UTF_8,
            Self::Utf16Le => encoding_rs::UTF_16LE,
            Self::Utf16Be => encoding_rs::UTF_16BE,
            Self::Windows1251 => encoding_rs::WINDOWS_1251,
            Self::Koi8U => encoding_rs::KOI8_U,
        }
    }
    fn bom(self) -> &'static [u8] {
        match self {
            Self::Utf8 => &[0xef, 0xbb, 0xbf],
            Self::Utf16Le => &[0xff, 0xfe],
            Self::Utf16Be => &[0xfe, 0xff],
            Self::Windows1251 | Self::Koi8U => &[],
        }
    }
}
/// Streams another encoding as UTF-8, so loading stays chunked for every format.
struct DecodingReader<R> {
    inner: R,
    decoder: encoding_rs::Decoder,
    finished: bool,
}
impl<R: BufRead> Read for DecodingReader<R> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        while !self.finished && !out.is_empty() {
            let input = self.inner.fill_buf()?;
            let last = input.is_empty();
            let (result, read, written, _) = self.decoder.decode_to_utf8(input, out, last);
            self.inner.consume(read);
            self.finished = last && result == encoding_rs::CoderResult::InputEmpty;
            if written > 0 {
                return Ok(written);
            }
            if result == encoding_rs::CoderResult::OutputFull {
                return Err(io::Error::other("Decoding buffer is too small"));
            }
        }
        Ok(0)
    }
}
/// Running text is mostly lowercase, and Windows-1251 and KOI8-U swap the Cyrillic case halves.
fn guess_single_byte(path: &Path) -> io::Result<TextEncoding> {
    let mut input = BufReader::new(File::open(path)?);
    let (mut capitals_1251, mut small_1251) = (0usize, 0usize);
    loop {
        let chunk = input.fill_buf()?;
        if chunk.is_empty() {
            break;
        }
        for &b in chunk {
            match b {
                0 => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "This looks like a binary file, not text",
                    ))
                }
                0xc0..=0xdf => capitals_1251 += 1,
                0xe0..=0xff => small_1251 += 1,
                _ => {}
            }
        }
        let read = chunk.len();
        input.consume(read);
    }
    Ok(if small_1251 >= capitals_1251 {
        TextEncoding::Windows1251
    } else {
        TextEncoding::Koi8U
    })
}
/// Writes UTF-8 text in the target encoding and names the first character it cannot hold.
struct EncodedWriter<W> {
    out: W,
    encoding: TextEncoding,
    encoder: encoding_rs::Encoder,
    line: usize,
    buffer: Vec<u8>,
}
impl<W: Write> EncodedWriter<W> {
    fn new(out: W, encoding: TextEncoding) -> Self {
        Self {
            out,
            encoding,
            encoder: encoding.codec().new_encoder(),
            line: 1,
            buffer: Vec::new(),
        }
    }
    fn write_str(&mut self, text: &str) -> io::Result<()> {
        match self.encoding {
            TextEncoding::Utf8 => self.out.write_all(text.as_bytes())?,
            TextEncoding::Utf16Le | TextEncoding::Utf16Be => {
                let little = self.encoding == TextEncoding::Utf16Le;
                self.buffer.clear();
                for unit in text.encode_utf16() {
                    self.buffer.extend_from_slice(&if little {
                        unit.to_le_bytes()
                    } else {
                        unit.to_be_bytes()
                    });
                }
                self.out.write_all(&self.buffer)?;
            }
            TextEncoding::Windows1251 | TextEncoding::Koi8U => {
                let mut rest = text;
                loop {
                    let needed = self
                        .encoder
                        .max_buffer_length_from_utf8_without_replacement(rest.len())
                        .unwrap_or(rest.len());
                    self.buffer.resize(needed.max(1), 0);
                    let (result, read, written) = self
                        .encoder
                        .encode_from_utf8_without_replacement(rest, &mut self.buffer, false);
                    self.out.write_all(&self.buffer[..written])?;
                    match result {
                        encoding_rs::EncoderResult::InputEmpty => break,
                        encoding_rs::EncoderResult::OutputFull => rest = &rest[read..],
                        encoding_rs::EncoderResult::Unmappable(c) => {
                            let line = self.line
                                + text[..text.len() - rest.len() + read].matches('\n').count();
                            return Err(io::Error::new(
                                io::ErrorKind::InvalidData,
                                format!(
                                    "'{c}' (U+{:04X}) on line {line} cannot be saved as {}",
                                    c as u32,
                                    self.encoding.name()
                                ),
                            ));
                        }
                    }
                }
            }
        }
        self.line += text.matches('\n').count();
        Ok(())
    }
}
#[derive(Debug, Clone)]
pub struct LoadedDocument {
    pub content: Rope,
    pub path: Option<PathBuf>,
    pub line_ending: LineEnding,
    pub has_bom: bool,
    pub encoding: TextEncoding,
    pub byte_size: usize,
    /// The file's identity when it was read; `None` for a new document.
    pub disk: Option<DiskStamp>,
}
/// Identifies one version of a file on disk, to notice when another program replaces it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiskStamp {
    modified: Option<std::time::SystemTime>,
    len: u64,
    // Editors that save atomically replace the file, so its inode changes too.
    inode: u64,
}
impl DiskStamp {
    pub fn of<P: AsRef<Path>>(path: P) -> io::Result<Self> {
        Ok(Self::from_metadata(&fs::metadata(path)?))
    }
    fn from_metadata(meta: &fs::Metadata) -> Self {
        #[cfg(unix)]
        let inode = std::os::unix::fs::MetadataExt::ino(meta);
        #[cfg(not(unix))]
        let inode = 0;
        Self {
            modified: meta.modified().ok(),
            len: meta.len(),
            inode,
        }
    }
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
        Self::open_file_as(path, None)
    }
    /// Opens with `encoding`, or detects it when `None`: a BOM, then UTF-8, then a
    /// Cyrillic single-byte guess. Files with NUL bytes are refused as binary.
    pub fn open_file_as<P: AsRef<Path>>(
        path: P,
        encoding: Option<TextEncoding>,
    ) -> io::Result<LoadedDocument> {
        let path = path.as_ref();
        match Self::read(path, encoding, true) {
            Err(error) if encoding.is_none() && error.kind() == io::ErrorKind::InvalidData => {
                Self::read(path, Some(guess_single_byte(path)?), true)
            }
            result => result,
        }
    }
    fn read(
        path: &Path,
        requested: Option<TextEncoding>,
        normalize: bool,
    ) -> io::Result<LoadedDocument> {
        let file = File::open(path)?;
        let meta = file.metadata()?;
        let byte_size = meta.len() as usize;
        let mut input = BufReader::new(file);
        let head = input.fill_buf()?;
        let sniffed = [
            TextEncoding::Utf8,
            TextEncoding::Utf16Le,
            TextEncoding::Utf16Be,
        ]
        .into_iter()
        .find(|e| head.starts_with(e.bom()));
        let encoding = requested.or(sniffed).unwrap_or(TextEncoding::Utf8);
        let has_bom = sniffed == Some(encoding);
        if has_bom {
            input.consume(encoding.bom().len());
        }
        let source: Box<dyn BufRead> = if encoding == TextEncoding::Utf8 {
            Box::new(input)
        } else {
            Box::new(BufReader::new(DecodingReader {
                inner: input,
                decoder: encoding.codec().new_decoder_without_bom_handling(),
                finished: false,
            }))
        };
        let (content, line_ending) = if normalize {
            let mut reader = NormalizedReader::new(source);
            let content = Rope::from_reader(&mut reader)?;
            if reader.crlf && (reader.bare_lf || reader.doubled_cr) {
                // One ending setting cannot restore these lines; keep every byte as written.
                return Self::read(path, Some(encoding), false);
            }
            let ending = if reader.crlf {
                LineEnding::Crlf
            } else {
                LineEnding::Lf
            };
            (content, ending)
        } else {
            let mut source = source;
            (Rope::from_reader(&mut source)?, LineEnding::Lf)
        };
        Ok(LoadedDocument {
            content,
            path: Some(path.into()),
            line_ending,
            has_bom,
            encoding,
            byte_size,
            disk: Some(DiskStamp::from_metadata(&meta)),
        })
    }
    pub fn save_atomic<P: AsRef<Path>>(
        path: P,
        content: &str,
        has_bom: bool,
        ending: LineEnding,
    ) -> io::Result<()> {
        Self::save_chunks(
            path,
            std::iter::once(content),
            has_bom,
            ending,
            TextEncoding::Utf8,
        )
    }
    pub fn save_rope<P: AsRef<Path>>(
        path: P,
        content: &Rope,
        has_bom: bool,
        ending: LineEnding,
    ) -> io::Result<()> {
        Self::save_rope_as(path, content, has_bom, ending, TextEncoding::Utf8)
    }
    /// Saves in `encoding`; a character it cannot hold fails the save and keeps the old file.
    pub fn save_rope_as<P: AsRef<Path>>(
        path: P,
        content: &Rope,
        has_bom: bool,
        ending: LineEnding,
        encoding: TextEncoding,
    ) -> io::Result<()> {
        Self::save_chunks(path, content.chunks(), has_bom, ending, encoding)
    }
    fn save_chunks<'a, P: AsRef<Path>>(
        path: P,
        chunks: impl Iterator<Item = &'a str>,
        has_bom: bool,
        ending: LineEnding,
        encoding: TextEncoding,
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
                writer.write_all(encoding.bom())?;
            }
            let mut text = EncodedWriter::new(&mut writer, encoding);
            let mut previous_cr = false;
            for chunk in chunks {
                if ending == LineEnding::Lf {
                    text.write_str(chunk)?;
                    continue;
                }
                let mut rest = chunk;
                while let Some(newline) = rest.find('\n') {
                    let line = &rest[..newline];
                    if !line.is_empty() {
                        previous_cr = line.ends_with('\r');
                        text.write_str(line)?;
                    }
                    // A CR already before the LF stays a single CRLF.
                    text.write_str(if previous_cr { "\n" } else { "\r\n" })?;
                    previous_cr = false;
                    rest = &rest[newline + 1..];
                }
                if !rest.is_empty() {
                    previous_cr = rest.ends_with('\r');
                    text.write_str(rest)?;
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
    // Fixtures come from Python's codecs, not from the decoder under test.
    const UKRAINIAN: &str = "Привіт, їжаку!\nҐанок і Єва\n";
    const CP1251: &[u8] = &[
        207, 240, 232, 226, 179, 242, 44, 32, 191, 230, 224, 234, 243, 33, 10, 165, 224, 237, 238,
        234, 32, 179, 32, 170, 226, 224, 10,
    ];
    const KOI8U: &[u8] = &[
        240, 210, 201, 215, 166, 212, 44, 32, 167, 214, 193, 203, 213, 33, 10, 189, 193, 206, 207,
        203, 32, 166, 32, 180, 215, 193, 10,
    ];
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("texted-io-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }
    #[test]
    fn legacy_and_utf16_files_are_detected_and_save_back_byte_for_byte() {
        let dir = scratch("encodings");
        let path = dir.join("doc.txt");
        let utf16_le: &[u8] = &[
            255, 254, 31, 4, 64, 4, 56, 4, 50, 4, 86, 4, 66, 4, 13, 0, 10, 0, 87, 4, 54, 4, 48, 4,
            58, 4,
        ];
        let utf16_be: &[u8] = &[
            254, 255, 4, 31, 4, 64, 4, 56, 4, 50, 4, 86, 4, 66, 0, 13, 0, 10, 4, 87, 4, 54, 4, 48,
            4, 58,
        ];
        for (raw, encoding, text) in [
            (CP1251, TextEncoding::Windows1251, UKRAINIAN),
            (KOI8U, TextEncoding::Koi8U, UKRAINIAN),
            (utf16_le, TextEncoding::Utf16Le, "Привіт\nїжак"),
            (utf16_be, TextEncoding::Utf16Be, "Привіт\nїжак"),
        ] {
            fs::write(&path, raw).unwrap();
            let doc = IoManager::open_file(&path).unwrap();
            assert_eq!(doc.encoding, encoding);
            assert_eq!(doc.content.to_string(), text, "{encoding:?}");
            IoManager::save_rope_as(
                &path,
                &doc.content,
                doc.has_bom,
                doc.line_ending,
                doc.encoding,
            )
            .unwrap();
            assert_eq!(fs::read(&path).unwrap(), raw, "{encoding:?}");
        }
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn binary_files_are_refused_instead_of_decoded() {
        let dir = scratch("binary");
        let path = dir.join("image.bin");
        fs::write(&path, [0x89, b'P', b'N', b'G', 0x00, 0xff, 0x10]).unwrap();
        let error = IoManager::open_file(&path).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("binary"), "{error}");
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn unmappable_characters_stop_the_save_and_leave_the_file_untouched() {
        let dir = scratch("unmappable");
        let path = dir.join("doc.txt");
        fs::write(&path, KOI8U).unwrap();
        // KOI8-U has no typographic apostrophe.
        let text = Rope::from_str("ok\nм’ята\n");
        let error =
            IoManager::save_rope_as(&path, &text, false, LineEnding::Lf, TextEncoding::Koi8U)
                .unwrap_err();
        assert!(error.to_string().contains("U+2019"), "{error}");
        assert!(error.to_string().contains("line 2"), "{error}");
        assert_eq!(fs::read(&path).unwrap(), KOI8U);
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn an_explicit_encoding_rereads_the_same_bytes_differently() {
        let dir = scratch("reread");
        let path = dir.join("doc.txt");
        fs::write(&path, CP1251).unwrap();
        let doc = IoManager::open_file_as(&path, Some(TextEncoding::Koi8U)).unwrap();
        assert_eq!(doc.encoding, TextEncoding::Koi8U);
        assert_eq!(doc.content.to_string(), "оПХБЁР, ©ФЮЙС!\n╔ЮМНЙ Ё ╙БЮ\n");
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn disk_stamp_matches_the_opened_file_until_another_program_rewrites_it() {
        let dir = scratch("stamp");
        let path = dir.join("doc.txt");
        fs::write(&path, "first").unwrap();
        let doc = IoManager::open_file(&path).unwrap();
        assert_eq!(doc.disk, Some(DiskStamp::of(&path).unwrap()));
        // Same length, so only the modification time can tell the versions apart.
        std::thread::sleep(std::time::Duration::from_millis(20));
        fs::write(&path, "fresh").unwrap();
        assert_ne!(doc.disk, Some(DiskStamp::of(&path).unwrap()));
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
