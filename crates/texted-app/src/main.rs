//! Texted's native entry point. Editing, workers, file lifecycle and painting have separate owners.
mod app;
mod documents;
mod editor_view;
mod file_flow;
mod gpu;
mod graphics;
mod horizontal_scroll;
mod mouse;
mod native_macos;
mod palette;
mod search_ui;
mod transfers;
mod viewport;
use documents::Action;
use std::{path::PathBuf, sync::Arc};
use winit::{event_loop::EventLoopBuilder, window::WindowBuilder};
#[derive(Debug, Clone)]
pub struct AppEvent(Action);
fn main() {
    let argument = std::env::args().nth(1);
    match argument.as_deref() {
        Some("--version" | "-V") => {
            println!("Texted {}", env!("CARGO_PKG_VERSION"));
            return;
        }
        Some("--help" | "-h") => {
            println!("Texted {}\nUsage: texted [FILE]\n       texted --version\n\nOpen FILE as plain UTF-8 text. Without FILE, open an empty document.",env!("CARGO_PKG_VERSION"));
            return;
        }
        _ => {}
    }
    let event_loop = EventLoopBuilder::<AppEvent>::with_user_event()
        .build()
        .expect("Failed to initialize event loop");
    native_macos::install(event_loop.create_proxy())
        .expect("Could not install native document callbacks");
    let window = Arc::new(
        WindowBuilder::new()
            .with_title("Untitled - Texted")
            .with_inner_size(winit::dpi::LogicalSize::new(980.0, 700.0))
            .build(&event_loop)
            .expect("Failed to create window"),
    );
    let renderer = match pollster::block_on(gpu::Renderer::new(window.clone())) {
        Ok(renderer) => renderer,
        Err(error) => {
            eprintln!("{error}");
            rfd::MessageDialog::new()
                .set_title("Texted GPU initialization failed")
                .set_description(&error)
                .set_level(rfd::MessageLevel::Error)
                .show();
            return;
        }
    };
    let mut app = app::App::new(window, renderer, argument.map(PathBuf::from));
    let _ = event_loop.run(move |event, target| app.event(event, target));
}
