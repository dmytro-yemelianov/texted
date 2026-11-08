//! Texted's native entry point. Editing, workers, file lifecycle and painting have separate owners.
mod app;
mod appearance_ui;
mod chrome;
mod context_menu;
mod cursor_ui;
mod documents;
mod dropdown;
mod editor_view;
mod file_flow;
mod gpu;
mod graphics;
mod horizontal_scroll;
mod menus;
mod mouse;
mod native_macos;
mod presentation;
mod search_ui;
mod session;
mod status_ui;
mod transfers;
mod viewport;
use documents::Action;
use std::{path::PathBuf, sync::Arc};
use winit::{event_loop::EventLoopBuilder, window::WindowBuilder};
pub(crate) type Wake = Arc<dyn Fn() + Send + Sync>;

#[derive(Debug, Clone)]
pub enum AppEvent {
    Document(Action),
    Menu(menus::MenuCommand),
    Quit,
    Wake,
}
fn main() {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    let argument = arguments.first();
    match argument.map(String::as_str) {
        Some("--version" | "-V") => {
            println!("Texted {}", env!("CARGO_PKG_VERSION"));
            return;
        }
        Some("--list-fonts") => {
            for family in texted_app::appearance::FontFamily::ALL {
                println!(
                    "{} · {}x{} · bundled",
                    family.name(),
                    family.cell().0,
                    family.cell().1
                );
            }
            for font in &texted_app::font_library::catalog().fonts {
                println!(
                    "{} · {} · {}x{} · UA {}/66 · {}",
                    font.id,
                    font.name,
                    font.width,
                    font.height,
                    font.ukrainian,
                    font.disabled_reason.as_deref().unwrap_or("ready")
                );
            }
            for warning in &texted_app::font_library::catalog().warnings {
                eprintln!("Font catalog: {warning}");
            }
            return;
        }
        Some("--help" | "-h") => {
            println!("Texted {}\nUsage: texted [FILE ...]\n       texted --version\n       texted --list-fonts\n\nOpen files as plain UTF-8 text in separate tabs. Without FILE, open an empty document.",env!("CARGO_PKG_VERSION"));
            return;
        }
        _ => {}
    }
    for warning in &texted_app::font_library::catalog().warnings {
        eprintln!("Font catalog: {warning}");
    }
    let mut event_loop = EventLoopBuilder::<AppEvent>::with_user_event();
    #[cfg(target_os = "macos")]
    {
        use winit::platform::macos::EventLoopBuilderExtMacOS;
        // Our native menu owns shortcuts; winit's launch-time default must not replace it.
        event_loop.with_default_menu(false);
    }
    let event_loop = event_loop.build().expect("Failed to initialize event loop");
    #[cfg(target_os = "macos")]
    {
        use winit::platform::macos::EventLoopWindowTargetExtMacOS;
        event_loop.set_allows_automatic_window_tabbing(false);
    }
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
    let menus = menus::NativeMenus::new(event_loop.create_proxy()).expect("Could not create menus");
    let mut app = app::App::new(
        window,
        renderer,
        arguments.into_iter().map(PathBuf::from).collect(),
        menus,
        event_loop.create_proxy(),
    );
    let _ = event_loop.run(move |event, target| app.event(event, target));
}
