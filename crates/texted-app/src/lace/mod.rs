//! The lace layer: a document's letters joined by connecting strokes.
//!
//! Window-independent. Glyphs are modelled as lines and arcs in model units
//! (x-height 8, y up); [`connectors`] finds where a stroke may continue out of a
//! letter, and later modules turn those into ligatures between neighbours.

pub mod connectors;
pub mod geom;
#[cfg(test)]
pub mod model_font;
