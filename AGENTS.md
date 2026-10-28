# FilemoFM Agent Guide

This guide records the architecture and behavior traced in the current source. The implementation is authoritative; check the call sites and current configuration before changing behavior.

## Before editing

1. Run `git status --short` and preserve existing user changes. This working tree may contain unrelated edits.
2. Read the complete function being changed, its callers, the relevant trait, and the event or persisted model it uses.
3. Trace the data path across the UI, service, repository, and filesystem or external process. Do not infer runtime behavior from a type name alone.
4. Keep changes within the requested scope. Do not reset, remove, or reformat unrelated files.
5. For behavior changes, identify the invariant being preserved and the failure path that should be checked.

## Project overview

- **Application:** FilemoFM, a portable Rust desktop file manager built with egui/eframe.
- **Toolchain:** Rust edition 2024; `Cargo.toml` declares Rust 1.92 or newer.
- **Targets:** Windows x86_64 MSVC and Linux x86_64 GNU. Linux builds enable both X11 and Wayland support.
- **Metadata authority:** SQLite. Qdrant and the optional name index are projections or search helpers, not sources of truth.
- **Composition roots:** `src/main.rs` wires startup dependencies; `src/ui/app.rs` wires GUI services.

## Source map

| Location | Responsibility |
| --- | --- |
| `src/main.rs` | Startup, portable paths, config and database construction, restore mode, MCP mode, and GUI launch. |
| `src/shared/models.rs` | Cross-layer data structures, commands, events, and settings models. |
| `src/shared/traits.rs` | Interfaces used to connect infrastructure, services, and the UI. |
| `src/shared/query.rs` | Search grammar and validation. |
| `src/shared/validation.rs` | Configuration validation. |
| `src/shared/utils.rs` | Path handling, native identity, file opening, and metadata-reference helpers. |
| `src/shared/i18n.rs` | Translation lookup and language selection. Route UI copy through this module. |
| `src/infrastructure/config.rs` | TOML settings, atomic writes, recovery of invalid settings, and executable-relative data paths. |
| `src/infrastructure/sqlite.rs` | SQLite schema and migrations, metadata repository, FTS, revisions, references, vector outbox, operation journal, backup, and restore. |
| `src/infrastructure/system.rs` | Filesystem validation and inspection plus platform integration. |
| `src/infrastructure/clipboard.rs` | OS file clipboard integration. |
| `src/infrastructure/name_index.rs` | Optional `fff-search` adapter for file-name discovery. |
| `src/infrastructure/model_store.rs` | Model artifact review, download, and verification. |
| `src/infrastructure/runtime.rs` | Local Qdrant and `llama-server` process lifecycle and adapters. |
| `src/services/scanner.rs` | Current-folder scan, watcher, and bounded folder-size work. |
| `src/services/search.rs` | Background keyword, semantic, and hybrid search orchestration. |
| `src/services/operations.rs` | Serialized filesystem operations, transfer logic, app trash, and crash recovery. |
| `src/services/vector.rs` | Embedding, Qdrant synchronization, semantic search, rebuild, and rollback. |
| `src/services/access.rs` | Agent capability and allowed-root policy. |
| `src/services/agent.rs` | Configured chat endpoint, streaming, tool loop, cancellation, and connection checks. |
| `src/services/tools.rs` | Agent/MCP tool schemas, argument validation, authorization, and dispatch. |
| `src/services/approval.rs` | GUI approval for proposed mutations. |
| `src/services/mcp.rs` | MCP JSON-RPC over standard input and output. |
| `src/services/configuration.rs` | Background persistence of settings. |
| `src/services/presentation.rs` | Background presentation work such as sorting and view preparation. |
| `src/services/preview.rs` | Bounded image and UTF-8 text preview generation. |
| `src/ui/` | egui screens, navigation, settings, selection, dialogs, and event handling. |
| `scripts/` | Environment reporting, SQLite invariant checks, localization template generation, and packaging of an existing binary. |
| `docs/` | Portable setup notes, implementation record, research notes, and machine-readable reports. |

