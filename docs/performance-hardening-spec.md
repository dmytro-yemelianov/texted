# Performance and architecture hardening specification

Status: implemented. See [hardening results](performance-hardening-results.md) for validation evidence and remaining limits. The acceptance criteria below retain the original contract.

Baseline: `af9ef43` (`feat/gpu-renderer`), audited 2026-11-05. Implementation sequence: [hardening plan](performance-hardening-plan.md). Historical measurements and implemented work: [performance notes](performance.md).

## 1. Outcome and scope

Keep editing, scrolling and native document actions responsive as document length, match count and selection size grow. Replace the oversized event-loop function with cohesive components whose behavior can be tested without a native window.

This work addresses five audit findings:

| ID | Finding | Current evidence |
|---|---|---|
| H1 | Highlighting processes off-screen matches | On a 2,000,000-byte single line of `a`, searching for `a` at column 1,000,000 visits 1,000,082 matches to draw 81 highlights. The reproduced CPU loop took 324–370 ms. |
| H2 | Search materializes every match and cannot cancel an active scan | The same input produces 32,000,000 bytes of live offset pairs, with 33,554,432 bytes of vector capacity. A dense 64 MiB input would require about 1 GiB for offset pairs alone. Job and result channels are unbounded. |
| H3 | Long edits and clipboard operations block the UI | The UI waits for buffer RPC and clipboard subprocesses. Inserting into 10,000 rows of 2,048 characters took 53–73 ms in the buffer operation alone. |
| H4 | The event loop owns too many responsibilities | `main.rs` has 3,022 lines; `main()` occupies about 2,490, including buffer hosting, input, selection, search coordination, layout and drawing. |
| H5 | Word selection copies an entire physical line | The caller creates a `String`, then the helper collects `Vec<char>`. Selecting a one-character word near the start of a 10,000,000-character line took 6–16 ms. |

Measurements above are local release-mode CPU reproductions on Apple M5, not keyboard-to-pixel or GPU execution measurements. Ordinary 64 MiB viewport layout remained about 3.5–9 µs; uncontended IPC averaged about 3 µs. Those results do not justify replacing the rope or GPU renderer.

### Non-goals

- New editor features such as syntax highlighting, tabs, IME, grapheme shaping or mmap storage.
- A replacement renderer, IPC framework or document-storage engine.
- Broad splitting of otherwise cohesive `Documents`, `TextBuffer` or GPU resource types based on file length alone.
- Display-latency, display-FPS or whole-process RAM claims from CPU counters.

## 2. Required invariants

The following requirements apply throughout migration, including intermediate PRs:

1. The buffer service remains the sole owner of the live mutable document. Workers operate on shared immutable snapshots.
2. Accepted document commands execute in order. Coalescing may replace obsolete searches and viewport requests; it must not silently discard accepted text edits, cuts, pastes or undo commands.
3. Asynchronous results carry document identity and their relevant revision, generation and operation identifiers. Results for an obsolete document or request cannot alter current text, cursor, selection or highlights.
4. A bulk mutation commits atomically as one undo transaction. Cancellation or rejection leaves the live document and its history unchanged.
5. Save records the exact written rope root. Edits after that snapshot remain dirty; undo back to the saved root remains clean.
6. Save, Close, New and Open account for earlier accepted mutations before inspecting dirty state or taking their save snapshot. Existing Save/Discard/Cancel behavior and action ordering remain intact.
7. Preserve UTF-8 boundaries, current non-overlapping search semantics, wraparound navigation, keyboard-layout-independent shortcuts, BOM/line endings and atomic file-save behavior.

## 3. H1 — Viewport-bounded highlighting

### Design

Move highlight computation into a pure layout function that produces clipped spans or rectangles. Its inputs are a snapshot, visible rows, a revision-valid match source and the active match identity. GPU encoding remains a separate consumer.

- Derive byte intervals from each visible row's `start_col` and `end_col`, including horizontal scrolling and `top_wrap_row`.
- Merge adjacent or overlapping scan windows. When searching a window, allow the bounded look-behind needed to find a match that starts before its left edge.
- Seek to the first intersecting match rather than replaying the physical line's preceding matches.
- Iterate visible rows when clipping a multiline match. Never walk every document line spanned by that match.
- Preserve active-match color, Unicode offsets and clipping at both edges. Do not paint results from another revision, including when the search bar is closed.
- Keep this API independent of a complete `Vec` of document matches so H2 can replace the match source.

### Acceptance

- In the audit fixture, processing at column 1,000,000 considers only the visible interval and boundary overlaps: at most 83 match candidates for the 81 visible matches.
- For deep wrapping, candidate visits are bounded by the actual visible match count plus at most two boundary candidates per merged window; the 83-candidate limit applies to the one-row, 81-match fixture above.
- A multiline match crossing a viewport visits at most the visible rows, regardless of its off-screen span.
- Tests cover an empty viewport, exact row boundaries, UTF-8 text, adjacent matches, multiline matches, an active match and stale results.
- Reference-machine p95 highlight preparation is below 1 ms for the audit fixture at both columns 0 and 1,000,000. Candidate counts are the portable CI gate.

