# Performance and architecture hardening plan

Status: proposed; implementation has not started. Specification and acceptance criteria: [hardening spec](performance-hardening-spec.md).

Baseline: `af9ef43`, including the native document workflow and Metal renderer. Rebase onto the accepted baseline as those branches advance and rerun characterization checks after conflicts.

## Delivery approach

Use small PRs with one reviewable purpose. Fix the measured highlight problem first. Extract only the seams a change needs, then perform the larger structural moves as behavior-preserving changes. Follow those moves with explicit asynchronous-command changes so reviewers can distinguish relocation from changed behavior.

| Phase | Work item | Depends on | Reviewable result |
|---|---|---|---|
| 0 | A0: Preserve audit fixtures and characterize behavior | Baseline | Repeatable benchmarks and invariant tests |
| 1 | A1: Bound visible highlight work | A0 | Pure highlight builder and regression fix |
| 2a | A2: Add streaming, cancellable search primitives | A0 | Scanner, generation checks and single-result seeks |
| 2b | A3: Introduce bounded search controller | A1, A2 | Bounded jobs/results/caches, accurate count progress |
| 2c | A4: Stream replacement planning | A2 | Replace operations without a full match vector |
| 3 | A5: Make word selection rope-native | A0 | Local scanning without whole-line copies |
| 4a | A6: Extract buffer hosting and command contracts | A0 | Relocated server code, unchanged ordering |
| 4b | A7: Extract selection and input transitions | A6 | Windowless controllers and typed intents |
| 4c | A8: Extract view preparation and thin the shell | A1, A3, A7 | Cohesive view code and a small bootstrap/router |
| 5a | A9: Publish snapshots and ordered async completions | A3, A4, A6, A7 | Nonblocking UI command/snapshot path and barriers |
| 5b | A10: Move bulk edits and clipboard work off UI | A5, A9 | Cancellable planning, atomic commits and safe clipboard completion |
| 6 | A11: Run integrated acceptance and document results | A1–A10 | Before/after report, universal artifacts and updated architecture notes |

These dependencies allow independent work to be scheduled separately; they do not require parallel agents. Each item starts only when its needed interfaces are stable.

## Phase 0 — Reproducible baseline

### A0. Capture fixtures, counters and behavior

Create generated scenarios for:

- A 2,000,000-byte `a` line, query `a`, 81 visible matches in no-wrap mode, viewport at columns 0, 100,000 and 1,000,000; also test deep wrapping with its actual visible match count.
- Dense 2 MB and 64 MiB search inputs, sparse matches, no matches, cross-chunk UTF-8 queries and multiline matches.
- A 10,000,000-character `x ` line with a one-character word selected near the start and end.
- A block insertion over 10,000 rows of 2,048 characters, plus large paste/copy scenarios.
- Slow/failing/cancelled workers and clipboard operations under a controllable executor.

Capture candidate/row visits, bytes scanned after cancellation, match-cache capacity, queue occupancy and command/completion sequence numbers. Keep benchmark instrumentation out of normal product UI unless it is useful in diagnostics.

Characterize command and selection behavior before moving it: shortcut modifiers and physical keys, word/block drag anchors, selection replacement, undo grouping, replacement conflicts and native save/close barriers. Reuse existing tests where they already establish the invariant.

**Exit gate:** every audit workload is repeatable from a checked-in command; current measurements are recorded honestly. Tests intended to accompany a fix may demonstrate the existing failure on the development branch, but each merged PR must be green. Do not weaken an assertion to make the baseline pass.

## Phase 1 — Fix the highest measured UI cost

### A1. Extract and correct highlight preparation

1. Add a pure visible-highlight API in the layout domain.
2. Compute real visible byte intervals from row start/end columns.
3. Seek intersecting matches and clip by visible rows, including multiline matches.
4. Replace the nested highlight loops in the event handler.
5. Retain active-match styling and reject stale result generations.

Initially adapt the existing result vector; do not combine this PR with the search-worker rewrite. Shape the API so a bounded page or streamed match source can replace the vector in A3.

**Exit gate:** H1 tests pass; the million-column fixture considers at most 83 candidates. Record the new reference-machine timing against the 324–370 ms audit result.

## Phase 2 — Bound search resources

### A2. Build the reusable scanner

Add cancellable streaming scan and seek interfaces in the buffer/search domain. Preserve byte offsets and non-overlapping semantics. Implement Next/Previous and wraparound without collecting every document match.

Use deterministic cancellation hooks and a reference model to test empty, Unicode, multiline, cross-chunk and boundary cases. Keep compatibility adapters only where needed; production GUI paths must stop using the unbounded collector by the end of A3/A4.

**Exit gate:** cancellation respects the 64 KiB work bound and obsolete generations cannot publish results.

### A3. Extract the bounded search controller

Move worker lifecycle, query state, navigation and progress out of `main.rs`. Implement one active job, one latest pending job and one latest completion/progress mailbox. Enforce the 64 KiB query limit and 8 MiB persistent auxiliary-memory budget with named constants and observable test counters.

Replace all-document offset storage with bounded navigation/viewport pages. Connect those pages to A1. Count matches without retaining their offsets; distinguish incomplete progress from an exact final count. Preserve repeated accepted navigation steps.

**Exit gate:** dense 64 MiB search stays within the auxiliary-memory budget; query/document churn has bounded queues and no stale cursor, count or highlight changes. Worker shutdown and failure clear pending state.

### A4. Stream replacement planning

