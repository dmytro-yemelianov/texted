//! Ghostty-inspired Instanced Quad GPU Compositor
//!
//! Emits flat GPU instance buffers `(QuadInstance)` for single-pass rendering.
//! Backgrounds, gutters, selections, carets, and glyphs are batched together.

use bytemuck::{Pod, Zeroable};
use texted_layout::{ViewportConfig, ViewportLayout};

pub const QUAD_FLAG_SOLID: u32 = 0;
pub const QUAD_FLAG_GLYPH: u32 = 1;
pub const QUAD_FLAG_BORDER: u32 = 2;

/// Flat 48-byte Instanced Quad directly consumable by wgpu / Metal / Vulkan vertex shaders
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable, PartialEq)]
pub struct QuadInstance {
    pub position: [f32; 2], // Screen pixel coordinates (x, y)
    pub size: [f32; 2],     // Width and height in pixels
    pub uv_min: [f32; 2],   // Texture atlas UV coordinate (top-left)
    pub uv_max: [f32; 2],   // Texture atlas UV coordinate (bottom-right)
    pub color: [f32; 4],    // RGBA float color [r, g, b, a]
    pub flags: u32,         // QUAD_FLAG_*
    pub _padding: [u32; 3], // Align to 16 bytes for GPU uniform/storage alignment
}

impl QuadInstance {
    pub fn solid(x: f32, y: f32, width: f32, height: f32, color: [f32; 4]) -> Self {
        Self {
            position: [x, y],
            size: [width, height],
            uv_min: [0.0, 0.0],
            uv_max: [1.0, 1.0],
            color,
            flags: QUAD_FLAG_SOLID,
            _padding: [0; 3],
        }
    }

    pub fn glyph(
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        uv_min: [f32; 2],
        uv_max: [f32; 2],
        color: [f32; 4],
    ) -> Self {
        Self {
            position: [x, y],
            size: [width, height],
            uv_min,
            uv_max,
            color,
            flags: QUAD_FLAG_GLYPH,
            _padding: [0; 3],
        }
    }
}

/// Color theme for the minimal Notepad aesthetic
pub struct EditorTheme {
    pub background: [f32; 4],
    pub gutter_bg: [f32; 4],
    pub gutter_text: [f32; 4],
    pub text_primary: [f32; 4],
    pub selection: [f32; 4],
    pub caret: [f32; 4],
    pub status_bar_bg: [f32; 4],
    pub status_bar_text: [f32; 4],
}

impl Default for EditorTheme {
    fn default() -> Self {
        Self {
            background: [0.98, 0.98, 0.98, 1.0],     // Notepad light white/off-white
            gutter_bg: [0.94, 0.94, 0.94, 1.0],      // Subtle gutter
            gutter_text: [0.55, 0.55, 0.55, 1.0],    // Soft grey
            text_primary: [0.10, 0.10, 0.10, 1.0],   // Deep charcoal
            selection: [0.70, 0.85, 1.00, 0.6],      // Translucent Windows selection blue
            caret: [0.0, 0.0, 0.0, 1.0],             // Sharp black
            status_bar_bg: [0.93, 0.93, 0.93, 1.0],  // Status bar grey
            status_bar_text: [0.30, 0.30, 0.30, 1.0],// Status bar text
        }
    }
}

/// Dynamic Glyph Atlas coordinates
#[derive(Clone, Copy, Debug)]
pub struct AtlasGlyph {
    pub uv_min: [f32; 2],
    pub uv_max: [f32; 2],
    pub width_px: f32,
    pub height_px: f32,
}

/// Generates batched instance quads from a ViewportLayout
pub struct QuadBatcher {
    theme: EditorTheme,
}

impl Default for QuadBatcher {
    fn default() -> Self {
        Self::new()
    }
}

impl QuadBatcher {
    pub fn new() -> Self {
        Self {
            theme: EditorTheme::default(),
        }
    }

    pub fn with_theme(theme: EditorTheme) -> Self {
        Self { theme }
    }

    /// Builds a single contiguous array of GPU instanced quads
    pub fn build_quads(
        &self,
        layout: &ViewportLayout,
        config: &ViewportConfig,
        caret_visible: bool,
    ) -> Vec<QuadInstance> {
        let mut quads = Vec::with_capacity(512);

        // 1. Editor Canvas Background Quad
        quads.push(QuadInstance::solid(
            0.0,
            0.0,
            config.width_px,
            config.height_px,
            self.theme.background,
        ));

        // 2. Gutter Quad (if line numbers enabled)
        if config.show_line_numbers {
            quads.push(QuadInstance::solid(
                0.0,
                0.0,
                config.line_number_gutter_width_px,
                config.height_px - 24.0, // leave room for status bar
                self.theme.gutter_bg,
            ));
        }

        // 3. Selection Highlights
        for sel in &layout.selection_rects {
            quads.push(QuadInstance::solid(
                sel.x,
                sel.y,
                sel.width,
                sel.height,
                self.theme.selection,
            ));
        }

        // 4. Caret Quad (blinking vertical bar)
        if caret_visible && layout.caret_rect.x >= 0.0 {
            quads.push(QuadInstance::solid(
                layout.caret_rect.x,
                layout.caret_rect.y,
                layout.caret_rect.width,
                layout.caret_rect.height,
                self.theme.caret,
            ));
        }

        // 5. Text Glyphs (mock/atlas quads for each visible char)
        let gutter_offset = if config.show_line_numbers {
            config.line_number_gutter_width_px
        } else {
            10.0
        };

        for line in &layout.visible_lines {
            // Render line number in gutter if enabled
            if config.show_line_numbers {
                let num_str = format!("{:>3}", line.line_index + 1);
                for (col_idx, _ch) in num_str.chars().enumerate() {
                    quads.push(QuadInstance::glyph(
                        8.0 + (col_idx as f32 * config.char_width_px),
                        line.y_px,
                        config.char_width_px,
                        config.line_height_px,
                        [0.0, 0.0],
                        [1.0, 1.0],
                        self.theme.gutter_text,
                    ));
                }
            }

            // Render line text glyphs
            for (col_idx, _ch) in line.text.chars().enumerate() {
                let x = gutter_offset + (col_idx as f32 * config.char_width_px);
                if x + config.char_width_px > config.width_px {
                    break;
                }
                quads.push(QuadInstance::glyph(
                    x,
                    line.y_px,
                    config.char_width_px,
                    config.line_height_px,
                    [0.0, 0.0],
                    [1.0, 1.0],
                    self.theme.text_primary,
                ));
            }
        }

        // 6. Minimal Status Bar (Windows Notepad style at bottom)
        let status_y = config.height_px - 24.0;
        quads.push(QuadInstance::solid(
            0.0,
            status_y,
            config.width_px,
            24.0,
            self.theme.status_bar_bg,
        ));

        quads
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use texted_layout::Rect;

    #[test]
    fn test_batcher_quad_generation() {
        let batcher = QuadBatcher::new();
        let config = ViewportConfig::default();
        let layout = ViewportLayout {
            visible_lines: Vec::new(),
            caret_rect: Rect { x: 50.0, y: 0.0, width: 2.0, height: 20.0 },
            cursor_line: 0,
            cursor_col: 0,
            selection_rects: Vec::new(),
        };

        let quads = batcher.build_quads(&layout, &config, true);
        assert!(!quads.is_empty());
        // Verify background is first quad
        assert_eq!(quads[0].flags, QUAD_FLAG_SOLID);
        assert_eq!(quads[0].size, [config.width_px, config.height_px]);
    }
}
