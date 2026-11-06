//! Native shell: routes events to controllers and paints immutable published state.
use crate::{
    appearance_ui::{self, AppearanceUi},
    documents::Action,
    editor_view::{self, ViewModel},
    file_flow::FileFlow,
    gpu,
    graphics::{DrawList, FontEngine},
    mouse::Mouse,
    search_ui::{self, BarHit, SearchUi},
    transfers::{submit, Transfers},
    viewport::Viewport,
    AppEvent,
};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use texted_app::{
    appearance::Appearance,
    buffer_service::{BufferService, Command},
    commands::EditorCommand,
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
    window: Arc<Window>,
    renderer: gpu::Renderer,
    failed: bool,
    font: FontEngine,
    ui_font: FontEngine,
    appearance: AppearanceUi,
    preferences: Option<Preferences>,
    draws: DrawList,
    viewport: Viewport,
    mouse: Mouse,
    modifiers: ModifiersState,
    hud: bool,
    buffer: BufferService,
    search: SearchUi,
    transfers: Transfers,
    files: FileFlow,
    message: Option<String>,
    snap_after: Option<u64>,
}
impl App {
    pub fn new(
        window: Arc<Window>,
        renderer: gpu::Renderer,
        initial: Option<std::path::PathBuf>,
    ) -> Self {
        let w = window.clone();
        let buffer = BufferService::new(move || w.request_redraw());
        let w = window.clone();
        let search = SearchUi::new(move || w.request_redraw());
        let w = window.clone();
        let transfers = Transfers::new(move || w.request_redraw());
        let w = window.clone();
        let viewport = Viewport::new(move || w.request_redraw());
        let mut files = FileFlow::new(buffer.client.latest().snapshot.content.clone());
        if let Some(path) = initial {
            let _ = files.enqueue(Action::OpenPath(path));
        }
        let path = preferences::default_path();
        let (settings,warning)=match path.as_deref().map(preferences::load) {Some(Ok(settings))=>(settings,None),Some(Err(error))=>(Appearance::default(),Some(format!("Appearance preferences could not be loaded: {error}. Changes apply for this session."))),None=>(Appearance::default(),Some("Appearance preferences path is unavailable; changes apply for this session.".into()))};
        let mut appearance = AppearanceUi::new(
            settings,
            texted_app::appearance::FontFamily::ALL.map(FontEngine::available),
        );
        let w = window.clone();
        let preferences = if warning.is_none() {
            path.map(|path| Preferences::new(path, move || w.request_redraw()))
        } else {
            appearance.save_status = "Session only: preferences could not be loaded".into();
            None
        };
        let font = FontEngine::with_typography(appearance.settings.typography);
        Self {
            window,
            renderer,
            failed: false,
            font,
            ui_font: FontEngine::new(18.0),
            appearance,
            preferences,
            draws: DrawList::default(),
            viewport,
            mouse: Mouse::new(),
            modifiers: ModifiersState::empty(),
            hud: false,
            buffer,
            search,
            transfers,
            files,
            message: warning,
            snap_after: None,
        }
    }
    fn report(&mut self, result: Result<(), String>) {
        if let Err(message) = result {
            eprintln!("{message}");
            self.message = Some(message);
        }
        self.window.request_redraw();
    }
    fn locked(&self) -> bool {
        self.failed || self.files.locked() || self.transfers.capturing() || self.appearance.open
    }
    pub fn event(&mut self, event: Event<AppEvent>, target: &EventLoopWindowTarget<AppEvent>) {
        target.set_control_flow(ControlFlow::WaitUntil(
            Instant::now()
                + Duration::from_millis(
                    if self.buffer.client.latest().busy.is_some()
                        || self.search.state.pending
                        || self.transfers.busy()
                    {
                        16
                    } else {
                        500
                    },
                ),
        ));
        match event {
            Event::UserEvent(AppEvent(action)) => {
                let result = self.files.enqueue(action);
                self.report(result);
            }
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
            WindowEvent::CloseRequested => {
                let result = self.files.enqueue(Action::Close);
                self.report(result);
            }
            WindowEvent::DroppedFile(path) => {
                let result = self.files.enqueue(Action::OpenPath(path));
                self.report(result);
            }
            WindowEvent::ModifiersChanged(mods) => self.modifiers = mods.state(),
            WindowEvent::Resized(_) => {
                self.viewport.input(true);
                self.window.request_redraw();
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                self.keyboard(event);
            }

            WindowEvent::MouseWheel { delta, .. }
                if !self.files.locked() && !self.appearance.open =>
            {
                let (dx, dy) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => (
                        f64::from(x) * f64::from(self.font.char_width_px) * 3.0,
                        f64::from(y) * f64::from(self.font.line_height_px) * 2.5,
                    ),
                    MouseScrollDelta::PixelDelta(p) => (p.x, p.y),
                };
                self.viewport.wheel(
                    dx,
                    dy,
                    self.modifiers.shift_key(),
                    &self.buffer.client.latest().snapshot,
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
            WindowEvent::RedrawRequested => self.redraw(),
            _ => {}
        }
    }
    fn keyboard(&mut self, event: KeyEvent) {
        let state = &self.search.state;
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
        let intent = input::decode(&input, context);
        if self.appearance.open
            && !matches!(
                intent,
                Some(
                    Intent::ToggleAppearance
                        | Intent::File(_)
                        | Intent::Zoom(_)
                        | Intent::ResetFontSize
                )
            )
        {
            let before = self.appearance.settings.clone();
            self.appearance.key(&input);
            self.appearance_changed(before);
            return;
        }
        if let Some(intent) = intent {
            self.intent(intent);
        }
    }
    fn appearance_changed(&mut self, before: Appearance) {
        if before != self.appearance.settings {
            if before.typography != self.appearance.settings.typography {
                let typography = self.appearance.settings.typography;
                if before.typography.family != typography.family
                    || before.typography.size_px != typography.size_px
                {
                    self.font = FontEngine::with_typography(typography);
                } else {
                    self.font.set_line_spacing(typography.line_spacing);
                }
                // Hit-test the last painted frame until redraw installs the new font geometry.
                self.viewport.horizontal.reset();
                self.viewport.bar = None;
                self.viewport.top_wrap_row = 0;
                self.viewport.input(true);
            }
            if let Some(preferences) = &mut self.preferences {
                match preferences.save(self.appearance.settings.clone()) {
                    Ok(()) => self.appearance.save_status = "Saving...".into(),
                    Err(error) => {
                        self.appearance.save_status = "Could not save preferences".into();
                        self.message = Some(error);
                    }
                }
            }
        }
        self.window.request_redraw();
    }
    fn edit(&mut self, command: EditorCommand, snap: bool) {
        self.search.state.cancel_navigation();
        let result = submit(&self.buffer.client, Command::Edit(command));
        match result {
            Ok(sequence) => {
                if snap {
                    self.snap_after = Some(sequence);
                }
                self.viewport.input(false);
                self.message = None;
            }
            Err(error) => self.message = Some(error),
        }
        self.window.request_redraw();
    }
    fn intent(&mut self, intent: Intent) {
        if matches!(
            intent,
            Intent::ToggleAppearance | Intent::Zoom(_) | Intent::ResetFontSize
        ) {
            if self.failed || self.files.locked() {
                return;
            }
            let before = self.appearance.settings.clone();
            match intent {
                Intent::ToggleAppearance => {
                    self.appearance.open = !self.appearance.open;
                    self.mouse.dragging = false;
                    self.viewport.horizontal.end_drag();
                }
                Intent::Zoom(delta) => self.appearance.zoom(delta),
                Intent::ResetFontSize => self.appearance.reset_size(),
                _ => {}
            }
            self.appearance_changed(before);
            return;
        }
        // Lifecycle intents are queued even while clipboard capture or another file action is pending.
        if let Some(result) = self.files.route(&intent) {
            self.appearance.open = false;
            self.report(result);
            return;
        }
        if matches!(intent, Intent::Cancel | Intent::SearchClose) {
            self.buffer.client.cancel();
            self.transfers.cancel();
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
        self.message = None;
        let snap = self.buffer.client.latest().snapshot.clone();
        if let Some(result) = self.search.handle(&intent, snap) {
            self.report(result);
            return;
        }
        match intent {
            Intent::Edit(command) => self.edit(command, true),
            Intent::Copy { cut } => {
                let result = self.transfers.copy(&self.buffer.client, cut);
                self.report(result);
            }
            Intent::Paste => {
                let result = self
                    .transfers
                    .paste(&self.buffer.client, self.search.stamp());
                self.report(result);
            }
            Intent::ReplaceMatch { all } => {
                if !self.search.state.find_query.is_empty() {
                    self.search.state.cancel_navigation();
                    let result = submit(
                        &self.buffer.client,
                        Command::Replace {
                            query: self.search.state.find_query.clone(),
                            replacement: self.search.state.replace_query.clone(),
                            all,
                        },
                    );
                    match result {
                        Ok(sequence) => self.snap_after = Some(sequence),
                        Err(error) => self.message = Some(error),
                    }
                }
            }
            Intent::ToggleWrap => {
                self.viewport.wrap = !self.viewport.wrap;
                self.viewport.input(true);
            }
            Intent::ToggleLineNumbers => {
                self.viewport.line_numbers = !self.viewport.line_numbers;
                self.viewport.input(true);
            }
            Intent::ToggleDiagnostics => self.hud = !self.hud,
            Intent::Cancel => self.edit(EditorCommand::ClearSelection, false),
            _ => {}
        }
        self.window.request_redraw();
    }
    fn pointer_move(&mut self, x: f32, y: f32) {
        self.mouse.position = (x, y);
        if self.locked() {
            return;
        }
        if self.viewport.scroll_drag(x) {
            self.window.request_redraw();
            return;
        }
        if self.mouse.dragging {
            let (line, col) = self.viewport.point(x, y);
            self.edit(EditorCommand::Drag { line, col }, false);
        }
    }
    fn pointer_button(&mut self, state: ElementState) {
        if state == ElementState::Released {
            self.mouse.dragging = false;
            self.viewport.horizontal.end_drag();
            return;
        }
        let (x, y) = self.mouse.position;
        let size = self.window.inner_size();
        if self.appearance.open {
            let before = self.appearance.settings.clone();
            self.appearance
                .click(size.width as usize, size.height as usize, x, y);
            self.appearance_changed(before);
            return;
        }
        if appearance_ui::status_button(size.width as usize, size.height as usize).contains(x, y) {
            self.intent(Intent::ToggleAppearance);
            return;
        }
        if self.locked() {
            return;
        }
        if let Some(hit) = search_ui::hit(
            self.search.state.mode,
            self.window.inner_size().width as f32,
            x,
            y,
        ) {
            match hit {
                BarHit::Focus(field) => self.search.focus(field),
                BarHit::Previous => self.intent(Intent::NextMatch { forward: false }),
                BarHit::Next => self.intent(Intent::NextMatch { forward: true }),
                BarHit::Replace(all) => self.intent(Intent::ReplaceMatch { all }),
                BarHit::Close => self.intent(Intent::SearchClose),
            }
            self.window.request_redraw();
            return;
        }
        if self.viewport.scroll_press(x, y) {
            self.mouse.dragging = false;
            self.window.request_redraw();
            return;
        }
        if self
            .viewport
            .current
            .as_ref()
            .is_none_or(|f| y >= f.text_height as f32)
        {
            return;
        }
        let gutter = self
            .viewport
            .current
            .as_ref()
            .is_some_and(|f| x < f.config.text_x());
        let kind = self.mouse.press(gutter);
        let (line, col) = self.viewport.point(x, y);
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
                    self.message = Some(format!("Appearance preferences were not saved: {error}"));
                }
            }
            self.window.request_redraw();
        }
        for done in self.buffer.client.drain() {
            if let Err(error) = done.result {
                self.message = Some(error);
            }
        }
        let result = self.transfers.poll(&self.buffer.client, &mut self.search);
        self.report_if_error(result);
        match self
            .files
            .poll(&self.buffer.client, self.transfers.busy(), &self.window)
        {
            Ok(true) => {
                target.exit();
                return;
            }
            Err(error) => self.message = Some(error),
            _ => {}
        }
        if self.files.reset {
            self.files.reset = false;
            self.viewport.reset();
            self.search.reset();
            self.mouse = Mouse::new();
            self.snap_after = None;
            self.message = None;
        }
        let published = self.buffer.client.latest();
        if self
            .snap_after
            .is_some_and(|s| published.completed_through >= s)
        {
            self.snap_after = None;
            self.viewport.input(true);
        }
        if !self.files.locked()
            && published.busy.is_none()
            && published.completed_through >= self.buffer.client.barrier()
        {
            if self.search.state.mode != SearchBarMode::Closed {
                self.search.state.submit(published.snapshot.clone());
            }
            if let Some((revision, (start, end))) = self.search.state.poll() {
                match submit(
                    &self.buffer.client,
                    Command::Edit(EditorCommand::SelectOffsets {
                        revision,
                        start,
                        end,
                    }),
                ) {
                    Ok(sequence) => self.snap_after = Some(sequence),
                    Err(error) => self.message = Some(error),
                }
            }
        }
        if let Some(error) = self.search.state.error.take() {
            self.message = Some(error);
        }
        let title = if let Some(message) = &self.message {
            format!(
                "{message} — {}",
                self.files.documents.title(&published.snapshot)
            )
        } else {
            format!(
                "{}{}",
                self.files.documents.title(&published.snapshot),
                if self.transfers.busy() {
                    " (Clipboard…)"
                } else if published.busy.is_some() {
                    " (Working… Esc to cancel)"
                } else {
                    ""
                }
            )
        };
        self.window.set_title(&title);
        if self.viewport.blink.elapsed() >= Duration::from_millis(500)
            || self.search.state.pending
            || published.busy.is_some()
        {
            self.window.request_redraw();
        }
    }
    fn report_if_error(&mut self, result: Result<(), String>) {
        if let Err(error) = result {
            self.message = Some(error);
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
        let start = Instant::now();
        let p = self.buffer.client.latest();
        let frame = self.viewport.prepare(
            &p.snapshot,
            &self.font,
            size.width as usize,
            size.height as usize,
        );
        self.search.state.set_view(&p.snapshot, &frame.layout);
        editor_view::paint(
            &mut self.draws,
            &mut self.font,
            &mut self.ui_font,
            &ViewModel {
                palette: &self.appearance.palette,
                snap: &p.snapshot,
                layout: &frame.layout,
                config: &frame.config,
                selection: &p.selection,
                search_state: &self.search.state,
                width: size.width as usize,
                height: size.height as usize,
                text_height: frame.text_height,
                gutter_w: frame.gutter_w,
                line_digits: frame.line_digits,
                text_start_x: frame.text_start_x,
                cur_line: frame.cur_line,
                cur_col: frame.cur_col,
                word_wrap: self.viewport.wrap,
                show_line_numbers: self.viewport.line_numbers,
                caret_visible: self.viewport.caret,
                current_scrollbar: self.viewport.bar,
                show_perf_hud: self.hud,
                bom: self.files.documents.bom,
                ending: self.files.documents.ending,
                gpu_name: &self.renderer.description,
                frame_start: start,
                layout_time: frame.layout_time,
            },
        );
        self.appearance.paint(
            &mut self.draws,
            &mut self.ui_font,
            size.width as usize,
            size.height as usize,
        );
        self.viewport.current = Some(frame);
        if let Err(error) = self.renderer.render(&self.draws, size.width, size.height) {
            self.failed = true;
            let _=self.files.enqueue(Action::ReportError(format!("GPU rendering failed: {error}. Texted will close; you can save unsaved changes before closing.")));
            let _ = self.files.enqueue(Action::Close);
        }
    }
}
