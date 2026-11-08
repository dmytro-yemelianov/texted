# macOS exit lifecycle

The exit hang was reproduced after saving a changed tab. The main thread was joining `DocumentStats` while its worker waited in macOS main-queue dispatch. An earlier sample also showed the last `Arc<Window>` being destroyed by that worker. In winit 0.29, `Window::request_redraw()` routes through `maybe_queue_on_main`, which currently calls synchronous `maybe_wait_on_main`. Keeping the window alive longer fixes final-release ordering, but does not fix a wake callback already waiting for UI dispatch.

All buffer, search, clipboard, horizontal-measurement, statistics, preferences and file-I/O callbacks share `Wake`. In the window backend it owns an `EventLoopProxy`, never a native window; notifications enqueue `AppEvent::Wake` and only the main handler requests redraw. In the terminal backend it sets an atomic redraw flag. Shared in-app dialogs use asynchronous reply tickets and the same wake route; the old native dialog futures and `DialogWake` are removed. The native proxy safely ignores an already-closed event loop. The host/window remains the last shell field destroyed, after renderer and document state.

Save/close barriers are unchanged: accepted edits and clipboard work settle before dirty checks, and Save completes its durable write before closing the tab. In the window, Quit keeps the session instead of asking (see `session_store.rs`): it waits for pending file work and open dialogs, writes drafts and `session.json`, then exits. If the session cannot be written, or another instance holds its lock, Quit falls back to asking per changed tab, and Cancel stops quitting. Normal shutdown still joins owned workers and flushes accepted appearance settings. No thread is abandoned to hide a shutdown deadlock.

Before showing a shared file or dirty-close dialog, the parent is revealed, restored from minimization and focused. Repeating Quit while a file operation is already pending also reveals that window, so a hidden dialog cannot silently hold exit open.

Regression checks:

- Shared dialog requests and completion tickets operate without a window handle or native event loop.
- Per-tab services, statistics and preferences can stop without processing queued UI events; accepted preferences are still saved.
- Native empty and two-clean-tab exits complete within the three-second deadline.
- Native dirty-tab Save exits with edited text and Ukrainian Unicode intact; the clean tab is unchanged.
- The installed app is checked through its normal Launch Services entry point, including Cancel and retrying Save/quit.

For native checks, use a separate QA bundle identifier and isolated `TEXTED_SETTINGS_PATH`. Verify that each process actually disappears after Quit; a hidden window is not proof of exit. Do not leave repeated `/Applications/Texted.app` processes running across updates. Inspect and preserve unsaved work before closing any existing instance.