## Runtime flow

### Startup

1. `runtime_paths()` uses the executable's parent as the portable root and creates `data/`, `logs/`, `models/`, `runtime/`, and `trash/` there.
2. `ConfigStorage` loads and validates `config.toml`. Invalid settings are retained under a timestamped name and defaults are written.
3. Logging starts under `logs/`; `SqliteStorage` opens `data/filemofm.db`, takes the exclusive `.dataset.lock`, enables WAL, and applies schema migrations.
4. A scheduled restore or `--restore-backup <absolute-path>` is handled before normal mode. Restore creates a safety backup first.
5. `--mcp` constructs read-only tool schemas and serves MCP over stdio. Normal startup constructs platform integrations and launches `FilemoApp`.

The database lock prevents two processes from using the same portable data directory at once. Do not change the data root to the current working directory; package behavior depends on the executable-relative root.

### GUI, navigation, and scanning

`FilemoApp` owns mutable UI state. Long-running work is sent to services over bounded channels. Services return `BackgroundMessage` events through `EventSink`; the UI applies a result only when its generation still matches the active navigation, scan, search, editor, or preview request.

Navigation validates or canonicalizes the requested location, resolves a file path to its parent folder, updates history, and requests a folder snapshot. `FileScannerService` reads the immediate children of one folder and upserts metadata in batches. It marks previously indexed children missing only if the directory scan had no entry errors. The optional filesystem watcher watches only the current folder and debounces changes.

Folder-size measurement is a separate recursive worker. It has entry and time limits, does not follow symlinks, and can return a partial result. Keep it separate from the shallow folder listing.

### Search

1. The UI debounces input and calls `parse_query()` to validate terms, filters, scope, roots, and result limits.
2. Keyword search runs against SQLite metadata and FTS. Search terms cover names, paths, and descriptions; file contents are not part of the index.
3. In selected-root scope, the optional `fff-search` adapter can discover matching names on disk. Discovered paths are inspected and rehydrated into SQLite before metadata filters are applied. If the adapter fails, the service reports that it is showing metadata results only.
4. Semantic search asks the vector worker to embed the query and search Qdrant. The semantic document contains the file name, path, tags, and plain-text description.
5. Hybrid search combines keyword and semantic rankings with reciprocal rank fusion. If vector search fails, it returns keyword results with a diagnostic.

Search modes are `Keyword`, `Semantic`, and `Hybrid`; scopes are `CurrentFolder`, `IndexedFiles`, and `SelectedRoots`. Keep their filtering behavior aligned across SQLite and vector retrieval.

### File operations

1. The UI builds an `OperationRequest` from selected paths, stable file IDs, native identities, metadata revisions, destination, collision policy, and required confirmations.
2. `OperationService` sends requests to one worker queue. The worker removes redundant descendant selections, checks stale revisions and source identities, and executes items serially.
3. `execute_item()` revalidates paths and destinations, protects the application root, journals mutation phases, performs the filesystem change, then updates SQLite metadata and folder-size caches.
4. On startup, `recover()` reconciles journal entries left by interrupted moves, case-only renames, replacements, or temporary copies.

Agent-proposed writes use the same approval path and filesystem operation queue as GUI operations. Keep mutation logic out of tool dispatch and UI rendering.

### SQLite and vector index

SQLite owns stable file IDs, native path keys, tags, descriptions, references, revisions, missing/tombstone state, folder-size cache, operation journal, and vector outbox. SQLite triggers update FTS and queue vector synchronization when indexed metadata changes. File operations update paths in the repository after filesystem commits.

Qdrant is a rebuildable projection. The vector worker reads pending outbox revisions, verifies each file revision before writing, and records projection hashes to skip unchanged documents. Rebuilds use a candidate collection and checkpoint; the active manifest changes only after catch-up and verification. Keep the previous active dataset available for rollback.

### Agent and MCP

