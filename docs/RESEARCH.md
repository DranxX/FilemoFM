# FilemoFM research notes

The review on October 15, 2026 used official repositories, crate source resolved by Cargo.lock, official model metadata, and local images. The choices target Windows and Linux x64.

## fsearch and fff

| Candidate | Snapshot reviewed | Findings and decision |
| --- | --- | --- |
| [noahdunnagan/fsearch](https://github.com/noahdunnagan/fsearch) | `88e63fdf850cf7749deae8198b187accd21f8ba5` | The repository targets macOS; traversal and events use macOS APIs such as `getattrlistbulk` and FSEvents. MIT licensed. Not selected for Windows/Linux. |
| [dmtrKovalenko/fff v0.11.0](https://github.com/dmtrKovalenko/fff/releases/tag/v0.11.0) | `95fd777c2529fc7b4d7572dabff64cc07268f2c5` | Released as a Rust crate. `fff-search =0.11.0` is pinned in the manifest and lockfile. MIT licensed, copyright Dmitriy Kovalenko. Used as a name/path index, not as the authority for tags or descriptions. |

Research clones in `temp/fsearch` and `temp/fff` are not included in the source archive. The referenced API is [fff-search 0.11.0](https://docs.rs/fff-search/0.11.0/fff_search/). `SharedFilePicker`, options, scan waiting, the query parser, pagination, and absolute paths were checked against the downloaded crate.

The adapter enables a watcher only for a search root selected by the user. fff content indexing and mmap caching are disabled. Symlinks are not followed. Regular browsing does not index the entire disk. fff results are mapped back to SQLite identities before dork and scope filters are applied. This describes the integration choice; the upstream release does not establish this application's runtime performance or stability.

## Model and runtime

The default logical model remains [Qwen3-Embedding-0.6B](https://huggingface.co/Qwen/Qwen3-Embedding-0.6B). The runtime artifact comes from the official publisher of [Qwen3-Embedding-0.6B-GGUF](https://huggingface.co/Qwen/Qwen3-Embedding-0.6B-GGUF).

| Artifact field reviewed | Value |
| --- | --- |
| Revision | `370f27d7550e0def9b39c1f16d3fbaa13aa67728` |
| Filename | `Qwen3-Embedding-0.6B-f16.gguf` |
| Size | 1,197,629,632 bytes |
| SHA-256 | `421a27e58d165478cc7acb984a688c2aa41404968b0203e7cd743ece44c54340` |
| License metadata | Apache-2.0 |

These values came from metadata inspection, not a model download or inference run. When used, the downloader requests the immutable revision from the metadata, checks its size and SHA, then verifies the format. The GGUF model was not downloaded during this work.

The [llama-server documentation](https://github.com/ggml-org/llama.cpp/blob/master/tools/server/README.md) informed the use of `--embedding`, last-token pooling, `/v1/embeddings`, an API key through the environment, and CPU layers. The adapter requests the full 1024-dimensional output, then applies a Matryoshka prefix and L2 normalization for the selected dimension. The fingerprint includes the artifact, runtime manifest, dimension, context, and query instruction. The source smoke fixture was not run.

The native Qdrant integration follows the [query points API](https://api.qdrant.tech/api-reference/search/query-points) and [official configuration](https://github.com/qdrant/qdrant/blob/master/config/config.yaml). The sidecar uses loopback, per-process random credentials, disabled clustering/CORS/telemetry, and storage beside the executable. It does not use Docker or Qdrant Cloud. The application owns the collection and verifies a new generation before switching the SQLite pointer. A matched Qdrant and llama-server executable release has not been packaged or tested.

## APIs and protocols

| Area | Primary sources reviewed |
| --- | --- |
| GUI | [eframe 0.34.3](https://docs.rs/eframe/0.34.3/eframe/trait.App.html), [egui 0.34.3](https://docs.rs/egui/0.34.3/egui/), and local crate source for panels, themes, modals, and text cursors |
| Clipboard | [clipboard-rs 0.3.5](https://docs.rs/clipboard-rs/0.3.5/clipboard_rs/) and its Windows/Linux/Wayland backends |
| Metadata | [FTS5](https://www.sqlite.org/fts5.html), [Online Backup API](https://www.sqlite.org/backup.html), [WAL](https://www.sqlite.org/wal.html), [rusqlite 0.31](https://docs.rs/rusqlite/0.31.0/rusqlite/) |
| Markdown | [pulldown-cmark 0.13](https://docs.rs/pulldown-cmark/0.13.4/pulldown_cmark/) and offset events; the renderer does not execute remote HTML or images |
| Network | [reqwest 0.12](https://docs.rs/reqwest/0.12.28/reqwest/) and the blocking client source for timeouts, pinned DNS, and redirect policy |
| Identity | [file-id 0.2.3](https://docs.rs/file-id/0.2.3/file_id/) and the Windows high/low-resolution file ID implementation |
| MCP | [Lifecycle protocol 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/basic/lifecycle), initialize/initialized, and JSON-RPC stdio |
| Agent | [Ollama chat API](https://docs.ollama.com/api/chat) and the user-selected chat-compatible endpoint protocol |

Resolved crate versions and active features are recorded in the [environment report](environment-2026-10-15.json). Native C/C++ libraries required by transitive dependencies still need to be checked during compilation; metadata resolution does not prove linking succeeds.

## Visual references and design rationale

The design review followed the local brief: a Windows/Linux desktop file manager with dark slate, a blue accent, existing Phosphor icons, and ENERGY 1 / RHYTHM 2 / MOTION 1. Reference screenshots are not presented as implementation screenshots.

The 13 local images are `DrxManager_Toolbar.png`, `Layout_Windows.png`, `TopBar.png`, `LeftBar.png`, `Sort.png`, `Sort_More.png`, `Sort_GroupBy.png`, `View.png`, `More_IfSelectFile.png`, `More_IfNotSelectFile.png`, `RightClick_ifSelectFile.png`, `RightClick_ifNotSelectFile.png`, and `drxmanager_gui_mockup_.png`. The historical DESIGN.md lists 12 because an additional mockup was missing from the initial table; the actual inventory contains 13.

| Decision | Reason |
| --- | --- |
| Dark slate with one blue accent | Keeps the requested palette and distinguishes selection/focus from file metadata. |
| Light and system preferences | Gives users an environment choice while retaining the default identity. |
| File list in the center | Keeps operation targets visible before users change files. |
| Navigation separate from operation toolbar | History and location have a different role from selection mutations. |
| One tabbed inspector | Makes preview, metadata, and Agent available without three permanent panels. |
| Overlay in narrow layouts | Keeps panels available without taking most of the file canvas width. |
| Existing proportional font; monospace only for editor/path/code | Keeps names easy to scan while preserving readability for syntax and addresses. |
| Compact spacing with 4/8 gaps | Supports dense file lists and separates desktop control groups. |
| Existing Phosphor icons | Keeps the current icon language without creating custom assets. |
| Repeated file views, different Settings composition | Repetition supports directory scanning; settings need grouped fields and review. |
| Motion limited to hover/focus/task progress | Provides feedback without distracting during browsing. |

No logo, raster asset, font, or illustration was created. Reviewing the actual layout, composited colors, and keyboard interactions still requires the running application.