## 4. H2 — Bounded, cancellable search

### Execution and ownership

Introduce a search controller with one running job, one replaceable latest pending job and one replaceable result/progress mailbox. A newer search generation invalidates the active scan and replaces pending work.

Search scanning must expose streaming match delivery, cancellation and progress. Check cancellation at rope-chunk boundaries with no more than 64 KiB of additional input processed between checks. Query preprocessing must also be cancellable. A generation check is required before publishing or applying any result.

The initial implementation limits a find query to 64 KiB of UTF-8. Oversized input is rejected visibly, never silently truncated. Keep this limit named and documented so it can be changed deliberately.

### Storage and behavior

- The GUI must not retain an offset pair for every document match. Use bounded pages/caches for navigation and viewport work; stream exact match counting separately.
- Keep persistent search-owned auxiliary storage within 8 MiB per editor session, including query/preprocessing data, pending/completed payloads and retained match-page capacities. This budget excludes the shared document rope and normal viewport drawing buffers.
- This budget covers automatic find/count/navigation/highlight bookkeeping. User-supplied replacement and clipboard text is accounted separately as operation data, with bounded retained payloads and explicit preflight failure if an operation's named capacity limit is exceeded.
- Count shared payloads once and include allocated capacity in the accounting. Workers may hold at most one active and one pending snapshot; cancellation releases obsolete payloads promptly.
- While counting, show an explicit searching/progress state. Publish an exact final count only after the current generation finishes successfully. Do not present a capped count as exact.
- Next/Previous searches preserve current wraparound and non-overlapping semantics. Repeated accepted navigation requests retain their intended steps; they may be represented by a bounded accumulator rather than an unbounded queue.
- Cancellation and query changes cannot publish old counts, move the cursor using old offsets or restore old highlights.
- Failed workers produce a recoverable error and clear pending state. They must not leave the application polling forever.

### Replacement compatibility

Replace Next streams to the needed match rather than collecting every match. Replace All builds a replacement rope incrementally from the original snapshot; it must not require an unbounded match vector or mutate the live document once per match.

Pin the query and replacement text to the explicit mutation operation. Commit only if its document, base revision and required cursor/selection context still match. A stale plan is rejected visibly; automatic rebasing is outside this specification. A query change alone must not accidentally reinterpret a previously accepted replacement operation.

### Acceptance

- Dense 2 MB and 64 MiB fixtures complete counting with correct totals while staying within the 8 MiB search-owned storage budget.
- A deterministic cancellation test publishes a newer generation mid-scan and verifies the 64 KiB maximum additional scanning work. A queued superseded generation never starts.
- Queue/mailbox occupancy never exceeds the specified bounds during rapid query and document changes.
- Closing search, changing documents, worker failure and cancellation release obsolete jobs and caches.
- Search results and replacements match a simple reference model on randomized small UTF-8 documents, including cross-chunk and multiline queries.
- Replace All has no match-count-sized offset vector, commits once, undoes once and leaves no partial edit on cancellation/conflict.

## 5. H3 — Responsive command execution

### Bulk document operations

Extract buffer hosting behind a typed interface. Prepare expensive block edits, large paste/replacement operations and large extraction work on snapshot workers. Apply a completed mutation with a short, fenced root swap on the buffer owner.

- The native event loop must not wait for a bulk computation, child process, disk operation or worker completion.
- Use an operation ID and a named work/size threshold to classify expensive commands. Benchmark the threshold; do not hide a large paste behind a nominally small `Insert` request.
- Keep ordinary editing usable while a worker prepares a plan. An intervening edit may invalidate the plan; show a cancelled/conflict outcome without overwriting newer edits.
- Publish progress and provide cooperative cancellation for long operations. Each operation is reported as committed, cancelled or failed.
- No worker mutates the live buffer or writes directly to selection/view state.
- Replace `.chars().count()` with indexed rope metadata where the operation only needs a line's length. Preserve required scans when producing actual text.

### Short commands and snapshot publication

Keep ordered command acceptance and completion separate. Publish the latest immutable snapshot to the UI with completion sequence numbers so redraw does not need a blocking `GetSnapshot` RPC.

A finite pending-command limit and explicit backpressure are required. Queue saturation must be reported, never converted into a dropped accepted edit. Coalesce only observational requests. File actions wait for or explicitly cancel earlier pending mutations before their dirty-state/save barrier completes; asynchronous execution must not save an older displayed snapshot by accident.

Short synchronous RPC may remain during the staged migration, with its callers and observed cost documented. The final UI input and redraw paths use completion events/snapshots rather than blocking reply waits.

### Clipboard

Keep subprocess creation, pipe transfers and waiting out of the event loop, or use a native clipboard implementation with measured bounded main-thread work.

- Copy/cut extraction is tied to its invocation snapshot.
- Cut deletes only after clipboard write succeeds and the original edit context is still valid. A failed write or stale context cannot delete text.
- Paste completion is fenced to its invocation document and edit context. Changed context produces an explicit conflict/retry outcome.
- Workers support cancellation and a named subprocess deadline. A timeout terminates/reaps the child and leaves document state intact.
- Do not silently truncate clipboard content or retain an unbounded queue of large payloads.

