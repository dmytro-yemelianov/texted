//! Local, immutable bitmap catalog. Load once before the event loop; no font I/O while drawing.
use serde::Deserialize;
use std::{
    collections::HashSet,
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    sync::OnceLock,
};

const MAX_MANIFEST: usize = 16 * 1024 * 1024;
const MAX_FACE: usize = 2 * 1024 * 1024;
const MAX_TOTAL: usize = 64 * 1024 * 1024;
const MAX_FONTS: usize = 4096;
const UA: &str = "АБВГҐДЕЄЖЗИІЇЙКЛМНОПРСТУФХЦЧШЩЬЮЯабвгґдеєжзиіїйклмнопрстуфхцчшщьюя";
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Manifest {
    format: String,
    schema_version: u32,
    fonts: Vec<Descriptor>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Descriptor {
    id: String,
    name: String,
    width: u16,
    height: u16,
    baseline: u16,
    #[serde(default)]
    file: Option<String>,
    #[serde(default)]
    crc32: Option<u32>,
    #[serde(default)]
    disabled_reason: Option<String>,
}
pub struct LocalFont {
    pub id: String,
    pub name: String,
    pub width: u16,
    pub height: u16,
    pub baseline: u16,
    pub data: Vec<u8>,
    pub glyphs: usize,
    pub ukrainian: usize,
    pub disabled_reason: Option<String>,
}
impl LocalFont {
    pub fn enabled(&self) -> bool {
        self.disabled_reason.is_none() && !self.data.is_empty()
    }
}
#[derive(Default)]
pub struct Catalog {
    pub fonts: Vec<LocalFont>,
    pub warnings: Vec<String>,
}
static CATALOG: OnceLock<Catalog> = OnceLock::new();
pub fn default_path() -> Option<PathBuf> {
    std::env::var_os("TEXTED_FONT_LIBRARY")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .map(|p| PathBuf::from(p).join("Library/Application Support/Texted/fonts"))
        })
}
pub fn catalog() -> &'static Catalog {
    CATALOG.get_or_init(|| match default_path() {
        Some(path) => load(&path),
        None => Catalog::default(),
    })
}
pub fn get(index: u16) -> Option<&'static LocalFont> {
    catalog().fonts.get(usize::from(index))
}
pub fn find(id: &str) -> Option<u16> {
    catalog()
        .fonts
        .iter()
        .position(|f| f.id == id && f.enabled())
        .map(|i| i as u16)
}
fn bounded(path: &Path, max: usize) -> Result<Vec<u8>, String> {
    let file = File::open(path).map_err(|e| e.to_string())?;
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("Expected a regular font file".into());
    }
    let mut data = Vec::new();
    file.take((max + 1) as u64)
        .read_to_end(&mut data)
        .map_err(|e| e.to_string())?;
    if data.len() > max {
        return Err("Font resource exceeds its byte limit".into());
    }
    Ok(data)
}
const CRC_TABLE: [u32; 256] = {
    let mut table = [0; 256];
    let mut index = 0;
    while index < 256 {
        let mut value = index as u32;
        let mut bit = 0;
        while bit < 8 {
            value = (value >> 1) ^ (0xedb88320 & 0u32.wrapping_sub(value & 1));
            bit += 1;
        }
        table[index] = value;
        index += 1;
    }
    table
};
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &byte in data {
        crc = (crc >> 8) ^ CRC_TABLE[((crc ^ u32::from(byte)) & 0xff) as usize];
    }
    !crc
}
pub(crate) fn validate_records(
    data: &[u8],
    width: u16,
    height: u16,
) -> Result<(usize, usize), String> {
    let row_bytes = usize::from(width.div_ceil(8));
    let stride = 4 + row_bytes * usize::from(height);
    if data.is_empty() || data.len() % stride != 0 || data.len() / stride > 65536 {
        return Err("Invalid glyph record count".into());
    }
    let mut previous = None;
    let mut replacement = false;
    let mut ukrainian = 0;
    let padding = if width % 8 == 0 {
        0
    } else {
        (1u8 << (8 - width % 8)) - 1
    };
    for record in data.chunks_exact(stride) {
        let cp = u32::from_le_bytes(record[..4].try_into().unwrap());
        let ch = char::from_u32(cp).ok_or("Invalid Unicode scalar")?;
        if previous.is_some_and(|p| p >= cp) {
            return Err("Glyphs must be unique and sorted".into());
        }
        previous = Some(cp);
        if record[4..]
            .chunks_exact(row_bytes)
            .any(|r| r[row_bytes - 1] & padding != 0)
        {
            return Err("Nonzero font padding bits".into());
        }
        let ink = record[4..].iter().any(|v| *v != 0);
        if ch == '\u{fffd}' {
            replacement = ink;
        }
        if UA.contains(ch) && ink {
            ukrainian += 1;
        }
    }
    if !replacement {
        return Err("Font requires a visible missing-character box".into());
    }
    Ok((data.len() / stride, ukrainian))
}
pub fn load(root: &Path) -> Catalog {
    let mut result = Catalog::default();
    if !root.exists() {
        return result;
    }
    let manifest = (|| {
        let bytes = bounded(&root.join("library.json"), MAX_MANIFEST)?;
        let m: Manifest = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        if m.format != "texted-bitmap-library" || m.schema_version != 1 || m.fonts.len() > MAX_FONTS
        {
            return Err("Unsupported local font catalog".into());
        }
        Ok(m)
    })();
    let manifest = match manifest {
        Ok(m) => m,
        Err(e) => {
            result.warnings.push(e);
            return result;
        }
    };
    let root = match root.canonicalize() {
        Ok(p) => p,
        Err(e) => {
            result.warnings.push(e.to_string());
            return result;
        }
    };
    let mut ids = HashSet::new();
    let mut total = 0;
    for d in manifest.fonts {
        if d.id.is_empty()
            || d.id.len() > 160
            || !d
                .id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
            || !ids.insert(d.id.clone())
            || d.name.is_empty()
            || d.name.chars().count() > 200
            || d.name.chars().any(char::is_control)
        {
            result
                .warnings
                .push("Invalid/duplicate local font identity".into());
            continue;
        }
        let mut font = LocalFont {
            id: d.id,
            name: d.name,
            width: d.width,
            height: d.height,
            baseline: d.baseline,
            data: Vec::new(),
            glyphs: 0,
            ukrainian: 0,
            disabled_reason: d.disabled_reason,
        };
        if font.disabled_reason.is_none() {
            let data = (|| {
                if !(1..=1022).contains(&font.width)
                    || !(1..=1022).contains(&font.height)
                    || u32::from(font.width) * u32::from(font.height) > 262144
                    || font.baseline > font.height
                {
                    return Err("Native cell exceeds the renderer limit".into());
                }
                let name = d.file.ok_or("Missing font file")?;
                if !name.ends_with(".bin")
                    || name.len() > 220
                    || !name
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
                {
                    return Err("Invalid font filename".into());
                }
                let file = root.join(name).canonicalize().map_err(|e| e.to_string())?;
                if !file.starts_with(&root) {
                    return Err("Font file escapes its catalog".into());
                }
                let data = bounded(&file, MAX_FACE)?;
                if total + data.len() > MAX_TOTAL {
                    return Err("Local fonts exceed the 64 MiB memory budget".into());
                }
                if Some(crc32(&data)) != d.crc32 {
                    return Err("Font checksum mismatch".into());
                }
                let (glyphs, ukrainian) = validate_records(&data, font.width, font.height)?;
                font.glyphs = glyphs;
                font.ukrainian = ukrainian;
                total += data.len();
                Ok(data)
            })();
            match data {
                Ok(data) => font.data = data,
                Err(e) => font.disabled_reason = Some(e),
            }
        }
        result.fonts.push(font);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        sync::atomic::{AtomicU64, Ordering},
    };
    static NEXT: AtomicU64 = AtomicU64::new(1);
    struct Pack(PathBuf);
    impl Pack {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "texted-font-pack-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn write(&self, data: &[u8]) {
            fs::write(self.0.join("wide.bin"), data).unwrap();
            fs::write(self.0.join("library.json"),serde_json::to_vec(&serde_json::json!({"format":"texted-bitmap-library","schemaVersion":1,"fonts":[{"id":"wide-face","name":"Wide face","width":13,"height":5,"baseline":4,"file":"wide.bin","crc32":crc32(data)}]})).unwrap()).unwrap();
        }
    }
    impl Drop for Pack {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn data() -> Vec<u8> {
        let mut out = Vec::new();
        for ch in ['A', 'І', 'Ї', '\u{fffd}'] {
            out.extend_from_slice(&(ch as u32).to_le_bytes());
            for _ in 0..5 {
                out.extend_from_slice(&[0x80, 0x08]);
            }
        }
        out
    }
    #[test]
    fn native_multi_byte_records_are_bounded_and_count_real_ukrainian_ink() {
        let pack = Pack::new();
        pack.write(&data());
        let c = load(&pack.0);
        assert_eq!(c.fonts.len(), 1);
        let f = &c.fonts[0];
        assert!(f.enabled());
        assert_eq!((f.width, f.height, f.glyphs, f.ukrainian), (13, 5, 4, 2));
    }
    #[test]
    fn checksum_corruption_disables_only_the_damaged_face() {
        let pack = Pack::new();
        pack.write(&data());
        let mut broken = data();
        broken[4] ^= 1;
        fs::write(pack.0.join("wide.bin"), broken).unwrap();
        let c = load(&pack.0);
        assert!(!c.fonts[0].enabled());
        assert!(c.fonts[0]
            .disabled_reason
            .as_ref()
            .unwrap()
            .contains("checksum"));
    }
    #[test]
    fn padding_unicode_order_and_missing_marker_are_validated_even_with_a_matching_checksum() {
        for change in 0..3 {
            let pack = Pack::new();
            let mut bytes = data();
            match change {
                0 => bytes[5] |= 1,
                1 => bytes[..4].copy_from_slice(&u32::MAX.to_le_bytes()),
                _ => bytes.truncate(bytes.len() - 14),
            };
            pack.write(&bytes);
            assert!(!load(&pack.0).fonts[0].enabled());
        }
    }
    #[test]
    fn empty_directory_keeps_the_bundled_fonts_available() {
        let pack = Pack::new();
        let c = load(&pack.0);
        assert!(c.fonts.is_empty());
        assert_eq!(c.warnings.len(), 1);
    }
    #[test]
    fn paths_cannot_escape_the_font_catalog() {
        let pack = Pack::new();
        pack.write(&data());
        let path = pack.0.join("library.json");
        let mut m: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        m["fonts"][0]["file"] = "../outside.bin".into();
        fs::write(path, serde_json::to_vec(&m).unwrap()).unwrap();
        assert!(!load(&pack.0).fonts[0].enabled());
    }
    #[test]
    fn unknown_library_versions_do_not_replace_bundled_fonts() {
        let pack = Pack::new();
        fs::write(
            pack.0.join("library.json"),
            br#"{"format":"texted-bitmap-library","schemaVersion":2,"fonts":[]}"#,
        )
        .unwrap();
        assert!(!load(&pack.0).warnings.is_empty());
    }
    #[test]
    fn crc_matches_the_standard_fixture() {
        assert_eq!(crc32(b"123456789"), 0xcbf43926);
    }
}
