//! Window-independent editor controllers and services.
extern crate self as texted_app;
pub mod appearance;
pub mod bitmap_fonts;
pub mod buffer_service;
pub mod cell_grid;
pub mod clipboard;
pub mod commands;
pub mod document_stats;
pub mod editing;
pub mod font_library;
pub mod framebuffer;
pub mod input;
pub mod preferences;
pub mod search;
pub mod selection;

pub(crate) mod app;
pub(crate) mod appearance_ui;
pub(crate) mod chrome;
pub(crate) mod context_menu;
pub(crate) mod cursor_ui;
pub(crate) mod dialog_ui;
pub(crate) mod documents;
pub(crate) mod dropdown;
pub(crate) mod editor_view;
pub(crate) mod file_flow;
pub(crate) mod gpu;
pub(crate) mod graphics;
pub(crate) mod horizontal_scroll;
pub(crate) mod kitty;
pub(crate) mod menus;
pub(crate) mod mouse;
pub(crate) mod native_macos;
pub(crate) mod presentation;
pub(crate) mod raster;
pub mod runtime;
pub(crate) mod search_ui;
pub(crate) mod session;
pub(crate) mod session_store;
pub(crate) mod status_ui;
pub(crate) mod surface;
pub(crate) mod terminal;
pub(crate) mod terminal_input;
pub(crate) mod terminal_probe;
pub(crate) mod transfers;
pub(crate) mod viewport;
use std::sync::Arc;
pub(crate) type Wake = Arc<dyn Fn() + Send + Sync>;
#[derive(Debug, Clone)]
pub(crate) enum AppEvent {
    Document(documents::Action),
    Menu(menus::MenuCommand),
    Quit,
    Wake,
}