### Acceptance

- A test-controlled slow worker or clipboard process cannot prevent UI heartbeat, redraw or cancellation events. Readiness is coordinated with barriers/fake executors rather than arbitrary sleeps.
- The 10,000-row block fixture produces one undo transaction and has no UI-thread wait for its full execution.
- Deterministic tests cover interleaved typing, a stale bulk result, cut failure, paste into another document, cancellation, queue saturation and ordered completions.
- Save and Close tests cover an earlier accepted mutation, a later edit during save, cancelled Save As and a worker failure.
- Reference-machine p95 UI dispatch/command acceptance is below 2 ms during the bulk fixture. This excludes worker completion and display latency.

## 6. H4 — Component boundaries

Keep these responsibilities behind narrow interfaces; proposed filenames may change during implementation:

| Component | Owns | Must not own |
|---|---|---|
| `main.rs` | Startup and event-loop registration | Input branches, buffer server loop, drawing or search algorithms |
| `app.rs` | Native-event adaptation, routing effects and scheduling | Editing algorithms or widget drawing |
| `input.rs` / `commands.rs` | Shortcut mapping and typed user intents | File I/O, GPU resources or blocking RPC |
| `selection.rs` | Selection transitions, anchors and drag semantics | Clipboard processes or rendering |
| `search.rs` | Generations, budgets, workers, navigation/count progress | Native event dispatch or GPU calls |
| `buffer_host.rs` | Ordered commands, live buffer ownership, fenced commits and snapshot publication | Window state or widget geometry |
| `editor_view.rs` | UI drawing-command preparation, status/search bars and overlay composition | Document mutation, jobs or IPC waits |
| Existing document/layout/graphics/GPU modules | Their current cohesive domains and the new pure highlight function | Unrelated controller responsibilities |

Controllers return typed commands/effects; a windowless test can exercise their transitions. Shared state is passed explicitly or owned by its controller, rather than exposed as a large mutable context passed to every helper.

### Acceptance

- `main()` is a bootstrap function, targeting at most 100 lines. Event adaptation/routing targets at most 150 lines.
- Input, selection and search behavior can be tested without constructing winit, Metal or native dialogs.
- The app shell does not contain the old editing, selection or drawing branches moved wholesale into another method.
- No new dependency cycle, generic service registry or cross-controller access to private mutable state is introduced.
- Line targets are review signals, not a reason to fragment cohesive declarative GPU setup or state-machine code. Each exception must identify its single responsibility.
- Preserve current commands, visual ordering, hit-testing, wrapping, scrolling, native file actions and undo behavior through characterization tests and the GUI checklist.

## 7. H5 — Rope-native word selection

Accept a rope/snapshot position instead of a whole-line `String`. Use indexed character access or a bidirectional rope iterator around the click. Scan only the contiguous word, whitespace or punctuation run being selected.

Preserve current classification: alphanumeric/underscore, whitespace and punctuation. Scanning must not cross the physical line boundary. Preserve anchor behavior for double-click dragging and existing scalar-character columns.

### Acceptance

- No whole-line `String` or `Vec<char>` is constructed by word selection or word dragging.
- Work scales as O(log N + W), where W is the selected contiguous run. A genuinely long single word may require a long scan.
- In a 10,000,000-character `x ` fixture, selecting a one-character word near either end inspects only its local run and boundaries. Counted character inspection is independent of line length.
- Tests cover Ukrainian text, underscores, whitespace, punctuation, empty lines, out-of-range clicks, rope-chunk boundaries and drag anchor direction changes.
- Reference-machine p95 boundary lookup is below 0.5 ms for the short-word fixture, excluding layout and GPU work.

## 8. Validation and completion

Use generated fixtures; do not commit large documents or machine-specific temporary paths. Preserve fixture parameters and invocation commands in the repository.

- Functional CI gates: reference-model correctness, operation ordering, undo/dirty-state invariants, stale-result rejection, queue/storage bounds and algorithmic work counters.
- Reference performance runs: release mode, recorded OS/hardware/toolchain/commit, at least three warmups and 30 samples, with p50/p95/p99 and workload sizes. Timing thresholds are opt-in on the reference machine, not assertions on shared CI runners.
- Memory reporting: distinguish match/cache capacity, retained snapshots and measured process RSS. The 8 MiB contract is auxiliary search storage, not total application RAM.
- Run formatting, Clippy with warnings denied, the workspace tests and packaging tests for implementation PRs. Keep shader validation and the explicit Metal test where GPU-facing interfaces change.
- GUI acceptance covers native Open/Save As/quit guards, typing/navigation, stream/block/word selection, undo/redo, search/replace, wrap/horizontal scrolling, resizing and BOM/CRLF preservation using isolated temporary files.
- Finish with universal ARM/Intel packaging and clean CI. Record before/after measurements without claiming that CPU preparation time measures input-to-display latency.

Completion requires all H1–H5 acceptance criteria and no unresolved data-loss or ordering regression. Update the performance notes and handover entry point with the final component map and remaining limitations.
