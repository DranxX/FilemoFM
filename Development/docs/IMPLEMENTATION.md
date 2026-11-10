# FilemoFM implementation record

Date: October 15, 2026. The scope follows `Plan/BigChanges/PLAN.md`, `DESIGN.md`, all 13 available images, and the runtime package approval in the conversation. The original plan remains as a historical specification.

The source changes were written. **Update, October 19, 2026:** the source passed `cargo check`, `cargo build`, and `cargo clippy` on Linux x64, and the debug app was launched in WSLg on X11 and Wayland. Feature-level runtime testing (interaction, file operations, vector search, Agent) and Windows builds were not performed. This is not a release that meets every acceptance gate in the plan.

## Plan-to-source mapping

| Plan area | Implementation | Main locations |
| --- | --- | --- |
| Identity and portability | FilemoFM package/title/log/database, root from executable, writable directories, single-instance lock, Windows GUI subsystem, safe startup errors | `src/main.rs`, `src/infrastructure/config.rs`, `src/shared/constants.rs` |
| Authority and identity | SQLite migration v4, native path bytes, stable UUIDs, filesystem identity, revisions, reference relations, tombstones when a path is reused | `src/infrastructure/sqlite.rs`, `src/shared/utils.rs`, `src/shared/models.rs` |
| Browsing and watcher | Batched one-level listings, generation/cancellation, worker enrichment, non-recursive debounced watcher, bounded RAM snapshot | `src/services/scanner.rs`, `src/ui/app.rs` |
| Navigation and System | Back/Forward/Up, editable address, actual roots, file paths resolving to their parent, lazy tree, favorites/frequent folders, OS/UNC/WSL shares, System workspace and queue | `src/ui/breadcrumb.rs`, `src/ui/sidebar.rs`, `src/infrastructure/system.rs`, `src/ui/app.rs` |
| Folder size | DFS with entry/time limits, no following symlinks/reparse points or other mounts, SQLite TTL cache, unknown/partial/stale labels, selection totals without overlap | `src/services/scanner.rs`, `src/ui/table_view.rs`, `src/ui/app.rs` |
| Clipboard and operations | Native file clipboard and cut intent, single-worker queue, identity/revision preflight, Skip/Rename/Replace/Merge conflict handling, rename, cross-volume copy/move, cancellation and journal | `src/infrastructure/clipboard.rs`, `src/services/operations.rs` |
| Trash and restore | OS trash by default, app trash fallback only when allowed, manifest with native path/UUID/digest, restore without overwrite, confirmation for permanent delete | `src/services/operations.rs`, `src/ui/app.rs` |
| Keyword search and fff | Dork grammar, scope, SQLite FTS5/BM25 and substring search, opt-in fff 0.11.0 on selected roots, results hydrated into SQLite, diagnostic when the name index is unavailable | `src/shared/query.rs`, `src/services/search.rs`, `src/infrastructure/name_index.rs` |
| Tags/descriptions/references | Tag CRUD, description CAS, canonical stable-ID markers, human-readable editor labels, selection formatting, Write/Preview/Split, reference picker and relink, dirty Save/Discard/Cancel | `src/ui/preview_panel.rs`, `src/infrastructure/sqlite.rs`, `src/shared/utils.rs` |
| UI and settings | Eight views, sort/group, virtual rows, natural sort in worker, hideable tabbed sidebar and inspector, narrow-layout overlay, dark/light/system themes, font/scale/density, nine Settings tabs | `src/ui/`, `src/services/presentation.rs` |
| Model download | Official HF artifact review, immutable revision, size/SHA-256, HTTPS/DNS/redirect policy, `.part` resume, disk/file limits, GGUF verification | `src/infrastructure/model_store.rs` |
| Native runtime | Executable and DLL/SO registration with hashes, minimum environment, loopback binding with temporary credentials, child ownership/shutdown, native Qdrant and CPU llama-server | `src/infrastructure/runtime.rs` |
| Embedding and sync | Qwen 1024-dimensional output, query instruction, prefix/L2 for selected dimensions, revision outbox, idempotent replay, metadata/description chunking, projection hash, auto/manual sync, idle unload | `src/services/vector.rs`, `src/infrastructure/runtime.rs` |
| Semantic and Hybrid | Metadata/scope/permission filtering before retrieval, revision validation after retrieval, RRF, keyword fallback diagnostics when vector search fails | `src/services/vector.rs`, `src/services/search.rs` |
| Rebuild and rollback | Separate collection generations, durable checkpoint, edit/delete catch-up, count/retrieval verification, atomic active pointer, rollback with the previous model and current metadata | `src/services/vector.rs`, `src/infrastructure/sqlite.rs` |
| Agent | Chat-compatible/Ollama endpoint, streaming/cancellation, session/environment credentials, deny-by-default roots/capabilities, bounded tool arguments/output, six read tools and three mutation proposals with approval | `src/services/agent.rs`, `src/services/access.rs`, `src/services/tools.rs`, `src/services/approval.rs` |
| MCP | Limited JSON-RPC stdio, initialize/initialized, read-tool whitelist, explicit permissions, no LAN listener, stdout reserved for protocol | `src/services/mcp.rs`, `src/main.rs` |
| Storage and recovery | Online SQLite backup, restore at startup with safety backup, unavailable-root relocation with review and revision checks, journal reconciliation, cache clearing separate from metadata | `src/infrastructure/sqlite.rs`, `src/services/operations.rs`, `src/ui/app.rs` |
| Packaging | Script for existing binaries, PE/ELF x64 validation, portable directories and checksum manifest | `scripts/package.py`, `docs/PORTABLE.md` |

