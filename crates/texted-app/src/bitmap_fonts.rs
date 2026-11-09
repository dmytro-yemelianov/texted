//! Bundled native bitmap strikes. Glyph records are sorted Unicode scalars followed by MSB-left rows.
//! Sources, checksums, Ukrainian additions and licenses live in assets/fonts; no outline rasterization.
use crate::appearance::FontFamily;
#[derive(Clone, Copy)]
pub struct BitmapFace {
    pub width: u16,
    pub height: u16,
    data: &'static [u8],
}
impl BitmapFace {
    /// Construct a checked native strike from sorted Unicode records and packed rows.
    pub fn from_records(width: u16, height: u16, data: &'static [u8]) -> Result<Self, String> {
        if !(1..=1022).contains(&width)
            || !(1..=1022).contains(&height)
            || u32::from(width) * u32::from(height) > 262144
        {
            return Err("Native cell dimensions are unsupported".into());
        }
        crate::font_library::validate_records(data, width, height)?;
        Ok(Self {
            width,
            height,
            data,
        })
    }
    /// Names the strike: its records' address and the cell size. Records are never freed.
    pub fn id(self) -> (usize, u16, u16) {
        (self.data.as_ptr() as usize, self.width, self.height)
    }
    pub fn glyph(self, ch: char) -> Option<&'static [u8]> {
        let stride = 4 + usize::from(self.height) * usize::from(self.width.div_ceil(8));
        let mut low = 0;
        let mut high = self.data.len() / stride;
        while low < high {
            let mid = low + (high - low) / 2;
            let at = mid * stride;
            let cp = u32::from_le_bytes(self.data[at..at + 4].try_into().unwrap());
            match cp.cmp(&(ch as u32)) {
                std::cmp::Ordering::Less => low = mid + 1,
                std::cmp::Ordering::Greater => high = mid,
                std::cmp::Ordering::Equal => return Some(&self.data[at + 4..at + stride]),
            }
        }
        None
    }
}
pub fn face(family: FontFamily) -> BitmapFace {
    let (width, height) = family.cell();
    let data: &'static [u8] = match family {
        FontFamily::Bios => include_bytes!("../../../assets/fonts/bios_8x8.bin"),
        FontFamily::Ega => include_bytes!("../../../assets/fonts/ega_8x14.bin"),
        FontFamily::Vga => include_bytes!("../../../assets/fonts/vga_8x16.bin"),
        FontFamily::SpleenSmall => include_bytes!("../../../assets/fonts/spleen_6x12.bin"),
        FontFamily::Spleen => include_bytes!("../../../assets/fonts/spleen_8x16.bin"),
        FontFamily::Local(index) => crate::font_library::get(index)
            .filter(|f| f.enabled())
            .map_or(
                include_bytes!("../../../assets/fonts/vga_8x16.bin").as_slice(),
                |f| f.data.as_slice(),
            ),
    };
    BitmapFace {
        width,
        height,
        data,
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    const UA: &str = "АБВГҐДЕЄЖЗИІЇЙКЛМНОПРСТУФХЦЧШЩЬЮЯабвгґдеєжзиіїйклмнопрстуфхцчшщьюяʼ’'₴—–…";
    #[test]
    fn every_face_has_sorted_bounded_native_bitmaps_and_the_complete_ukrainian_alphabet() {
        for family in FontFamily::ALL {
            let font = face(family);
            let stride = 4 + usize::from(font.height);
            assert_eq!(font.data.len() % stride, 0);
            let mut previous = None;
            for record in font.data.chunks_exact(stride) {
                let cp = u32::from_le_bytes(record[..4].try_into().unwrap());
                assert!(char::from_u32(cp).is_some());
                assert!(previous.is_none_or(|p| p < cp));
                previous = Some(cp);
                assert!(record[4..]
                    .iter()
                    .all(|row| row & ((1 << (8 - font.width)) - 1) == 0));
            }
            for ch in UA.chars() {
                assert!(
                    font.glyph(ch)
                        .is_some_and(|rows| rows.iter().any(|v| *v != 0)),
                    "{}: {ch}",
                    family.name()
                );
            }
            assert!(font.glyph('\u{fffd}').is_some());
            assert!(font.glyph(' ').unwrap().iter().all(|v| *v == 0));
            for (a, b) in [
                ('Г', 'Ґ'),
                ('г', 'ґ'),
                ('І', 'Ї'),
                ('і', 'ї'),
                ('И', 'Й'),
                ('и', 'й'),
                ('Ь', 'З'),
                ('ь', 'з'),
            ] {
                assert_ne!(font.glyph(a), font.glyph(b), "{}: {a}/{b}", family.name());
            }
        }
    }
}
