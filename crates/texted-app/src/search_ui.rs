//! Search-field interactions; search jobs and buffer mutation have separate owners.
use texted_app::{
    clipboard::FieldStamp,
    input::Intent,
    search::{SearchBarMode, SearchState},
};
use texted_buffer::BufferSnapshot;
pub(crate) struct SearchUi {
    pub state: SearchState,
    serial: u64,
}
impl SearchUi {
    pub fn new(wake: impl Fn() + Send + Sync + 'static) -> Self {
        Self {
            state: SearchState::new(wake),
            serial: 0,
        }
    }
    pub fn stamp(&self) -> Option<FieldStamp> {
        (self.state.mode != SearchBarMode::Closed).then_some(FieldStamp {
            field: self.state.active_field,
            serial: self.serial,
        })
    }
    pub fn reset(&mut self) {
        self.serial += 1;
        self.state.reset_document();
    }
    pub fn paste(
        &mut self,
        stamp: FieldStamp,
        text: String,
        snap: BufferSnapshot,
    ) -> Result<(), String> {
        if self
            .stamp()
            .is_none_or(|now| now.field != stamp.field || now.serial != stamp.serial)
        {
            return Err("Paste skipped: search field changed".into());
        }
        if text.contains(['\n', '\r']) {
            return Err("Search fields accept a single line".into());
        }
        self.append(&text, snap)
    }
    fn append(&mut self, text: &str, snap: BufferSnapshot) -> Result<(), String> {
        let field = if self.state.active_field == 0 {
            &mut self.state.find_query
        } else {
            &mut self.state.replace_query
        };
        let limit = if self.state.active_field == 0 {
            texted_buffer::search::MAX_QUERY_BYTES
        } else {
            texted_app::clipboard::CLIPBOARD_LIMIT
        };
        if field.len().saturating_add(text.len()) > limit {
            return Err(format!(
                "Search field exceeds its {} KiB capacity",
                limit / 1024
            ));
        }
        field.push_str(text);
        self.serial += 1;
        self.state.submit(snap);
        Ok(())
    }
    pub fn focus(&mut self, field: usize) {
        self.state.active_field = field;
        self.serial += 1;
    }
    pub fn handle(&mut self, intent: &Intent, snap: BufferSnapshot) -> Option<Result<(), String>> {
        match intent {
            Intent::Find | Intent::Replace => {
                self.state.mode = if matches!(intent, Intent::Find) {
                    SearchBarMode::Find
                } else {
                    SearchBarMode::Replace
                };
                self.focus(0);
                self.state.navigate(snap, true);
            }
            Intent::NextMatch { forward } => self.state.navigate(snap, *forward),
            Intent::SearchText(text) => return Some(self.append(text, snap)),
            Intent::SearchBackspace => {
                if self.state.active_field == 0 {
                    self.state.find_query.pop();
                } else {
                    self.state.replace_query.pop();
                }
                self.serial += 1;
                self.state.submit(snap);
            }
            Intent::SearchTab => {
                if self.state.mode == SearchBarMode::Replace {
                    self.focus(1 - self.state.active_field);
                }
            }
            Intent::SearchClose => {
                self.state.close();
                self.serial += 1;
            }
            _ => return None,
        }
        Some(Ok(()))
    }
}
pub(crate) const PANEL_WIDTH: usize = 640;
pub(crate) const PREVIOUS_X: usize = 445;
pub(crate) const NEXT_X: usize = 503;
pub(crate) const CLOSE_X: usize = 618;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BarHit {
    Focus(usize),
    Previous,
    Next,
    Replace(bool),
    Close,
}
pub(crate) fn hit(mode: SearchBarMode, width: f32, x: f32, y: f32) -> Option<BarHit> {
    if mode == SearchBarMode::Closed {
        return None;
    }
    let left = (width - (PANEL_WIDTH + 16) as f32).max(0.0);
    let x = x - left;
    if !(0.0..PANEL_WIDTH as f32).contains(&x)
        || !(10.0..if mode == SearchBarMode::Replace {
            78.0
        } else {
            46.0
        })
            .contains(&y)
    {
        return None;
    }
    Some(if x >= CLOSE_X as f32 && y < 46.0 {
        BarHit::Close
    } else if y >= 46.0 && mode == SearchBarMode::Replace {
        if (320.0..362.0).contains(&x) {
            BarHit::Replace(true)
        } else if (245.0..310.0).contains(&x) {
            BarHit::Replace(false)
        } else {
            BarHit::Focus(1)
        }
    } else if (NEXT_X as f32..(NEXT_X + 52) as f32).contains(&x) {
        BarHit::Next
    } else if (PREVIOUS_X as f32..(PREVIOUS_X + 52) as f32).contains(&x) {
        BarHit::Previous
    } else {
        BarHit::Focus(0)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn buttons_share_their_painted_geometry_even_with_long_counts() {
        let left = 980.0 - (PANEL_WIDTH + 16) as f32;
        assert_eq!(
            hit(
                SearchBarMode::Find,
                980.0,
                left + PREVIOUS_X as f32 + 20.0,
                25.0
            ),
            Some(BarHit::Previous)
        );
        assert_eq!(
            hit(
                SearchBarMode::Find,
                980.0,
                left + NEXT_X as f32 + 20.0,
                25.0
            ),
            Some(BarHit::Next)
        );
        assert_eq!(
            hit(SearchBarMode::Replace, 980.0, left + 330.0, 60.0),
            Some(BarHit::Replace(true))
        );
        assert_eq!(hit(SearchBarMode::Closed, 980.0, left, 25.0), None);
    }
}