Prepare Replace All as a forward build of a new rope from the immutable base snapshot. Keep one operation result and no match-count-sized offset vector. Preserve cursor transformation, one-step undo and the existing conflict rule; add cancellation and explicit operation outcomes.

Implement Replace Next through the streaming seek API. Verify pinned query/replacement values remain attached to their operation when the UI query changes.

**Exit gate:** randomized reference-model tests pass; cancellation/conflict cannot partially edit the live document or history. One Replace All still requires one Undo.

## Phase 3 — Remove long-line selection copies

### A5. Use rope positions for word boundaries

Replace the `String`/`Vec<char>` helper and its callers with indexed rope traversal around the click. Keep current character classification and word-drag anchor transitions.

**Exit gate:** H5 work-count and correctness tests pass. Record short-word lookup timings on the 10-million-character line. Do not impose a constant-time promise on a genuinely long contiguous word.

## Phase 4 — Remove the monolith without changing behavior

### A6. Extract buffer hosting

Move request/response definitions, the buffer server loop and lifecycle/shutdown handling behind `buffer_host.rs`. Specify command identifiers, document identity and completion sequence numbers needed by A9, without making the transport asynchronous in the same relocation commit.

**Exit gate:** existing ordered editing, snapshot and undo tests pass through the extracted host. The event loop no longer contains the buffer server implementation.

### A7. Extract selection and input

Move selection transitions and drag anchors into `selection.rs`. Move shortcut decoding and editing intentions into `input.rs`/`commands.rs`. Use pure transitions producing typed effects; keep native key-event adaptation small.

**Exit gate:** characterization tests exercise these components without a window. Keyboard-layout-independent shortcuts, selection replacement and drag direction changes remain correct.

### A8. Extract the editor view and reduce the shell

Move gutter, text, selection/search overlay, status/search bar and diagnostics command preparation into `editor_view.rs` or focused view functions. Consume immutable view models. Keep geometry in the layout domain and resource submission in the existing GPU renderer.

Reduce `main()` to startup/registration and the application shell to routing/scheduling. Review ownership and interfaces to ensure the old monolith has not reappeared in an `Editor` method or a broadly mutable context.

**Exit gate:** H4 boundaries and bootstrap/router line targets are met or a single-responsibility exception is documented. GUI checks preserve drawing order, clipping, hit-testing, wrapping and resize behavior.

## Phase 5 — Keep expensive work out of the event loop

### A9. Add snapshot publication and asynchronous completions

Publish shared snapshots with ordered completion sequence numbers. Replace UI-thread blocking reply waits with accepted-command/completion events. Define finite pending capacity, explicit saturation behavior and coalescing rules; retain all accepted editing commands in order.

Implement document-action barriers before migrating bulk operations: a save snapshot includes prior accepted edits, and Close/New/Open cannot bypass an earlier pending mutation. Keep the saved-root dirty-state rule unchanged.

**Exit gate:** deterministic ordering, backpressure, stale-document, Save As cancellation and close/save barrier tests pass. Redraw reads the published snapshot without a blocking buffer request.

### A10. Migrate bulk edits and clipboard work

Add snapshot-based planning for block edits, large paste and extraction, with explicit operation IDs, progress, cancellation and named size/work thresholds. Keep workers outside the buffer owner; commit a valid plan as one atomic undoable swap.

Allow newer edits to invalidate a plan and report that conflict. Keep preparation and commit cancellation distinct so a completed operation cannot later be reported as cancelled.

Move clipboard pipe/process waiting out of the native event loop, or introduce a measured native implementation. Preserve clipboard-write-before-cut ordering, invocation context for paste and child timeout/reaping behavior. Remove scalar iteration used only to obtain indexed line lengths.

**Exit gate:** H3 heartbeat, clipboard failure, conflict, cancellation and ordering tests pass. The 10,000-row operation does not synchronously occupy the event loop; reference-machine dispatch p95 meets the 2 ms target.

## Phase 6 — Validate the integrated editor

### A11. Acceptance, delivery and documentation

1. Run the full workload matrix and record commit, hardware, OS, toolchain, warmups, sample counts and percentiles.
2. Compare H1 candidate counts/timings, H2 storage/cancellation bounds, H3 UI dispatch and H5 inspected characters/timings against the specification.
3. Run formatting, Clippy, workspace and packaging tests on ARM and Intel. Run the explicit Metal validation and native GUI checklist where applicable.
4. Build and verify universal ZIP/DMG artifacts with the existing pipeline.
5. Update `README.md`, performance notes and the handover entry point with the final module map, behavior changes and remaining limits.

**Exit gate:** all H1–H5 criteria are met, CI is green, and no unresolved command-ordering, dirty-state or data-loss regression remains. A normal release can then use the existing version/tag delivery process.

## Review and risk controls

| Risk | Required control |
|---|---|
| A stale result edits or highlights a different document | Document/revision/generation fences and deterministic document-switch tests |
| A partial bulk operation corrupts undo or drops newer input | Snapshot planning, atomic commit, explicit conflicts and ordered-command tests |
| A save/close races asynchronous input | Completion barriers and saved-root regression tests |
| Search stays expensive despite moving to a worker | Work counters, bounded storage/mailboxes and cancellation tests |
| Refactoring changes shortcut, selection or view behavior | Characterization before relocation; native GUI checklist after integration |
| Machine variability makes CI flaky | Deterministic algorithmic CI gates; opt-in reference timing thresholds |

On a failed gate, keep the preceding accepted behavior available and fix the smallest responsible item. Do not combine a failed algorithm change with another architectural move or silently restore an unbounded search path to hide a regression.
