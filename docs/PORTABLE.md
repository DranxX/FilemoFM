# Preparing a FilemoFM package

The source had not been compiled or smoke-tested when this guide was written. This guide describes the package contract and setup steps for verification.

Each target package contains `filemofm.exe` on Windows x64 or `filemofm` on Linux x64. The application root is the executable's directory, not the current working directory. FilemoFM creates `config.toml`, `data/filemofm.db`, `logs/`, `models/`, `runtime/`, and `trash/` there. The directory must be writable; startup reports failures instead of silently switching locations.

Windows release builds use the GUI subsystem, and internal child processes are started without a console window. Native Windows testing is still required. Linux needs an X11 or Wayland desktop session and a compatible graphics runtime. On Linux, rfd uses the XDG Desktop Portal; GTK is not a selected dependency.

Local environment snapshot: Rust/Cargo 1.98.1, declared dependency MSRVs up to 1.92, and egui/eframe/epaint 0.34.3. Dependency resolution succeeded for Linux x64 and Windows MSVC x64. The Windows Rust target is installed, but `cl`, `clang-cl`, and a Windows SDK setup are unavailable. `lld-link` alone does not prove that Windows builds are ready. The `xkbcommon`, GTK3, and D-Bus development packages were not found by pkg-config; runtime libxkbcommon, X11, Wayland, EGL, and GL are available. XDG Desktop Portal, Qdrant, and llama-server were not found on PATH.

For vector search, obtain native Qdrant and llama.cpp `llama-server` executables from trusted publishers for the target OS and architecture. Settings > Index & vector > Native runtimes asks for each executable and a version label. Registration copies the executable and DLL/SO files from its source folder, records checksums, and verifies them at startup. Source library symlinks are accepted only when their targets stay inside the source folder, and are copied as regular files. The operator supplies the version label; a local checksum does not prove publisher authenticity. Include the licenses for selected native packages before redistributing them.

Runtime layout after registration:

```text
runtime/
  qdrant.json
  qdrant/
    qdrant.exe or qdrant
  llama-server.json
  llama-server/
    llama-server.exe or llama-server
    related runtime libraries
```

The app starts each server as a child process bound to loopback, with a dynamic port and temporary credentials. Qdrant stores data in `data/vectors`. Clustering and CORS are disabled. llama-server uses last-token embedding pooling and a CPU baseline; the model is loaded only when work is requested. Do not expose internal ports to the LAN.

Model setup:

1. Open Settings > Index & vector and register trusted native runtimes.
2. Select Inspect repository, review the publisher, revision, license, size, and checksum, then select Download verified artifact.
3. Select Review model / dimension change, then Start rebuild to create the first dataset.
4. After the dataset becomes active, Synchronize processes the outbox, even when auto-sync is off. The model can be loaded or unloaded and unloads after the configured idle period.

The default logical model, Qwen3-Embedding-0.6B, points to the official Qwen3-Embedding-0.6B-GGUF F16 artifact. The model is about 1.20 GB and was not downloaded during this work. GGUF files outside the supported family and output size are rejected. The adapter requests the full 1024-dimensional output, then applies a Matryoshka prefix and L2 normalization for the selected dimension. Changing the model, dimension, runtime fingerprint, or encoding-related configuration requires a rebuild.

Canceling a rebuild saves its checkpoint. Restart it with the same artifact and fingerprint to resume. The app verifies the candidate before switching the active SQLite pointer. Rollback builds a new generation with the previous model and dimensions using current metadata. The previous dataset is retained; old generations are not cleaned up automatically.

For Agent, add an endpoint and model, choose an environment credential or enter a session credential, then enable the required network and root/capability permissions. Session credentials are not saved. Write permissions still require GUI approval for the displayed target and revision. MCP exposes read-only tools; run the executable with `--mcp` after enabling Agent, metadata roots, and MCP stdio. Stdout is reserved for JSON-RPC; MCP startup messages go to stderr and the log.

For LAN access, use shares mounted by the operating system. Linux uses local mounts; Windows uses available UNC/WSL paths. The operating system handles share credentials. Transfers use the same file-operation queue.

SQLite backups are created in Settings > Storage. Choose a new file name. Restore is scheduled for the next startup and creates a safety backup first. When the app is closed, the alternative is `filemofm --restore-backup <absolute-backup-path>`. Metadata backups do not include models, app trash, environment credentials, or vector snapshots. The vector projection is reconciled from metadata.

Root relocation is available in Settings > Storage. The old root must be unavailable. Review the mapping before confirming: the app maps old identities, tags, descriptions, and references to matching relative paths in the new root. It does not move files. The app rejects missing targets, type changes, revision changes, and conflicts with indexed identities. Update Agent root permissions separately.

To package a binary that has already been verified, run `python3 scripts/package.py --binary <binary> --target <target> --output <new-folder>`. Supported targets are `x86_64-unknown-linux-gnu` and `x86_64-pc-windows-msvc`. The script checks the binary format, copies the source lockfile and this guide, then records a checksum. It does not download or bundle runtimes or models.