Contracts live in `shared/traits.rs`. Services receive dependencies through constructors and do not import other services. Workers send DTO events to the UI. SQLite is the metadata authority; cache, fff, and Qdrant are projections. The old tag service wrapper was removed so writes do not have a duplicate path.

## Decisions

- fff replaced the fsearch candidate for Windows/Linux without forking its engine. FilemoFM can browse and store non-UTF-8 roots, but the fff adapter requires a UTF-8 root and reports a metadata fallback otherwise.
- SQLite substring search uses parameterized queries. No trigram index was added because no benchmark established a need. FTS5 remains for token search and description ranking.
- Traversal uses controlled `std::fs`; the old WalkDir dependency is not needed. Browsing remains shallow, with folder-size work handled separately.
- Native Qdrant sidecar and llama.cpp were selected with approval. CPU is the baseline; GPU/VRAM support is not promised.
- The default logical Qwen model points to an official GGUF artifact that is visible during review. Unsupported families and formats are rejected. Model-repository code is not executed.
- LAN access uses shares mounted by the OS. No pairing or direct peer protocol was added. MCP is read-only stdio.
- Secrets are session-only or supplied through the explicitly configured environment. Credentials are not persisted in plaintext.
- Custom icons, logos, and fonts remain deferred. The existing Phosphor icons and font are used.

## Verification performed

| Check | Result and limits |
| --- | --- |
| Rust environment | Rust/Cargo 1.98.1; the highest declared dependency MSRV is 1.92. Packages without a declared MSRV still need to be proven by compilation. |
| Lockfile and targets | `cargo fetch --offline`, locked/offline metadata, and dependency trees for Linux x64 and Windows MSVC x64 succeeded; 580 packages resolved, with one egui/eframe/epaint version at 0.34.3. No compiler was run at that time. |
| Source format | `cargo fmt --all -- --check` and `git diff --check`; format and syntax parsing do not prove type correctness. |
| SQL | Nine invariants using the source SQL schema/migration and scope predicates ran through Python SQLite 3.45.1. Rust adapters and bundled SQLite were not tested. |
| Python | Scripts were syntax-checked and the packaging CLI `--help` was checked; no application binary was created or run. |
| Crate APIs | Used signatures were reviewed against downloaded crate source, including fff, egui/eframe, reqwest, clipboard-rs, rusqlite, file-id, and Windows APIs. This does not replace type-checking. |
| Images | Design references were inspected as images. |
| Compilation (October 19) | Linux x64: `cargo check` and `cargo build` succeeded after fixing a missing `natural_compare` import in `ui/app.rs`; compiler warnings were cleared. Clippy reported style lints only (collapsible if, too many arguments, and similar), with no errors. Windows MSVC was not built. |
| Release builds (October 22) | Linux x86_64 and Windows x86_64 GNU compiled in release mode. Windows MSVC failed in `ring` because WSL could not run `lib.exe`; Linux ARM64 reached final linking but Zig rejected Rust's Cortex-A53 linker flag; Windows ARM64 could not link because the Zig sysroot lacked Windows ARM libraries. The x64 binaries were not smoke-tested. |
| Smoke run (October 19) | The debug binary ran in WSLg without crashing and created the portable layout (`config.toml`, `data/`, `logs/`, `models/`, `runtime/`, `trash/`) beside the executable. The one-level listing, background folder sizing (including partial state), sidebar, tabbed inspector, and status bar appeared. Wayland clipboard use fell back to X11 because the WSLg compositor does not support data-control. No automated click-through was performed. |
| Layout (October 19) | Updated to match the mockup: navigation, address, and search on one row (scope/mode/dork help in the filter menu); labeled toolbar with Delete/Sort/View/More menus and a Details pane toggle; Details table with full-row selection, Type column, right-aligned sizes, and colored tag chips. Pseudo-filesystem mounts (tmpfs/overlay) and `/mnt/wslg` no longer appear in System. |

Machine-readable reports are available in the [environment report](environment-2026-10-15.json) and [storage invariant report](storage-invariants-2026-10-15.json). The post-implementation audit is recorded separately in `anti-slop/`.

## Verification still required

Qdrant, llama-server, the GGUF model, and a complete Windows SDK were unavailable in the environment. Native runtime binaries must be selected from trusted publishers, with their versions recorded and verified. This source does not establish compatibility for a particular pair of native runtime releases.

Verification after an approved build should cover:

1. Compilation/type-checking on native Linux and Windows, then portable startup from a different CWD, a read-only directory, a moved package, and two simultaneous instances.
2. OS/Wayland clipboard, OS trash, merge/collision, cross-volume operations, full disk, cancellation, symlink/identity races, disconnected shares, and recovery after crashes at filesystem/database boundaries.
3. Actual Qwen fixtures, pooling, normalization, CPU RAM/latency, cold startup, idle unload, filtered recall against an exact baseline, and revision backlog while the model is offline.
4. Rebuild from 1024 to 512 dimensions and back, edits/deletes during backfill, restart/checkpoint/cancel/cutover/rollback, backup/restore, and relocation using fixture data.
5. Click-through of every control, keyboard/focus, description references, DPI/font scaling, narrow layouts, dark/light/system themes, composited contrast, and accessible names.
6. Adversarial Agent/MCP/network/download input, permission revocation, response limits, port impersonation, and prompt injection through metadata.

There are no claims for p95, recall, RAM use, sandbox security, screen-reader readiness, or release PASS without real results. File operations use validation and revalidation, but filesystem race resistance still needs OS testing. The process wrapper limits lifecycle and environment; it is not an OS sandbox.

Large downloads, runtime binaries, native libraries, installers, build output, and personal data are not included. The source archive contains the full project and a checksum manifest so changes can be reviewed without code excerpts.