Agent starts disabled, with no allowed roots, network access, MCP stdio, content reads, semantic search, or write permissions. Metadata reads still require Agent to be enabled and a non-empty allowed-root list.

The access policy checks the requested capability and canonical path against configured roots. Tool handlers validate arguments and IDs, recheck current metadata revisions, and enforce bounded content reads. Mutations require an interactive approval tied to the target and revision; policy is checked again before dispatching the write.

Remote chat endpoints require HTTPS, except explicitly enabled unauthenticated loopback endpoints. The HTTP client disables redirects and proxy use and has connect and request timeouts. Tool and file data are untrusted input to the model; never treat model output as authorization.

`--mcp` runs a line-delimited JSON-RPC session on stdio. The entry point supplies read-only schemas, and MCP permission must be enabled in settings. Do not add an HTTP listener or write log output to stdout in MCP mode.

## Invariants to preserve

- **SQLite is authoritative.** Rebuildable indexes and caches must not become the only copy of metadata.
- **Identity is stable.** A path reused by a different file must not inherit another entry's tags, description, or references. Use stable IDs and native identities where the flow requires them.
- **Writes are revision-aware.** Metadata edits and Agent mutations must not silently overwrite a newer revision.
- **A scan failure is not proof of deletion.** Mark entries missing only when the scan completes without per-entry errors.
- **Filesystem mutation is revalidated.** Paths, parent directories, source identity, revision, collision choice, and protected locations must be checked at the operation boundary, not only in the UI.
- **Interrupted writes are recoverable.** Journal phases must describe filesystem commit boundaries accurately; preserve recovery behavior when changing transfer steps.
- **Stale background results are ignored.** Retain generation checks and cancellation behavior when adding worker events.
- **Scope and permission filters apply before data leaves a boundary.** Recheck Agent policy around reads and after approval; do not rely on the chat prompt to enforce access.
- **Vector state follows metadata revisions.** Keep outbox acknowledgment, revision verification, deletion handling, and rebuild cutover consistent.
- **Portable data stays beside the executable.** Include any new persistent directory in path setup, validation, packaging, and documentation.

## Change guidelines

- Put cross-layer contracts in `shared/traits.rs` and shared DTOs in `shared/models.rs`; use existing interfaces before adding a new layer.
- Keep filesystem, SQLite, model, HTTP, and process work in infrastructure or service code, not in egui rendering callbacks.
- Use bounded channels and existing generation/cancellation patterns for background work. Keep UI state owned by `FilemoApp`.
- Treat paths, filenames, file contents, search text, config files, tool arguments, and remote responses as untrusted. Validate at the boundary where they enter the operation.
- Follow existing Rust and UI patterns. Do not add dependencies or broaden platform support without a concrete requirement.
- Route UI strings through the localization helpers in `shared/i18n.rs`; use English as the fallback. Keep logs and internal errors technical and free of secrets or file contents.
- Write concise English comments only for a non-obvious purpose, invariant, warning, lifecycle, or side effect. Do not narrate the next line of code.
- Keep README and docs factual. Do not claim support, safety, performance, or test coverage beyond what the source and verification establish.

## Build and verification commands

Run the smallest relevant checks for the change, then report what was and was not verified. Do not claim a test or platform build that was not run.

```sh
cargo fmt --all -- --check
cargo check
cargo test
cargo clippy --all-targets
```

Useful repository scripts:

```sh
python3 scripts/check-environment.py
python3 scripts/verify-storage.py
```

`check-environment.py` reports local toolchain and target availability. `verify-storage.py` exercises schema and selected SQL invariants with Python's SQLite module; it does not compile Rust or verify the bundled SQLite adapter. `scripts/package.py` packages an existing Linux or Windows x64 binary into a new directory; it does not build the application or download runtimes and models.

Important behavior tests live beside their modules, including query parsing in `src/shared/query.rs`, operation helpers in `src/services/operations.rs`, and repository behavior in `src/infrastructure/sqlite.rs`. Platform-sensitive filesystem behavior still requires verification on the target OS.
