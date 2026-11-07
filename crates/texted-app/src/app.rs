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
    event_loop::{ControlFlow, EventLoopWindowTarget},
    keyboard::ModifiersState,
    window::Window,
};
pub(crate) struct App {
    session: crate::session::Session,
    // The active slot is None because its complete state is owned by `session`.
    tabs: Vec<Option<crate::session::Session>>,
    active: usize,
    next_tab_id: u64,
    tab_first: usize,
    commands: bool,
    pending_open: std::collections::VecDeque<Action>,
    closing: bool,
    quitting: bool,
    window: Arc<Window>,
    renderer: gpu::Renderer,
    failed: bool,
    font: FontEngine,
    ui_font: FontEngine,
    appearance: AppearanceUi,
    preferences: Option<Preferences>,
    draws: DrawList,
    mouse: Mouse,
    modifiers: ModifiersState,
    hud: bool,
    menus: NativeMenus,
    context: Option<ContextRequest>,
    display: ScreenMapping,
    pointer_physical: (f32, f32),
    footer: crate::status_ui::Footer,
    stats: texted_app::document_stats::DocumentStats,
    pointer_inside: bool,
    focused: bool,
}
impl App {
    pub fn new(
        window: Arc<Window>,
        renderer: gpu::Renderer,
        initial: Vec<std::path::PathBuf>,
        menus: NativeMenus,
    ) -> Self {
        let mut session = crate::session::Session::new(&window, 1);
        let mut initial = initial.into_iter();
        if let Some(path) = initial.next() {
            let _ = session.files.enqueue(Action::OpenPath(path));
        }
        let pending_open = initial.map(Action::OpenPath).collect();
        let path = preferences::default_path();
        let (settings,warning)=match path.as_deref().map(preferences::load) {Some(Ok(settings))=>(settings,None),Some(Err(error))=>(Appearance::default(),Some(format!("Appearance preferences could not be loaded: {error}. Changes apply for this session."))),None=>(Appearance::default(),Some("Appearance preferences path is unavailable; changes apply for this session.".into()))};
        let mut appearance = AppearanceUi::new(settings);
        let w = window.clone();
        let preferences = if warning.is_none() {
            path.map(|path| Preferences::new(path, move || w.request_redraw()))
        } else {
            appearance.save_status = "Session only: preferences could not be loaded".into();
            None
        };
        let font = FontEngine::with_typography(appearance.settings.editor_typography());
        let ui_font = Self::chrome_font(appearance.settings.resolution().is_some());
        let size = window.inner_size();
        let display =
            ScreenMapping::new((size.width, size.height), appearance.settings.resolution());
        let w = window.clone();
        let stats = texted_app::document_stats::DocumentStats::new(move || w.request_redraw());
        session.message = warning;
        Self {
            session,
            tabs: vec![None],
            active: 0,
            next_tab_id: 2,
            tab_first: 0,
            commands: true,
            pending_open,
            closing: false,
            quitting: false,
            window,
            renderer,
            failed: false,
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
            display,
            pointer_physical: (0.0, 0.0),
            footer: crate::status_ui::Footer::new(),
            stats,
            pointer_inside: false,
            focused: false,
        }
    }
    fn appearance_height(&self) -> usize {
        self.display.source.1 as usize - self.geometry().bottom
    }
    fn panel_context(&self) -> crate::chrome::Context {
        let p = self.session.buffer.client.latest();
        crate::chrome::Context {
            search: SearchContext {
                open: self.session.search.state.mode != SearchBarMode::Closed,
                replace: self.session.search.state.mode == SearchBarMode::Replace,
                replacement_field: self.session.search.state.active_field == 1,
            },
            appearance: self.appearance.open,
            multiple_tabs: self.tabs.len() > 1,
            selected: context_menu::has_selection(&p),
            can_undo: p.can_undo,
            can_redo: p.can_redo,
            editable: !self.failed
                && !self.session.files.locked()
                && !self.session.transfers.capturing(),
            has_query: !self.session.search.state.find_query.is_empty(),
        }
    }
    fn panel_commands(&self) -> crate::chrome::Commands {
        crate::chrome::Commands::new(&self.panel_context(), self.modifiers)
    }
    fn function_command(&self, index: usize) -> Option<Intent> {
        crate::chrome::command(index, &self.panel_context(), self.modifiers)
    }
    fn geometry(&self) -> crate::chrome::Geometry {
        crate::chrome::Geometry::new(
            self.display.source.0 as usize,
            self.display.source.1 as usize,
            self.display.pixelated,
            self.commands,
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
        self.mouse = Mouse::new();
        self.footer.dismiss();
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
            let next = crate::session::Session::new(&self.window, self.next_tab_id);
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
        if self.closing {
            return;
        }
        self.pending_open.clear();
        let before = self.appearance.settings.clone();
        self.appearance.set_open(false);
        self.appearance_changed(before);
        match self.session.files.enqueue(Action::Close) {
            Ok(()) => self.closing = true,
            Err(error) => self.report(Err(error)),
        }
    }
    fn begin_quit(&mut self) {
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
            eprintln!("{message}");
            self.session.message = Some(message);
        }
        self.window.request_redraw();
    }
    fn locked(&self) -> bool {
        self.failed
            || self.session.files.locked()
            || self.session.transfers.capturing()
            || self.appearance.open
    }
    pub fn event(&mut self, event: Event<AppEvent>, target: &EventLoopWindowTarget<AppEvent>) {
        target.set_control_flow(ControlFlow::WaitUntil(
            Instant::now()
                + Duration::from_millis(
                    if self.footer.pending_tip()
                        || self.session.buffer.client.latest().busy.is_some()
                        || self.session.search.state.pending
                        || self.session.transfers.busy()
                    {
                        16
                    } else {
                        500
                    },
                ),
        ));
        match event {
            Event::UserEvent(AppEvent::Menu(command)) => self.menu_command(command),
            Event::UserEvent(AppEvent::Document(action)) => self.file_action(action),
            Event::UserEvent(AppEvent::Quit) => self.begin_quit(),
            Event::WindowEvent { window_id, event } if window_id == self.window.id() => {
                self.window_event(event)
            }
            Event::AboutToWait => self.tick(target),
            _ => {}
        }
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
                }
                self.focused = focused;
                self.update_cursor();
                self.window.request_redraw();
            }
            WindowEvent::CursorEntered { .. } => {
                self.pointer_inside = true;
                self.update_cursor();
            }
            WindowEvent::CursorLeft { .. } => {
                self.pointer_inside = false;
                self.update_cursor();
                self.footer.clear_hover();
                self.window.request_redraw();
            }
            WindowEvent::ModifiersChanged(mods) => {
                self.modifiers = mods.state();
                self.window.request_redraw();
            }
            WindowEvent::Resized(_) => {
                self.context = None;
                self.session.viewport.input(true);
                self.window.request_redraw();
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
        self.context = None;
        self.footer.clear_hover();
        let state = &self.session.search.state;
        let context = SearchContext {
            open: state.mode != SearchBarMode::Closed,
            replace: state.mode == SearchBarMode::Replace,
            replacement_field: state.active_field == 1,
        };
        let input = KeyInput {
            physical: event.physical_key,
            logical: &event.logical_key,
            text: event.text.as_deref(),
            modifiers: self.modifiers,
        };
        if let Some(index) = input::function_key_index(&event.logical_key) {
            if let Some(intent) = self.function_command(index) {
                self.intent(intent);
            }
            return;
        }
        let intent = input::decode(&input, context);
        if self.footer.details && !matches!(intent, Some(Intent::DocumentInfo)) {
            self.footer.dismiss();
            self.window.request_redraw();
            if matches!(
                event.logical_key,
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
                &input,
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
            if previous != typography || resolution_changed {
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
        let scene = self.display.to_source_clamped(self.pointer_physical);
        if self
            .geometry()
            .hit(scene.0, scene.1, self.tab_first, self.tabs.len())
            .is_some_and(|h| matches!(h, crate::chrome::Hit::Command(_)))
        {
            return Shape::Pointer;
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
        if let Some(shape) = self
            .footer
            .cursor(width as usize, height as usize, compact, x, y)
        {
            return shape;
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
            || y >= height as f32 - if compact { 14.0 } else { 28.0 }
        {
            Shape::Arrow
        } else {
            Shape::Text
        }
    }
    fn custom_cursor(&self) -> bool {
        self.focused
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
            Intent::AcceptAppearance | Intent::ResetAppearance | Intent::BrowseAppearance { .. }
        ) {
            if self.appearance.open {
                let before = self.appearance.settings.clone();
                match intent {
                    Intent::AcceptAppearance => self.appearance.accept_and_close(),
                    Intent::ResetAppearance => self.appearance.reset_defaults(),
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
        if matches!(intent, Intent::ToggleCommands) {
            self.commands = !self.commands;
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
            self.footer.clear_hover();
            let before = self.appearance.settings.clone();
            self.appearance.set_open(false);
            self.appearance_changed(before);
            self.footer.details = !self.footer.details;
            self.mouse.dragging = false;
            self.window.request_redraw();
            return;
        }
        if matches!(intent, Intent::ContextMenu) {
            self.footer.dismiss();
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
                    self.footer.dismiss();
                    self.appearance.set_open(!self.appearance.open);
                    self.mouse.dragging = false;
                    self.session.viewport.horizontal.end_drag();
                }
                Intent::Zoom(delta) => self.appearance.zoom(delta),
                Intent::ResetFontSize => self.appearance.reset_size(),
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
                            all,
                        },
                    );
                    match result {
                        Ok(sequence) => self.session.snap_after = Some(sequence),
                        Err(error) => self.session.message = Some(error),
                    }
                }
            }
            Intent::ToggleWrap => {
                self.session.viewport.wrap = !self.session.viewport.wrap;
                self.session.viewport.input(true);
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
        self.context = None;
        self.menus.invalidate();
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
    fn context_click(&mut self) {
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
                self.menus.update(self.menu_state());
                self.menus.show_context(&self.window, request.position);
                self.window.request_redraw();
            }
            Err(error) => self.report(Err(error)),
        }
    }
    fn pointer_move(&mut self, x: f32, y: f32) {
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
        if self.footer.hover(
            self.display.source.0 as usize,
            self.geometry().document_height(),
            self.display.pixelated,
            x,
            y,
        ) {
            self.window.request_redraw();
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
    fn pointer_button(&mut self, state: ElementState) {
        self.context = None;
        if state == ElementState::Released {
            self.appearance.end_drag();
            self.update_cursor();
            self.mouse.dragging = false;
            self.session.viewport.horizontal.end_drag();
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
        let (width, height) = self.document_size();
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
                        if let Some(intent) = self.function_command(index) {
                            self.intent(intent);
                        }
                    }
                }
                Hit::Consumed => {}
            }
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
        if let Some(action) = self.footer.click(
            width as usize,
            height as usize,
            self.display.pixelated,
            x,
            y,
        ) {
            use crate::status_ui::Action as F;
            self.footer.clear_hover();
            match action {
                F::Statistics => self.footer.details = !self.footer.details,
                F::Close => self.footer.dismiss(),
                F::Smaller => self.intent(Intent::Zoom(-1)),
                F::Reset => self.intent(Intent::ResetFontSize),
                F::Larger => self.intent(Intent::Zoom(1)),
                F::Appearance => {
                    self.footer.dismiss();
                    self.intent(Intent::ToggleAppearance);
                }
                F::Wrap => self.intent(Intent::ToggleWrap),
                F::Consumed => {}
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
    fn tick(&mut self, target: &EventLoopWindowTarget<AppEvent>) {
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
        match self.session.files.poll(
            &self.session.buffer.client,
            self.session.transfers.busy(),
            &self.window,
        ) {
            Ok(true) => {
                if self.tabs.len() == 1 {
                    target.exit();
                    return;
                }
                self.remove_active();
                if self.quitting {
                    self.begin_close();
                }
                return;
            }
            Err(error) => self.session.message = Some(error),
            _ => {}
        }
        if self.closing && self.session.files.idle() {
            self.closing = false;
            self.quitting = false; // A cancelled sheet/save aborts the quit sequence.
        }
        if !self.closing && !self.quitting && self.session.settled() {
            if let Some(action) = self.pending_open.pop_front() {
                self.open_tab(action);
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
        self.menus.update(self.menu_state());
        self.poll_context();
        if self.footer.tick() {
            self.window.request_redraw();
        }
        if self.session.viewport.blink.elapsed() >= Duration::from_millis(500)
            || self.session.search.state.pending
            || published.busy.is_some()
        {
            self.window.request_redraw();
        }
    }
    fn report_if_error(&mut self, result: Result<(), String>) {
        if let Err(error) = result {
            self.session.message = Some(error);
            self.window.request_redraw();
        }
    }
    fn redraw(&mut self) {
        if self.failed {
            return;
        }
        let size = self.window.inner_size();
        if size.width == 0 || size.height == 0 {
            return;
        }
        let display = ScreenMapping::new(
            (size.width, size.height),
            self.appearance.settings.resolution(),
        );
        let (width, scene_height) = display.source;
        let geometry = crate::chrome::Geometry::new(
            width as usize,
            scene_height as usize,
            display.pixelated,
            self.commands,
        );
        let height = geometry.document_height() as u32;
        let start = Instant::now();
        let p = self.session.buffer.client.latest();
        let words = self.stats.update(
            (u128::from(self.session.id) << 64) | u128::from(p.document),
            &p.snapshot,
        );
        let typography = self.appearance.settings.editor_typography();
        let zoom_percent = typography.scale_percent();
        let frame = self.session.viewport.prepare(
            &p.snapshot,
            &self.font,
            width as usize,
            height as usize,
            display.pixelated,
        );
        self.session
            .search
            .state
            .set_view(&p.snapshot, &frame.layout);
        editor_view::paint(
            &mut self.draws,
            &mut self.font,
            &mut self.ui_font,
            &ViewModel {
                footer: &self.footer,
                words,
                zoom_percent,
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
                cur_line: frame.cur_line,
                cur_col: frame.cur_col,
                word_wrap: self.session.viewport.wrap,
                show_line_numbers: self.session.viewport.line_numbers,
                caret_visible: self.session.viewport.caret,
                current_scrollbar: self.session.viewport.bar,
                show_perf_hud: self.hud,
                bom: self.session.files.documents.bom,
                ending: self.session.files.documents.ending,
                gpu_name: &self.renderer.description,
                frame_start: start,
                layout_time: frame.layout_time,
            },
        );
        for draw in &mut self.draws.draws {
            draw.rect[1] += geometry.top as f32;
        }
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
        );
        self.appearance.paint(
            &mut self.draws,
            &mut self.ui_font,
            width as usize,
            scene_height as usize - geometry.bottom,
            display.pixelated,
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
            }
            Ok(false) => self.window.request_redraw(),
            Err(error) => {
                self.failed = true;
                let _=self.session.files.enqueue(Action::ReportError(format!("GPU rendering failed: {error}. Texted will close; you can save unsaved changes before closing.")));
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
