# macOS exit lifecycle

The exit hang was reproduced after saving a changed tab. The main thread was joining `DocumentStats` while its worker waited in macOS main-queue dispatch. An earlier sample also showed the last `Arc<Window>` being destroyed by that worker. In winit 0.29, `Window::request_redraw()` routes through `maybe_queue_on_main`, which currently calls synchronous `maybe_wait_on_main`. Keeping the window alive longer fixes final-release ordering, but does not fix a wake callback already waiting for UI dispatch.

All buffer, search, clipboard, horizontal-measurement, statistics, preferences and file-I/O callbacks now share an event-loop notification. `Wake` owns an `EventLoopProxy`, not a native window. Worker notifications enqueue `AppEvent::Wake`; only the main-thread event handler requests redraw. Native dialog futures use that same queue through `DialogWake`. The proxy safely ignores an already-closed event loop. The window remains the last shell field destroyed, after renderer and document state.

Save/close barriers are unchanged: accepted edits and clipboard work settle before dirty checks, Cancel stops quitting, and Save completes its durable write before closing the tab. Normal shutdown still joins owned workers and flushes accepted appearance settings. No thread is abandoned to hide a shutdown deadlock.

Before showing a file dialog or dirty-close sheet, the parent is revealed, restored from minimization and focused. Repeating Quit while a file operation is already pending also reveals that window, so a hidden sheet cannot silently hold exit open.

Regression checks:

- Background dialog completion can wake an undispatched UI queue without a window handle.
- Per-tab services, statistics and preferences can stop without processing queued UI events; accepted preferences are still saved.
- Native empty and two-clean-tab exits complete within the three-second deadline.
- Native dirty-tab Save exits with edited text and Ukrainian Unicode intact; the clean tab is unchanged.
- The installed app is checked through its normal Launch Services entry point, including Cancel and retrying Save/quit.

For native checks, use a separate QA bundle identifier and isolated `TEXTED_SETTINGS_PATH`. Verify that each process actually disappears after Quit; a hidden window is not proof of exit. Do not leave repeated `/Applications/Texted.app` processes running across updates. Inspect and preserve unsaved work before closing any existing instance.
