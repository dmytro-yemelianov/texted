//! Native shell: routes events to controllers and paints immutable published state.
use crate::{
    appearance_ui::AppearanceUi,
    context_menu::{self, ContextRequest, ContextStep},
    documents::Action,
    editor_view::{self, ViewModel},
    gpu,
    graphics::{DrawList, FontEngine},
    menus::{MenuCommand, MenuState, NativeMenus},
    mouse::Mouse,
    search_ui::{self, BarHit},
    surface::{Host, Renderer as RenderBackend},
    transfers::submit,
    AppEvent,
};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use texted_app::{
    appearance::{Appearance, FontFamily, Typography},
    buffer_service::Command,
    commands::EditorCommand,
    framebuffer::ScreenMapping,
    input::{self, Intent, KeyInput, SearchContext},
    preferences::{self, Preferences},
    search::SearchBarMode,
};
use winit::{
    event::{ElementState, Event, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent},
    event_loop::{ControlFlow, EventLoopProxy, EventLoopWindowTarget},
    keyboard::ModifiersState,
    window::Window,
};
pub(crate) struct Shell {
    pub host: Arc<Host>,
    pub renderer: RenderBackend,
    pub menus: Option<NativeMenus>,
    pub wake: crate::Wake,
    pub preferences: Option<std::path::PathBuf>,
}
pub(crate) struct App {
    session: crate::session::Session,
    wake: crate::Wake,
    dialogs: crate::dialog_ui::DialogUi,
    dialog_client: crate::dialog_ui::Client,
    // The active slot is None because its complete state is owned by `session`.
    tabs: Vec<Option<crate::session::Session>>,
    active: usize,
    next_tab_id: u64,
    tab_first: usize,
    commands: bool,
    pending_open: std::collections::VecDeque<Action>,
    closing: bool,
    quitting: bool,
    /// Keeps tabs and unsaved text between runs; `None` means quitting asks to save.
    store: Option<crate::session_store::SessionStore>,
    /// Quit is waiting for file work to settle before keeping the session.
    hot_exit: bool,
    /// The restored session's active tab, selected once every tab has loaded.
    restore_active: Option<usize>,
    renderer: RenderBackend,
    failed: bool,
    render_failure: Option<String>,
    font: FontEngine,
    ui_font: FontEngine,
    appearance: AppearanceUi,
    preferences: Option<Preferences>,
    draws: DrawList,
    mouse: Mouse,
    modifiers: ModifiersState,
    hud: bool,
    menus: Option<NativeMenus>,
    context: Option<ContextRequest>,
    popup: Option<context_menu::Popup>,
    display: ScreenMapping,
    pointer_physical: (f32, f32),
    panel_help: crate::status_ui::PanelHelp,
    stats: texted_app::document_stats::DocumentStats,
    pointer_inside: bool,
    focused: bool,
    command_hover: Option<usize>,
    key_function: Option<usize>,
    mouse_function: Option<usize>,
    command_flash: Option<(usize, Instant)>,
    /// The effect time last drawn, which holds while unfocused, and when the next frame is due.
    effect_time: f32,
    next_effect_frame: Option<Instant>,
    // Destroy the main-thread window last, after the renderer and document state.
    // Worker callbacks must notify `wake`, never capture this native window.
    window: Arc<Host>,
}
impl App {
    pub fn new(
        window: Arc<Window>,
        renderer: gpu::Renderer,
        initial: Vec<std::path::PathBuf>,
        menus: NativeMenus,
        proxy: EventLoopProxy<AppEvent>,
    ) -> Self {
        // EventLoopProxy is safe to notify after the loop stops; Window's macOS
        // request_redraw dispatches synchronously and cannot be called by joined workers.
        let proxy = std::sync::Mutex::new(proxy);
        let wake: crate::Wake = Arc::new(move || {
            let _ = proxy
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .send_event(AppEvent::Wake);
        });
        let mut app = Self::from_shell(
            Shell {
                host: Host::native(window),
                renderer: RenderBackend::Native(Box::new(renderer)),
                menus: Some(menus),
                wake,
                preferences: preferences::default_path(),
            },
            initial,
        );
        // The session sits beside the preferences, so TEXTED_SETTINGS_PATH isolates both.
        if let Some(dir) =
            preferences::default_path().and_then(|p| p.parent().map(|d| d.join("session")))
        {
            app.keep_session(dir);
        }
        app
    }
    pub(crate) fn from_shell(shell: Shell, initial: Vec<std::path::PathBuf>) -> Self {
        let Shell {
            host: window,
            renderer,
            menus,
            wake,
            preferences: path,
        } = shell;
        let (dialog_client, dialogs) = crate::dialog_ui::channel(wake.clone());
        let mut session = crate::session::Session::new(wake.clone(), 1, dialog_client.clone());
        let mut initial = initial.into_iter();
        if let Some(path) = initial.next() {
            let _ = session.files.enqueue(Action::OpenPath(path));
        }
        let pending_open = initial.map(Action::OpenPath).collect();
        let (mut settings,warning)=match path.as_deref().map(preferences::load) {
            Some(Ok(settings))=>(settings,None),Some(Err(error))=>(Appearance::default(),Some(format!("Appearance preferences could not be loaded: {error}. Changes apply for this session."))),None=>(Appearance::default(),None),
        };
        if window.is_cells() {
            settings.framebuffer = texted_app::framebuffer::Framebuffer::Native;
            settings.typography = Typography::default();
            settings.pixel_typography = Typography::default();
        }
        let mut appearance = AppearanceUi::new(settings);
        appearance.set_cell_backend(window.is_cells());
        appearance.set_terminal_graphics(window.is_terminal() && !window.is_cells());
        let w = wake.clone();
        let preferences = if warning.is_none() {
            path.map(|path| Preferences::new(path, move || w()))
        } else {
            appearance.save_status = "Session only: preferences could not be loaded".into();
            None
        };
        let font = if window.is_cells() {
            FontEngine::for_cells()
        } else {
            FontEngine::with_typography(appearance.settings.editor_typography())
        };
        let ui_font = if window.is_cells() {
            FontEngine::for_cells()
        } else {
            Self::chrome_font(appearance.settings.resolution().is_some())
        };
        let size = window.inner_size();
        let display =
            ScreenMapping::new((size.width, size.height), appearance.settings.resolution());
        let w = wake.clone();
        let stats = texted_app::document_stats::DocumentStats::new(move || w());
        session.message = warning;
        Self {
            session,
            wake,
            dialogs,
            dialog_client,
            tabs: vec![None],
            active: 0,
            next_tab_id: 2,
            tab_first: 0,
            commands: true,
            pending_open,
            closing: false,
            quitting: false,
            store: None,
            hot_exit: false,
            restore_active: None,
            window,
            renderer,
            failed: false,
            render_failure: None,
            font,
            ui_font,
            appearance,
            preferences,
            draws: DrawList::default(),
            mouse: Mouse::new(),
            modifiers: ModifiersState::empty(),
            hud: false,
            menus,
            context: None,
            popup: None,
            display,
            pointer_physical: (0.0, 0.0),
            panel_help: crate::status_ui::PanelHelp::new(),
            stats,
            pointer_inside: false,
            focused: false,
            command_hover: None,
            key_function: None,
            mouse_function: None,
            command_flash: None,
            effect_time: 0.0,
            next_effect_frame: None,
        }
    }
    /// Screen effects belong to the window backend; terminals draw the flat picture.
    fn with_effects(&mut self, mapping: ScreenMapping) -> ScreenMapping {
        if self.window.is_terminal() {
            return mapping;
        }
        let visuals = self.appearance.settings.visuals();
        if !visuals.animated() {
            self.effect_time = visuals.effect_time(0.0);
        } else if self.focused {
            self.effect_time =
                visuals.effect_time(texted_app::visuals::local_seconds_since_midnight());
        }
        let pixel = self.font.pixel_px() * mapping.scale().0;
        let phosphor = texted_app::visuals::Phosphor::for_theme(self.appearance.settings.theme);
        mapping
            .with_effects(visuals.effects, self.effect_time, pixel)
            .with_glass(phosphor, self.appearance.palette.bg)
    }
    fn resolution(&self) -> Option<(u32, u32)> {
        if self.window.is_cells() {
            None
        } else {
            self.appearance.settings.resolution()
        }
    }
    /// Restores the last session, and from now on keeps open tabs and unsaved text at
    /// quit instead of asking to save.
    pub(crate) fn keep_session(&mut self, dir: std::path::PathBuf) {
        let store = match crate::session_store::SessionStore::open(dir) {
            Ok(store) => store,
            Err(error) => {
                self.session.message = Some(format!(
                    "Session not kept ({error}); quitting will ask to save"
                ));
                return;
            }
        };
        let restored = match store.load() {
            Ok(restored) => restored,
            Err(error) => {
                // Leave the unreadable session on disk rather than overwrite it at quit.
                self.session.message = Some(format!(
                    "The last session could not be read ({error}); quitting will ask to save"
                ));
                return;
            }
        };
        let opening_files = !self.pending_open.is_empty() || !self.session.files.idle();
        let mut active = None;
        let tabs: Vec<_> = restored
            .tabs
            .into_iter()
            .enumerate()
            .filter(|(_, tab)| tab.draft.is_some() || tab.path.as_ref().is_some_and(|p| p.exists()))
            .enumerate()
            .map(|(kept, (index, tab))| {
                if index == restored.active {
                    active = Some(kept);
                }
                Action::Restore(tab)
            })
            .collect();
        for action in tabs.into_iter().rev() {
            self.pending_open.push_front(action);
        }
        if !opening_files {
            self.restore_active = active;
        }
        self.store = Some(store);
    }
    /// Writes every tab, with unsaved text as drafts, for the next start.
    fn keep_tabs(&self) -> Result<(), String> {
        let Some(store) = &self.store else {
            return Ok(());
        };
        let mut tabs = vec![];
        let mut active = 0;
        for index in 0..self.tabs.len() {
            let tab = if index == self.active {
                &self.session
            } else {
                self.tabs[index]
                    .as_ref()
                    .expect("inactive tab owns its session")
            };
            let snapshot = tab.buffer.client.latest().snapshot.clone();
            let documents = &tab.files.documents;
            let unsaved = documents.dirty(&snapshot);
            if documents.path.is_none() && !unsaved {
                continue;
            }
            if index == self.active {
                active = tabs.len();
            }
            let (line, column) = snapshot.offset_to_line_col(snapshot.cursor_offset);
            tabs.push(crate::session_store::TabState {
                path: documents.path.clone(),
                draft: unsaved.then(|| snapshot.content.clone()),
                encoding: documents.encoding,
                bom: documents.bom,
                ending: documents.ending,
                line,
                column,
                disk: documents.disk(),
            });
        }
        store.save(&tabs, active)
    }
    pub(crate) fn render_failure(&self) -> Option<&str> {
        self.render_failure.as_deref()
    }
    pub(crate) fn rendered_grid(&self) -> Option<&texted_app::cell_grid::CellGrid> {
        self.renderer.frame()
    }
    pub(crate) fn settled(&self) -> bool {
        self.pending_open.is_empty() && self.session.settled()
    }
    pub(crate) fn resize_graphics(&mut self, size: crate::kitty::Size) -> Result<(), String> {
        self.window.resize_graphics(size)?;
        self.renderer.resize_graphics(size);
        self.resized();
        Ok(())
    }
    pub(crate) fn suspend_graphics(&mut self) -> Result<(), String> {
        self.renderer.suspend()
    }
    pub(crate) fn notice(&mut self, message: &str) {
        self.report(Err(message.into()));
    }
    /// Another program may have changed the file while Texted was in the background.
    pub(crate) fn focus_gained(&mut self) {
        self.session.files.check_disk();
        self.window.request_redraw();
    }
    pub(crate) fn resized(&mut self) {
        self.renderer.invalidate();
        self.context = None;
        self.session.viewport.input(true);
        self.window.request_redraw();
    }
    pub(crate) fn set_modifiers(&mut self, modifiers: ModifiersState) {
        self.modifiers = modifiers;
        self.panel_help.clear_hover();
        self.hover_commands();
        self.window.request_redraw();
    }
    pub(crate) fn key_release(&mut self, key: &winit::keyboard::Key) {
        if self.key_function.is_some() && input::function_key_index(key) == self.key_function {
            self.key_function = None;
            self.window.request_redraw();
        }
    }
    pub(crate) fn paste_text(&mut self, text: String) {
        if self.dialogs.open() {
            self.dialogs.insert(&text);
        } else if self.locked() {
            self.report(Err(
                "File or clipboard capture is pending; paste was not accepted".into(),
            ));
            return;
        } else if self.session.search.state.mode != SearchBarMode::Closed {
            if text.contains(['\n', '\r']) {
                self.report(Err("Search fields accept a single line".into()));
                return;
            }
            let intent = Intent::SearchText(text);
            let snap = self.session.buffer.client.latest().snapshot.clone();
            if let Some(result) = self.session.search.handle(&intent, snap) {
                self.report(result);
            }
        } else {
            self.edit(EditorCommand::Insert(text), true);
        }
        self.window.request_redraw();
    }
    fn poll_dialogs(&mut self) {
        let open = self.dialogs.open();
        if self.dialogs.poll() {
            if !open && self.dialogs.open() {
                crate::documents::reveal_for_dialog(&self.window);
            }
            self.window.request_redraw();
        }
    }
    fn appearance_height(&self) -> usize {
        self.geometry().above_commands()
    }
    fn panel_context(&self) -> crate::chrome::Context {
        let p = self.session.buffer.client.latest();
        crate::chrome::Context {
            search: SearchContext {
                open: self.session.search.state.mode != SearchBarMode::Closed,
                replace: self.session.search.state.mode == SearchBarMode::Replace,
                replacement_field: self.session.search.state.active_field == 1,
                ignore_case: self.session.search.state.options.ignore_case,
                whole_word: self.session.search.state.options.whole_word,
            },
            appearance: self.appearance.open,
            dialog: self.dialogs.open(),
            dialog_accept: self.dialogs.command_label(),
            multiple_tabs: self.tabs.len() > 1,
            selected: context_menu::has_selection(&p),
            can_undo: p.can_undo,
            can_redo: p.can_redo,
            editable: !self.failed
                && !self.session.files.locked()
                && !self.session.transfers.capturing(),
            has_query: !self.session.search.state.find_query.is_empty(),
            scalable: !self.window.is_cells(),
            zoom_percent: self.appearance.settings.editor_typography().scale_percent(),
            statistics: crate::status_ui::selection_label(&p.snapshot, &p.selection)
                .unwrap_or_else(|| {
                    self.stats
                        .result(
                            (u128::from(self.session.id) << 64) | u128::from(p.document),
                            p.snapshot.revision,
                        )
                        .map_or_else(|| "Counting words...".into(), |n| format!("{n} words"))
                }),
            statistics_short: match p.selection.mode {
                texted_app::selection::SelectionMode::Stream => {
                    let (a, b) = p.selection.offsets(&p.snapshot);
                    format!(
                        "{}s",
                        p.snapshot.content.byte_to_char(b) - p.snapshot.content.byte_to_char(a)
                    )
                }
                texted_app::selection::SelectionMode::Block => {
                    let (a, b, c, d) = p.selection.block();
                    format!("{}x{}", b - a + 1, d - c)
                }
                texted_app::selection::SelectionMode::None => self
                    .stats
                    .result(
                        (u128::from(self.session.id) << 64) | u128::from(p.document),
                        p.snapshot.revision,
                    )
                    .map_or_else(|| "...".into(), |n| format!("{n}w")),
            },
            cursor: {
                let (line, col) = p.snapshot.offset_to_line_col(p.snapshot.cursor_offset);
                (line + 1, col + 1)
            },
            wrap: self.session.viewport.wrap,
        }
    }
    fn panel_commands(&self) -> crate::chrome::Commands {
        crate::chrome::Commands::new(&self.panel_context(), self.modifiers)
    }
    fn function_command(&self, index: usize) -> Option<Intent> {
        crate::chrome::command(index, &self.panel_context(), self.modifiers)
    }
    fn invoke_function(&mut self, index: usize) {
        if let Some(intent) = self.function_command(index) {
            self.command_flash = Some((index, Instant::now()));
            self.intent(intent);
        }
    }
    fn hover_commands(&mut self) {
        let g = self.geometry();
        let (x, y) = self.display.to_source_clamped(self.pointer_physical);
        let commands = self.panel_commands();
        let key = if self.pointer_inside {
            g.command_slot(x, y)
                .and_then(|slot| commands.key_at(g, slot))
        } else {
            None
        };
        if key != self.command_hover {
            self.command_hover = key;
            self.window.request_redraw();
        }
        let tip = if self.pointer_inside {
            commands.tip_at(g, x, y)
        } else {
            None
        };
        if self.panel_help.hover(g, x, y, tip) {
            self.window.request_redraw();
        }
    }
    fn geometry(&self) -> crate::chrome::Geometry {
        crate::chrome::Geometry::new(
            self.display.source.0 as usize,
            self.display.source.1 as usize,
            self.display.pixelated,
            self.commands,
        )
        .with_inset(self.chrome_insets().1)
    }
    /// ShaderWallpaper's panel inset around the document text, in scene pixels; terminals have none.
    fn chrome_insets(&self) -> (f32, usize) {
        if self.window.is_terminal() {
            return (0.0, 0);
        }
        let visuals = self.appearance.settings.visuals();
        let pixel = self.font.pixel_px();
        (
            f32::from(visuals.inset_x) * pixel,
            (f32::from(visuals.inset_y) * pixel).round() as usize,
        )
    }
    fn document_size(&self) -> (u32, u32) {
        (
            self.display.source.0,
            self.geometry().document_height() as u32,
        )
    }
    fn reset_transient(&mut self) {
        self.context = None;
        self.popup = None;
        self.mouse = Mouse::new();
        self.panel_help.dismiss();
        self.session.viewport.current = None;
        self.session.viewport.input(false);
        self.geometry()
            .reveal(&mut self.tab_first, self.active, self.tabs.len());
        self.window.request_redraw();
    }
    fn switch_tab(&mut self, index: usize) -> bool {
        if index == self.active {
            return true;
        }
        if index >= self.tabs.len()
            || self.closing
            || self.quitting
            || self.appearance.open
            || !self.session.settled()
        {
            return false;
        }
        let next = self.tabs[index]
            .take()
            .expect("inactive tab owns its session");
        let old = std::mem::replace(&mut self.session, next);
        self.tabs[self.active] = Some(old);
        self.active = index;
        self.reset_transient();
        self.session.files.check_disk();
        true
    }
    fn file_action(&mut self, action: Action) {
        self.context = None;
        if matches!(
            action,
            Action::New | Action::OpenDialog | Action::OpenPath(_)
        ) {
            if self.pending_open.len() >= 128 {
                self.report(Err("Too many pending file requests".into()));
                return;
            }
            self.pending_open.push_back(action);
            self.window.request_redraw();
        } else {
            let result = self.session.files.enqueue(action);
            self.report(result);
        }
    }
    fn open_tab(&mut self, action: Action) {
        let before = self.appearance.settings.clone();
        self.appearance.set_open(false);
        self.appearance_changed(before);
        if let Action::OpenPath(ref path) = action {
            let canonical = path.canonicalize().unwrap_or_else(|_| path.clone());
            if let Some(index) =
                (0..self.tabs.len()).find(|&i| {
                    let tab = if i == self.active {
                        &self.session
                    } else {
                        self.tabs[i].as_ref().expect("inactive tab")
                    };
                    tab.files.documents.path.as_ref().is_some_and(|p| {
                        p.canonicalize().unwrap_or_else(|_| p.clone()) == canonical
                    })
                })
            {
                self.switch_tab(index);
                return;
            }
        }
        // Reuse only a pristine, unnamed, empty tab. Dirty documents are never replaced by Open/New.
        let reuse = self.session.files.documents.path.is_none()
            && !self
                .session
                .files
                .documents
                .dirty(&self.session.buffer.client.latest().snapshot)
            && self
                .session
                .buffer
                .client
                .latest()
                .snapshot
                .content
                .len_bytes()
                == 0
            && !matches!(action, Action::New);
        if !reuse {
            let next = crate::session::Session::new(
                self.wake.clone(),
                self.next_tab_id,
                self.dialog_client.clone(),
            );
            self.next_tab_id += 1;
            let old = std::mem::replace(&mut self.session, next);
            self.tabs[self.active] = Some(old);
            self.active += 1;
            self.tabs.insert(self.active, None);
        }
        if !matches!(action, Action::New) {
            let result = self.session.files.enqueue(action);
            self.report(result);
        }
        self.reset_transient();
    }
    fn begin_close(&mut self) {
        self.popup = None;
        self.context = None;
        if self.closing {
            return;
        }
        self.pending_open.clear();
        let before = self.appearance.settings.clone();
        self.appearance.set_open(false);
        self.appearance_changed(before);
        match self.session.files.enqueue(Action::Close) {
            Ok(()) => self.closing = true,
            Err(error) => {
                self.report(Err(error));
                self.abort_quit();
            }
        }
    }
    fn abort_quit(&mut self) {
        self.quitting = false;
        crate::native_macos::cancel_terminate();
    }
    fn begin_quit(&mut self) {
        if self.session.files.locked() {
            crate::documents::reveal_for_dialog(&self.window);
        }
        if self.store.is_some() {
            // Unsaved tabs are kept for the next start instead of asking now.
            self.hot_exit = true;
            self.pending_open.clear();
            return;
        }
        self.quitting = true;
        self.begin_close();
    }
    fn remove_active(&mut self) {
        self.tabs.remove(self.active);
        self.active = self.active.min(self.tabs.len() - 1);
        self.session = self.tabs[self.active].take().expect("remaining tab");
        self.closing = false;
        self.reset_transient();
    }
    fn chrome_font(pixelated: bool) -> FontEngine {
        if pixelated {
            FontEngine::with_typography(Typography {
                family: FontFamily::Bios,
                size_px: 8,
                line_spacing: 100,
                zoom_percent: 100,
            })
        } else {
            FontEngine::new(16.0)
        }
    }
    fn report(&mut self, result: Result<(), String>) {
        if let Err(message) = result {
            if !self.window.is_terminal() {
                eprintln!("{message}");
            }
            self.session.message = Some(message);
        }
        self.window.request_redraw();
    }
    fn locked(&self) -> bool {
        self.failed
            || self.session.files.locked()
            || self.session.transfers.capturing()
            || self.appearance.open
            || self.dialogs.open()
    }
    /// Returns true once the last tab has closed and the app should end.
    pub fn event(
        &mut self,
        event: Event<AppEvent>,
        target: &EventLoopWindowTarget<AppEvent>,
    ) -> bool {
        let effect_frame = self.next_effect_frame;
        target.set_control_flow(ControlFlow::WaitUntil(
            effect_frame
                .map_or(Instant::now() + Duration::from_secs(1), |at| at)
                .min(
                    Instant::now()
                        + Duration::from_millis(
                            if self.command_flash.is_some()
                                || self.panel_help.pending_tip()
                                || self.session.buffer.client.latest().busy.is_some()
                                || self.session.search.state.pending
                                || self.session.transfers.busy()
                            {
                                16
                            } else {
                                500
                            },
                        ),
                ),
        ));
        match event {
            Event::UserEvent(AppEvent::Menu(command)) => self.menu_command(command),
            Event::UserEvent(AppEvent::Document(action)) => self.file_action(action),
            Event::UserEvent(AppEvent::Quit) => self.begin_quit(),
            Event::UserEvent(AppEvent::Wake) => self.window.request_redraw(),
            Event::WindowEvent { window_id, event }
                if self
                    .window
                    .native_window()
                    .is_some_and(|w| window_id == w.id()) =>
            {
                self.window_event(event)
            }
            Event::AboutToWait => return self.tick(),
            _ => {}
        }
        false
    }
    // Native event routing stays small; each branch delegates editing, geometry or file work.
    fn window_event(&mut self, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => self.begin_quit(),
            WindowEvent::DroppedFile(path) => self.file_action(Action::OpenPath(path)),
            WindowEvent::Focused(focused) => {
                if !focused {
                    self.context = None;
                    self.modifiers = ModifiersState::empty();
                    self.key_function = None;
                    self.mouse_function = None;
                    self.command_hover = None;
                }
                self.focused = focused;
                if focused {
                    self.focus_gained();
                }
                self.update_cursor();
                self.window.request_redraw();
            }
            WindowEvent::CursorEntered { .. } => {
                self.pointer_inside = true;
                self.update_cursor();
            }
            WindowEvent::CursorLeft { .. } => {
                self.pointer_inside = false;
                self.command_hover = None;
                self.mouse_function = None;
                self.update_cursor();
                self.panel_help.clear_hover();
                self.window.request_redraw();
            }
            WindowEvent::ModifiersChanged(mods) => {
                self.modifiers = mods.state();
                self.panel_help.clear_hover();
                self.hover_commands();
                self.window.request_redraw();
            }
            WindowEvent::Resized(_) => {
                self.context = None;
                self.session.viewport.input(true);
                self.window.request_redraw();
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Released => {
                if self.key_function.is_some()
                    && input::function_key_index(&event.logical_key) == self.key_function
                {
                    self.key_function = None;
                    self.window.request_redraw();
                }
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                self.keyboard(event);
            }

            WindowEvent::MouseWheel { delta, .. } if self.appearance.open => {
                let rows = match delta {
                    MouseScrollDelta::LineDelta(_, y) => -f64::from(y),
                    MouseScrollDelta::PixelDelta(p) => {
                        -p.y / f64::from(self.display.scale().1)
                            / if self.display.pixelated { 14.0 } else { 26.0 }
                    }
                };
                if self.appearance.wheel(
                    rows,
                    self.display.source.0 as usize,
                    self.appearance_height(),
                    self.display.pixelated,
                ) {
                    self.window.request_redraw();
                }
            }
            WindowEvent::MouseWheel { delta, .. }
                if !self.session.files.locked() && !self.appearance.open =>
            {
                self.context = None;
                if !self
                    .geometry()
                    .in_document(self.display.to_source_clamped(self.pointer_physical).1)
                {
                    return;
                }
                let (dx, dy) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => (
                        f64::from(x) * f64::from(self.font.char_width_px) * 3.0,
                        f64::from(y) * f64::from(self.font.line_height_px) * 2.5,
                    ),
                    MouseScrollDelta::PixelDelta(p) => {
                        let (sx, sy) = self.display.scale();
                        (p.x / f64::from(sx), p.y / f64::from(sy))
                    }
                };
                self.session.viewport.wheel(
                    dx,
                    dy,
                    self.modifiers.shift_key(),
                    &self.session.buffer.client.latest().snapshot,
                    &self.font,
                );
                self.window.request_redraw();
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.pointer_move(position.x as f32, position.y as f32)
            }
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => self.pointer_button(state),
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Right,
                ..
            } => self.context_click(),
            WindowEvent::RedrawRequested => self.redraw(),
            _ => {}
        }
    }
    fn keyboard(&mut self, event: KeyEvent) {
        let input = KeyInput {
            physical: event.physical_key,
            logical: &event.logical_key,
            text: event.text.as_deref(),
            modifiers: self.modifiers,
        };
        self.key(&input);
    }
    pub(crate) fn key(&mut self, input: &KeyInput<'_>) {
        self.context = None;
        self.panel_help.clear_hover();
        let state = &self.session.search.state;
        let context = SearchContext {
            open: state.mode != SearchBarMode::Closed,
            replace: state.mode == SearchBarMode::Replace,
            replacement_field: state.active_field == 1,
            ignore_case: state.options.ignore_case,
            whole_word: state.options.whole_word,
        };
        if self.popup.is_some() {
            let state = self.menu_state();
            let mut popup = self.popup.take().unwrap();
            use winit::keyboard::{Key, NamedKey};
            match input.logical {
                Key::Named(NamedKey::Escape) => {}
                Key::Named(NamedKey::Enter) => {
                    if let Some(command) = popup.selected(state) {
                        self.menu_command(command);
                    }
                }
                Key::Named(NamedKey::ArrowUp | NamedKey::ArrowDown | NamedKey::Tab) => {
                    popup.navigate(
                        *input.logical != Key::Named(NamedKey::ArrowUp)
                            && !input.modifiers.shift_key(),
                        state,
                    );
                    self.popup = Some(popup);
                }
                _ => self.popup = Some(popup),
            }
            self.window.request_redraw();
            return;
        }
        if let Some(index) = input::function_key_index(input.logical) {
            self.key_function = Some(index);
            self.invoke_function(index);
            return;
        }
        if self.dialogs.open() {
            self.dialogs.key(input);
            self.window.request_redraw();
            return;
        }
        let intent = input::decode(input, context);
        if self.panel_help.details && !matches!(intent, Some(Intent::DocumentInfo)) {
            self.panel_help.dismiss();
            self.window.request_redraw();
            if matches!(
                *input.logical,
                winit::keyboard::Key::Named(winit::keyboard::NamedKey::Escape)
            ) {
                return;
            }
        }

        if self.appearance.open
            && !matches!(
                intent,
                Some(
                    Intent::ToggleAppearance
                        | Intent::DocumentInfo
                        | Intent::File(_)
                        | Intent::Zoom(_)
                        | Intent::ResetFontSize
                )
            )
        {
            let before = self.appearance.settings.clone();
            self.appearance.key(
                input,
                self.display.source.0 as usize,
                self.appearance_height(),
                self.display.pixelated,
            );
            self.appearance_changed(before);
            return;
        }
        if let Some(intent) = intent {
            self.intent(intent);
        }
    }
    fn appearance_changed(&mut self, before: Appearance) {
        let accepted = self.appearance.take_accepted();
        let changed = before != self.appearance.settings;
        if changed {
            let previous = before.editor_typography();
            let typography = self.appearance.settings.editor_typography();
            let resolution_changed = before.resolution() != self.appearance.settings.resolution();
            if before.resolution().is_some() != self.appearance.settings.resolution().is_some() {
                self.ui_font = Self::chrome_font(self.appearance.settings.resolution().is_some());
            }
            if resolution_changed {
                self.context = None;
                self.mouse.dragging = false;
                self.session.viewport.horizontal.end_drag();
            }
            if (previous != typography || resolution_changed) && !self.window.is_cells() {
                if previous.family != typography.family || previous.size_px != typography.size_px {
                    self.font = FontEngine::with_typography(typography);
                } else {
                    self.font
                        .set_zoom(typography.zoom_percent, typography.line_spacing);
                }
                self.session.viewport.horizontal.reset();
                self.session.viewport.bar = None;
                self.session.viewport.top_wrap_row = 0;
                self.session.viewport.input(true);
            }
        }
        if (changed && !self.appearance.previewing()) || accepted {
            if let Some(preferences) = &mut self.preferences {
                match preferences.save(self.appearance.settings.clone()) {
                    Ok(()) => self.appearance.save_status = "Saved".into(),
                    Err(error) => {
                        self.appearance.save_status = "Could not save preferences".into();
                        self.session.message = Some(error);
                    }
                }
            }
        }
        self.update_cursor();
        self.window.request_redraw();
    }
    fn pointer_shape(&self) -> crate::cursor_ui::Shape {
        use crate::cursor_ui::Shape;
        let (width, height) = self.document_size();
        let (x, y) = self.mouse.position;
        let compact = self.display.pixelated;
        if self.popup.is_some() {
            return Shape::Pointer;
        }
        let scene = self.display.to_source_clamped(self.pointer_physical);
        if self
            .geometry()
            .hit(scene.0, scene.1, self.tab_first, self.tabs.len())
            .is_some_and(|h| matches!(h, crate::chrome::Hit::Command(_)))
        {
            return if self
                .geometry()
                .command_slot(scene.0, scene.1)
                .and_then(|slot| self.panel_commands().key_at(self.geometry(), slot))
                .is_some()
            {
                Shape::Pointer
            } else {
                Shape::Arrow
            };
        }
        if self.dialogs.open() {
            return self.dialogs.cursor(
                (self.display.source.0 as usize, self.appearance_height()),
                compact,
                scene.0,
                scene.1,
            );
        }
        if let Some(shape) = self.panel_help.cursor(self.geometry(), scene.0, scene.1) {
            return shape;
        }
        if self.appearance.open {
            let width = self.display.source.0;
            let height = self.appearance_height() as u32;
            let (x, y) = self.display.to_source_clamped(self.pointer_physical);
            return self
                .appearance
                .cursor(width as usize, height as usize, compact, x, y);
        }
        let scene = self.display.to_source_clamped(self.pointer_physical);
        if self
            .geometry()
            .hit(scene.0, scene.1, self.tab_first, self.tabs.len())
            .is_some()
        {
            return Shape::Pointer;
        }
        if let Some(hit) =
            search_ui::hit(self.session.search.state.mode, width as f32, x, y, compact)
        {
            return if matches!(hit, BarHit::Focus(_)) {
                Shape::Text
            } else {
                Shape::Pointer
            };
        }
        if x < self
            .session
            .viewport
            .current
            .as_ref()
            .map_or(0.0, |f| f.gutter_w)
            || y >= height as f32
        {
            Shape::Arrow
        } else {
            Shape::Text
        }
    }
    fn custom_cursor(&self) -> bool {
        !self.window.is_terminal()
            && self.focused
            && self.pointer_inside
            && !self.failed
            && !self.session.files.locked()
            && self.appearance.settings.mouse_cursor != texted_app::appearance::MouseCursor::System
    }
    fn update_cursor(&self) {
        self.window.set_cursor_visible(
            !(self.custom_cursor() && self.display.to_source(self.pointer_physical).is_some()),
        );
        self.window.set_cursor_icon(self.pointer_shape().system());
    }
    fn edit(&mut self, command: EditorCommand, snap: bool) {
        self.session.search.state.cancel_navigation();
        let result = submit(&self.session.buffer.client, Command::Edit(command));
        match result {
            Ok(sequence) => {
                if snap {
                    self.session.snap_after = Some(sequence);
                }
                self.session.viewport.input(false);
                self.session.message = None;
            }
            Err(error) => self.session.message = Some(error),
        }
        self.window.request_redraw();
    }
    fn intent(&mut self, intent: Intent) {
        if matches!(
            intent,
            Intent::AcceptDialog | Intent::CancelDialog | Intent::NavigateDialog { .. }
        ) {
            match intent {
                Intent::AcceptDialog => self.dialogs.accept(),
                Intent::CancelDialog => self.dialogs.cancel(),
                Intent::NavigateDialog { forward } => self.dialogs.navigate(forward),
                _ => {}
            }
            self.window.request_redraw();
            return;
        }

        if matches!(
            intent,
            Intent::AcceptAppearance | Intent::ResetAppearance | Intent::BrowseAppearance { .. }
        ) {
            if self.appearance.open {
                let before = self.appearance.settings.clone();
                match intent {
                    Intent::AcceptAppearance => self.appearance.accept_and_close(),
                    Intent::ResetAppearance => self.appearance.reset(),
                    Intent::BrowseAppearance { forward } => self.appearance.browse(
                        forward,
                        self.display.source.0 as usize,
                        self.appearance_height(),
                        self.display.pixelated,
                    ),
                    _ => {}
                }
                self.appearance_changed(before);
            }
            return;
        }
        if matches!(intent, Intent::ToggleWrap) && !self.failed && !self.session.files.locked() {
            self.session.viewport.wrap = !self.session.viewport.wrap;
            self.session.viewport.input(true);
            self.window.request_redraw();
            return;
        }
        if matches!(intent, Intent::ToggleCommands) {
            self.commands = !self.commands;
            self.command_hover = None;
            self.panel_help.clear_hover();
            self.session.viewport.input(false);
            self.window.request_redraw();
            return;
        }
        if let Intent::NextTab { forward } = intent {
            let next = if forward {
                (self.active + 1) % self.tabs.len()
            } else {
                (self.active + self.tabs.len() - 1) % self.tabs.len()
            };
            self.switch_tab(next);
            return;
        }
        if let Intent::File(file) = intent {
            use texted_app::input::FileIntent as F;
            match file {
                F::New => self.file_action(Action::New),
                F::Open => self.file_action(Action::OpenDialog),
                F::Quit => self.begin_quit(),
                F::Close => self.begin_close(),
                F::Save { as_new } => {
                    let result = self.session.files.enqueue(Action::Save { save_as: as_new });
                    self.report(result);
                }
            }
            return;
        }
        if matches!(intent, Intent::DocumentInfo) {
            self.panel_help.clear_hover();
            let before = self.appearance.settings.clone();
            self.appearance.set_open(false);
            self.appearance_changed(before);
            self.panel_help.details = !self.panel_help.details;
            self.mouse.dragging = false;
            self.window.request_redraw();
            return;
        }
        if matches!(intent, Intent::ContextMenu) {
            self.panel_help.dismiss();
            self.context_keyboard();
            return;
        }
        if matches!(
            intent,
            Intent::ToggleAppearance | Intent::Zoom(_) | Intent::ResetFontSize
        ) {
            if self.failed || self.session.files.locked() {
                return;
            }
            let before = self.appearance.settings.clone();
            match intent {
                Intent::ToggleAppearance => {
                    self.panel_help.dismiss();
                    self.appearance.set_open(!self.appearance.open);
                    self.mouse.dragging = false;
                    self.session.viewport.horizontal.end_drag();
                }
                Intent::Zoom(delta) if !self.window.is_cells() => self.appearance.zoom(delta),
                Intent::ResetFontSize if !self.window.is_cells() => self.appearance.reset_size(),
                _ => {}
            }
            self.appearance_changed(before);
            return;
        }
        if matches!(intent, Intent::Cancel | Intent::SearchClose) {
            self.session.buffer.client.cancel();
            self.session.transfers.cancel();
        }
        if self.locked() {
            if matches!(intent, Intent::Cancel | Intent::SearchClose) {
                self.window.request_redraw();
            } else {
                self.report(Err(
                    "File or clipboard capture is pending; input was not accepted".into(),
                ));
            }
            return;
        }
        self.session.message = None;
        let snap = self.session.buffer.client.latest().snapshot.clone();
        if let Some(result) = self.session.search.handle(&intent, snap) {
            self.report(result);
            if let Some((line, col)) = self.session.search.jump.take() {
                let caret = texted_app::selection::SelectionState::caret(line, col);
                self.edit(EditorCommand::SetSelection(caret), true);
            }
            return;
        }
        match intent {
            Intent::Edit(command) => self.edit(command, true),
            Intent::Copy { cut } => {
                let result = self
                    .session
                    .transfers
                    .copy(&self.session.buffer.client, cut);
                self.report(result);
            }
            Intent::Paste => {
                let result = self
                    .session
                    .transfers
                    .paste(&self.session.buffer.client, self.session.search.stamp());
                self.report(result);
            }
            Intent::ReplaceMatch { all } => {
                if !self.session.search.state.find_query.is_empty() {
                    self.session.search.state.cancel_navigation();
                    let result = submit(
                        &self.session.buffer.client,
                        Command::Replace {
                            query: self.session.search.state.find_query.clone(),
                            replacement: self.session.search.state.replace_query.clone(),
                            options: self.session.search.state.options,
                            all,
                        },
                    );
                    match result {
                        Ok(sequence) => self.session.snap_after = Some(sequence),
                        Err(error) => self.session.message = Some(error),
                    }
                }
            }
            Intent::ToggleLineNumbers => {
                self.session.viewport.line_numbers = !self.session.viewport.line_numbers;
                self.session.viewport.input(true);
            }
            Intent::ToggleDiagnostics => self.hud = !self.hud,
            Intent::Cancel => self.edit(EditorCommand::ClearSelection, false),
            _ => {}
        }
        self.window.request_redraw();
    }
    fn menu_state(&self) -> MenuState {
        if self.dialogs.open() {
            return self.dialogs.menu_state();
        }
        let p = self.session.buffer.client.latest();
        MenuState {
            editable: !self.locked(),
            style_available: !self.failed && !self.session.files.locked(),
            clipboard_available: !self.session.transfers.busy(),
            selected: context_menu::has_selection(&p),
            can_undo: p.can_undo,
            can_redo: p.can_redo,
            has_query: !self.session.search.state.find_query.is_empty(),
            wrap: self.session.viewport.wrap,
            lines: self.session.viewport.line_numbers,
            diagnostics: self.hud,
            block: p.selection.mode == texted_app::selection::SelectionMode::Block,
            commands: self.commands,
        }
    }
    fn menu_command(&mut self, command: MenuCommand) {
        self.popup = None;
        if self.dialogs.open() {
            self.dialogs.intent(command.intent());
            self.window.request_redraw();
            return;
        }
        self.context = None;
        if let Some(menus) = &mut self.menus {
            menus.invalidate();
        }
        // Validate again at delivery: a native menu may have tracked while workers completed.
        if self.menu_state().enabled(command) {
            self.intent(command.intent());
        }
    }
    fn context_keyboard(&mut self) {
        if self.locked() || self.session.transfers.busy() {
            return;
        }
        let Some(frame) = &self.session.viewport.current else {
            return;
        };
        let rect = frame.layout.caret_rect;
        let x = rect.x.max(frame.config.text_x());
        let y = (rect.y + rect.height + 2.0).clamp(0.0, frame.text_height.saturating_sub(1) as f32);
        let p = self.session.buffer.client.latest();
        let _ = self
            .session
            .search
            .handle(&Intent::SearchClose, p.snapshot.clone());
        self.context = Some(ContextRequest {
            position: self.display.to_window((x, y + self.geometry().top as f32)),
            point: (0, 0),
            document: p.document,
            after: self.session.buffer.client.barrier(),
            positioned: true,
        });
        self.window.request_redraw();
    }
    pub(crate) fn context_click(&mut self) {
        self.context = None;
        let (x, y) = self.mouse.position;
        if self.locked()
            || self.session.transfers.busy()
            || self.display.to_source(self.pointer_physical).is_none()
            || self
                .session
                .viewport
                .current
                .as_ref()
                .is_none_or(|f| x < f.config.text_x() || y < 0.0 || y >= f.text_height as f32)
            || self
                .session
                .viewport
                .bar
                .is_some_and(|bar| bar.contains(x, y))
            || search_ui::hit(
                self.session.search.state.mode,
                self.display.source.0 as f32,
                x,
                y,
                self.display.pixelated,
            )
            .is_some()
        {
            return;
        }
        self.mouse.dragging = false;
        self.session.viewport.horizontal.end_drag();
        // A document popup targets the document, including Paste when the search bar was open.
        let _ = self.session.search.handle(
            &Intent::SearchClose,
            self.session.buffer.client.latest().snapshot.clone(),
        );
        self.context = Some(ContextRequest {
            position: self.pointer_physical,
            point: self.session.viewport.point(x, y),
            document: self.session.buffer.client.latest().document,
            after: self.session.buffer.client.barrier(),
            positioned: false,
        });
        self.window.request_redraw();
    }
    fn poll_context(&mut self) {
        let Some(mut request) = self.context else {
            return;
        };
        self.context = None;
        if self.locked() || self.session.transfers.busy() {
            return;
        }
        match request.advance(&self.session.buffer.client) {
            Ok(ContextStep::Wait) => self.context = Some(request),
            Ok(ContextStep::Cancel) => {}
            Ok(ContextStep::Show) => {
                self.session.viewport.input(false);
                let state = self.menu_state();
                self.popup = Some(context_menu::Popup::new(
                    self.display.to_source_clamped(request.position),
                    state,
                ));
                self.window.request_redraw();
            }
            Err(error) => self.report(Err(error)),
        }
    }
    pub(crate) fn wheel_lines(&mut self, x: f32, y: f32) {
        let state = self.menu_state();
        if let Some(popup) = &mut self.popup {
            popup.navigate(y < 0., state);
            self.window.request_redraw();
            return;
        }
        if self.dialogs.open() {
            self.dialogs.navigate(y < 0.0);
        } else if self.appearance.open {
            self.appearance.wheel(
                -f64::from(y),
                self.display.source.0 as usize,
                self.appearance_height(),
                self.display.pixelated,
            );
        } else if !self.session.files.locked() {
            self.session.viewport.wheel(
                f64::from(x * self.font.char_width_px * 3.0),
                f64::from(y * self.font.line_height_px * 2.5),
                self.modifiers.shift_key(),
                &self.session.buffer.client.latest().snapshot,
                &self.font,
            );
        }
        self.window.request_redraw();
    }
    pub(crate) fn pointer_move(&mut self, x: f32, y: f32) {
        self.pointer_inside = true;
        if self.custom_cursor() {
            self.window.request_redraw();
        }
        self.pointer_physical = (x, y);
        self.mouse.position = self
            .geometry()
            .document_point(self.display.to_source_clamped((x, y)));
        let (x, y) = self.mouse.position;
        self.update_cursor();
        self.hover_commands();
        let state = self.menu_state();
        let size = (self.display.source.0 as usize, self.appearance_height());
        if let Some(popup) = &mut self.popup {
            let scene = self.display.to_source_clamped(self.pointer_physical);
            popup.hover(size, self.display.pixelated, scene.0, scene.1, state);
            self.window.request_redraw();
            return;
        }
        if self.dialogs.open() {
            let scene = self.display.to_source_clamped(self.pointer_physical);
            if self.dialogs.hover(
                (self.display.source.0 as usize, self.appearance_height()),
                self.display.pixelated,
                scene.0,
                scene.1,
            ) {
                self.window.request_redraw();
            }
            return;
        }
        if self.appearance.open {
            if self.display.to_source(self.pointer_physical).is_some()
                && self.appearance.hover(
                    self.display.source.0 as usize,
                    self.appearance_height(),
                    self.display.to_source_clamped(self.pointer_physical).0,
                    self.display.to_source_clamped(self.pointer_physical).1,
                    self.display.pixelated,
                )
            {
                self.window.request_redraw();
            }
            return;
        }
        if self.locked()
            || (!self.mouse.dragging
                && !self
                    .geometry()
                    .in_document(self.display.to_source_clamped(self.pointer_physical).1))
        {
            return;
        }
        if self.session.viewport.scroll_drag(x) {
            self.window.request_redraw();
            return;
        }
        if self.mouse.dragging {
            let (line, col) = self.session.viewport.point(x, y);
            self.edit(EditorCommand::Drag { line, col }, false);
        }
    }
    pub(crate) fn pointer_button(&mut self, state: ElementState) {
        self.context = None;
        if state == ElementState::Released {
            self.mouse_function = None;
            self.dialogs.release();
            self.appearance.end_drag();
            self.update_cursor();
            self.mouse.dragging = false;
            self.session.viewport.horizontal.end_drag();
            return;
        }
        if let Some(mut popup) = self.popup.take() {
            let scene = self.display.to_source_clamped(self.pointer_physical);
            if let Some(command) = popup.click(
                (self.display.source.0 as usize, self.appearance_height()),
                self.display.pixelated,
                scene.0,
                scene.1,
                self.menu_state(),
            ) {
                self.menu_command(command);
            }
            self.window.request_redraw();
            return;
        }
        let (x, y) = self.mouse.position;
        if self.display.to_source(self.pointer_physical).is_none() {
            let before = self.appearance.settings.clone();
            if self.appearance.dismiss_dropdown() {
                self.appearance_changed(before);
            }
            return;
        }
        let scene = self.display.to_source_clamped(self.pointer_physical);
        if let Some(hit) = self
            .geometry()
            .hit(scene.0, scene.1, self.tab_first, self.tabs.len())
            .filter(|hit| !self.appearance.open || matches!(hit, crate::chrome::Hit::Command(_)))
        {
            use crate::chrome::Hit;
            match hit {
                Hit::Tab(index) => {
                    self.switch_tab(index);
                }
                Hit::Close(index) => {
                    if self.switch_tab(index) {
                        self.begin_close();
                    }
                }
                Hit::New => self.file_action(Action::New),
                Hit::Previous => self.tab_first = self.tab_first.saturating_sub(1),
                Hit::Next => {
                    self.tab_first = (self.tab_first + 1).min(
                        self.tabs
                            .len()
                            .saturating_sub(self.geometry().tab_capacity()),
                    )
                }
                Hit::Command(slot) => {
                    if let Some(index) = self.panel_commands().key_at(self.geometry(), slot) {
                        self.mouse_function = Some(index);
                        self.invoke_function(index);
                    }
                }
                Hit::Consumed => {}
            }
            self.window.request_redraw();
            return;
        }
        if self.dialogs.open() {
            self.dialogs.click(
                (self.display.source.0 as usize, self.appearance_height()),
                self.display.pixelated,
                scene.0,
                scene.1,
            );
            self.window.request_redraw();
            return;
        }
        if self.appearance.open {
            let before = self.appearance.settings.clone();
            let (scene_x, scene_y) = self.display.to_source_clamped(self.pointer_physical);
            self.appearance.click(
                self.display.source.0 as usize,
                self.appearance_height(),
                scene_x,
                scene_y,
                self.display.pixelated,
            );
            self.appearance_changed(before);
            return;
        }
        if let Some(action) = self.panel_help.click(self.geometry(), scene.0, scene.1) {
            use crate::status_ui::Action;
            self.panel_help.clear_hover();
            match action {
                Action::Close => self.panel_help.dismiss(),
                Action::Wrap => self.intent(Intent::ToggleWrap),
                Action::Encoding => self.session.files.documents.next_save_format(),
                Action::Reread => {
                    let documents = &self.session.files.documents;
                    if documents.path.is_some() {
                        let reopen = crate::documents::Action::Reopen(documents.reread_target());
                        let result = self.session.files.enqueue(reopen);
                        self.report(result);
                    }
                }
                Action::Consumed => {}
            }
            self.window.request_redraw();
            return;
        }
        if self.locked() {
            return;
        }
        if let Some(hit) = search_ui::hit(
            self.session.search.state.mode,
            self.display.source.0 as f32,
            x,
            y,
            self.display.pixelated,
        ) {
            match hit {
                BarHit::Focus(field) => self.session.search.focus(field),
                BarHit::Previous => self.intent(Intent::NextMatch { forward: false }),
                BarHit::Next => self.intent(Intent::NextMatch { forward: true }),
                BarHit::Replace(all) => self.intent(Intent::ReplaceMatch { all }),
                BarHit::Close => self.intent(Intent::SearchClose),
            }
            self.window.request_redraw();
            return;
        }
        if self.session.viewport.scroll_press(x, y) {
            self.mouse.dragging = false;
            self.window.request_redraw();
            return;
        }
        if self
            .session
            .viewport
            .current
            .as_ref()
            .is_none_or(|f| y >= f.text_height as f32)
        {
            return;
        }
        let gutter = self
            .session
            .viewport
            .current
            .as_ref()
            .is_some_and(|f| x < f.config.text_x());
        let kind = self.mouse.press(gutter);
        let (line, col) = self.session.viewport.point(x, y);
        self.edit(
            EditorCommand::Pointer {
                line,
                col,
                kind,
                extend: self.modifiers.shift_key(),
                block: self.modifiers.alt_key(),
            },
            false,
        );
    }
    pub(crate) fn tick(&mut self) -> bool {
        if self
            .next_effect_frame
            .is_some_and(|at| at <= Instant::now())
        {
            self.next_effect_frame = None;
            self.window.request_redraw();
        }
        self.poll_dialogs();
        if let Some(result) = self.preferences.as_mut().and_then(Preferences::poll) {
            match result {
                Ok(()) => self.appearance.save_status = "Saved".into(),
                Err(error) => {
                    self.appearance.save_status = "Could not save preferences".into();
                    self.session.message =
                        Some(format!("Appearance preferences were not saved: {error}"));
                }
            }
            self.window.request_redraw();
        }
        for done in self.session.buffer.client.drain() {
            if let Err(error) = done.result {
                self.session.message = Some(error);
            }
        }
        let result = self
            .session
            .transfers
            .poll(&self.session.buffer.client, &mut self.session.search);
        self.report_if_error(result);
        match self
            .session
            .files
            .poll(&self.session.buffer.client, self.session.transfers.busy())
        {
            Ok(true) => {
                if self.tabs.len() == 1 {
                    // The last tab was closed on purpose, so the next start begins empty.
                    if let Some(store) = &self.store {
                        let _ = store.save(&[], 0);
                    }
                    return true;
                }
                self.remove_active();
                if self.quitting {
                    self.begin_close();
                }
                return false;
            }
            Err(error) => self.session.message = Some(error),
            _ => {}
        }
        if let Some(notice) = self.session.files.notice.take() {
            self.session.message = Some(notice);
        }
        if self.closing && self.session.files.idle() {
            self.closing = false;
            self.abort_quit(); // A cancelled sheet/save aborts the quit sequence.
        }
        if self.hot_exit && self.session.settled() {
            self.hot_exit = false;
            match self.keep_tabs() {
                Ok(()) => return true,
                Err(error) => {
                    self.store = None;
                    self.report(Err(format!(
                        "Could not keep the session ({error}); asking to save instead"
                    )));
                    self.begin_quit();
                }
            }
        }
        if !self.closing && !self.quitting && !self.hot_exit && self.session.settled() {
            if let Some(action) = self.pending_open.pop_front() {
                self.open_tab(action);
            } else if let Some(index) = self.restore_active.take() {
                self.switch_tab(index);
            }
        }
        if self.session.files.reset {
            self.session.files.reset = false;
            self.context = None;
            self.session.viewport.reset();
            self.session.search.reset();
            self.mouse = Mouse::new();
            self.session.snap_after = None;
            self.session.message = None;
        }
        let published = self.session.buffer.client.latest();
        if self
            .session
            .snap_after
            .is_some_and(|s| published.completed_through >= s)
        {
            self.session.snap_after = None;
            self.session.viewport.input(true);
        }
        if !self.session.files.locked()
            && published.busy.is_none()
            && published.completed_through >= self.session.buffer.client.barrier()
        {
            if self.session.search.state.mode != SearchBarMode::Closed {
                self.session.search.state.submit(published.snapshot.clone());
            }
            if let Some((revision, (start, end))) = self.session.search.state.poll() {
                match submit(
                    &self.session.buffer.client,
                    Command::Edit(EditorCommand::SelectOffsets {
                        revision,
                        start,
                        end,
                    }),
                ) {
                    Ok(sequence) => self.session.snap_after = Some(sequence),
                    Err(error) => self.session.message = Some(error),
                }
            }
        }
        if let Some(error) = self.session.search.state.error.take() {
            self.session.message = Some(error);
        }
        let title = if let Some(message) = &self.session.message {
            format!(
                "{message} — {}",
                self.session.files.documents.title(&published.snapshot)
            )
        } else {
            format!(
                "{}{}",
                self.session.files.documents.title(&published.snapshot),
                if self.session.transfers.busy() {
                    " (Clipboard…)"
                } else if published.busy.is_some() {
                    " (Working… Esc to cancel)"
                } else {
                    ""
                }
            )
        };
        self.window.set_title(&title);
        let state = self.menu_state();
        if let Some(menus) = &mut self.menus {
            menus.update(state);
        }
        self.poll_context();
        if self
            .command_flash
            .is_some_and(|(_, at)| at.elapsed() >= Duration::from_millis(160))
        {
            self.command_flash = None;
            self.window.request_redraw();
        }
        if self.panel_help.tick() {
            self.window.request_redraw();
        }
        if self.session.viewport.blink.elapsed() >= Duration::from_millis(500)
            || self.session.search.state.pending
            || published.busy.is_some()
        {
            self.window.request_redraw();
        }
        false
    }
    fn report_if_error(&mut self, result: Result<(), String>) {
        if let Err(error) = result {
            self.session.message = Some(error);
            self.window.request_redraw();
        }
    }
    pub(crate) fn redraw(&mut self) {
        self.poll_dialogs();
        if self.failed {
            return;
        }
        let size = self.window.inner_size();
        if size.width == 0 || size.height == 0 {
            return;
        }
        let display = self.with_effects(ScreenMapping::new(
            (size.width, size.height),
            self.resolution(),
        ));
        let (width, scene_height) = display.source;
        let (text_inset, document_inset) = self.chrome_insets();
        let geometry = crate::chrome::Geometry::new(
            width as usize,
            scene_height as usize,
            display.pixelated,
            self.commands,
        )
        .with_inset(document_inset);
        self.session.viewport.text_inset = text_inset;
        let height = geometry.document_height() as u32;
        let start = Instant::now();
        let p = self.session.buffer.client.latest();
        let words = self.stats.update(
            (u128::from(self.session.id) << 64) | u128::from(p.document),
            &p.snapshot,
        );
        let frame =
            self.session
                .viewport
                .prepare(&p.snapshot, &self.font, width as usize, height as usize);
        self.session
            .search
            .state
            .set_view(&p.snapshot, &frame.layout);
        let visuals = self.appearance.settings.visuals();
        let window = !self.window.is_terminal();
        self.draws.shadow = (visuals.shadow && window).then_some(crate::graphics::TextShadow {
            x: visuals.shadow_x,
            y: visuals.shadow_y,
            color: self.appearance.palette.shadow,
        });
        // Tiles use the interface font's pixel, as the wallpaper draws them from its font.
        self.draws.frame = match visuals.border {
            texted_app::visuals::Border::Lines => None,
            border => window.then_some(crate::graphics::Frame {
                double: border == texted_app::visuals::Border::Double,
                pixel: self.ui_font.pixel_px(),
                corner_inset: visuals.corner_inset,
            }),
        };
        editor_view::paint(
            &mut self.draws,
            &mut self.font,
            &mut self.ui_font,
            &ViewModel {
                compact: display.pixelated,
                palette: &self.appearance.palette,
                snap: &p.snapshot,
                layout: &frame.layout,
                config: &frame.config,
                selection: &p.selection,
                search_state: &self.session.search.state,
                width: width as usize,
                height: height as usize,
                text_height: frame.text_height,
                gutter_w: frame.gutter_w,
                line_digits: frame.line_digits,
                text_start_x: frame.text_start_x,
                show_line_numbers: self.session.viewport.line_numbers,
                caret_visible: self.session.viewport.caret
                    && !self.dialogs.open()
                    && !self.appearance.open
                    && self.popup.is_none(),
                current_scrollbar: self.session.viewport.bar,
                show_perf_hud: self.hud,
                gpu_name: self.renderer.description(),
                frame_start: start,
                layout_time: frame.layout_time,
            },
        );
        self.draws.translate(0.0, geometry.document_top() as f32);
        let labels: Vec<_> = self
            .tabs
            .iter()
            .enumerate()
            .map(|(index, tab)| {
                if index == self.active {
                    self.session.label()
                } else {
                    tab.as_ref().expect("inactive tab").label()
                }
            })
            .collect();
        let commands = self.panel_commands();
        crate::chrome::paint(
            &mut self.draws,
            &mut self.ui_font,
            geometry,
            &self.appearance.palette,
            &labels,
            self.active,
            self.tab_first,
            &commands,
            crate::chrome::Feedback {
                hover: self.command_hover,
                pressed: self
                    .key_function
                    .or(self.mouse_function)
                    .or(self.command_flash.map(|(key, _)| key)),
            },
        );
        self.panel_help.paint(
            &mut self.draws,
            &mut self.ui_font,
            &crate::status_ui::Model {
                geometry,
                palette: &self.appearance.palette,
                snap: &p.snapshot,
                selection: &p.selection,
                words,
                cur_line: frame.cur_line,
                cur_col: frame.cur_col,
                word_wrap: self.session.viewport.wrap,
                format: self.session.files.documents.format_name(),
                reread: self
                    .session
                    .files
                    .documents
                    .path
                    .as_ref()
                    .map(|_| self.session.files.documents.reread_target().name()),
                ending: self.session.files.documents.ending,
            },
        );
        self.appearance.paint(
            &mut self.draws,
            &mut self.ui_font,
            width as usize,
            geometry.above_commands(),
            display.pixelated,
        );
        let state = self.menu_state();
        if let Some(popup) = &mut self.popup {
            popup.paint(
                &mut self.draws,
                &mut self.ui_font,
                (width as usize, geometry.above_commands()),
                display.pixelated,
                state,
                self.appearance.palette,
            );
        }
        self.dialogs.paint(
            &mut self.draws,
            &mut self.ui_font,
            (width as usize, geometry.above_commands()),
            display.pixelated,
            &self.appearance.palette,
        );
        if self.custom_cursor() {
            if let Some((x, y)) = display.to_source(self.pointer_physical) {
                let shape = self.pointer_shape();
                crate::cursor_ui::paint(
                    &mut self.draws,
                    [width as f32, scene_height as f32],
                    [x, y],
                    self.appearance.settings.mouse_cursor,
                    shape,
                    display.pixelated,
                );
            }
        }
        match self
            .renderer
            .render(&self.draws, size.width, size.height, display)
        {
            Ok(true) => {
                self.session.viewport.current = Some(frame);
                self.display = display;
                self.mouse.position = self
                    .geometry()
                    .document_point(self.display.to_source_clamped(self.pointer_physical));
                self.update_cursor();
                // Moving effects advance at the Visuals frame rate while the window is in use.
                let visuals = self.appearance.settings.visuals();
                self.next_effect_frame = (visuals.animated()
                    && self.focused
                    && !self.window.is_terminal())
                .then(|| Instant::now() + Duration::from_secs_f64(1.0 / f64::from(visuals.fps)));
            }
            Ok(false) => self.window.request_redraw(),
            Err(error) => {
                self.failed = true;
                self.render_failure = Some(error.clone());
                let _=self.session.files.enqueue(Action::ReportError(format!("Rendering failed: {error}. Texted will close; you can save unsaved changes before closing.")));
                let _ = self.session.files.enqueue(Action::Close);
            }
        }
    }
}

impl Drop for App {
    fn drop(&mut self) {
        self.window.set_cursor_visible(true);
    }
}
