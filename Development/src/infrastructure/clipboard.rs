use crate::shared::constants::{
    CHANNEL_CAPACITY, CLIPBOARD_INTENT_FORMAT, MAX_OPERATION_ITEMS, MAX_PATH_BYTES,
};
use crate::shared::models::{BackgroundMessage, ClipboardPayload, EventSink};
use clipboard_rs::{Clipboard, ClipboardContent, ClipboardContext};
use std::path::PathBuf;

enum ClipboardCommand {
    Read,
    Write(ClipboardPayload),
    Clear,
}
pub struct FileClipboard {
    commands: crossbeam_channel::Sender<ClipboardCommand>,
}
impl FileClipboard {
    pub fn new(sender: EventSink) -> Self {
        let (commands, receiver) = crossbeam_channel::bounded(CHANNEL_CAPACITY);
        std::thread::spawn(move || {
            let context = ClipboardContext::new();
            while let Ok(command) = receiver.recv() {
                let result = (|| -> Result<ClipboardPayload, String> {
                    let context = context
                        .as_ref()
                        .map_err(|_| "System clipboard is unavailable")?;
                    match command {
                        ClipboardCommand::Write(payload) => {
                            if payload.paths.len() > MAX_OPERATION_ITEMS {
                                return Err("Clipboard contains too many files".into());
                            }
                            let intent = serde_json::to_vec(&(
                                payload
                                    .paths
                                    .iter()
                                    .map(|path| crate::shared::utils::path_key(path))
                                    .collect::<Vec<_>>(),
                                payload.is_cut,
                                payload
                                    .identities
                                    .iter()
                                    .map(|(path, id)| {
                                        (crate::shared::utils::path_key(path), id.clone())
                                    })
                                    .collect::<Vec<_>>(),
                            ))
                            .map_err(|_| "Clipboard content is invalid")?;
                            if intent.len() as u64 > crate::shared::constants::MAX_HTTP_BYTES {
                                return Err("Clipboard payload is too large".into());
                            }
                            let mut contents = Vec::new();
                            #[cfg(windows)]
                            {
                                let files = payload
                                    .paths
                                    .iter()
                                    .map(|path| {
                                        path.to_str()
                                            .filter(|path| path.len() <= MAX_PATH_BYTES)
                                            .map(str::to_string)
                                            .ok_or(
                                                "Windows clipboard requires valid Unicode paths"
                                                    .to_string(),
                                            )
                                    })
                                    .collect::<Result<Vec<_>, _>>()?;
                                contents.push(ClipboardContent::Files(files));
                                contents.push(ClipboardContent::Other(
                                    "Preferred DropEffect".into(),
                                    if payload.is_cut { 2u32 } else { 1u32 }
                                        .to_le_bytes()
                                        .to_vec(),
                                ));
                            }
                            // On X11/Wayland the formats are written directly: clipboard-rs'
                            // `Files` always adds a GNOME entry marked "copy", which would turn
                            // every cut into a copy in file managers (and in this app).
                            #[cfg(not(windows))]
                            {
                                let uris = payload
                                    .paths
                                    .iter()
                                    .map(|path| {
                                        reqwest::Url::from_file_path(path)
                                            .map(String::from)
                                            .map_err(|_| "Clipboard path is invalid")
                                    })
                                    .collect::<Result<Vec<String>, _>>()?;
                                let marker = if payload.is_cut { "cut" } else { "copy" };
                                contents.push(ClipboardContent::Text(
                                    payload
                                        .paths
                                        .iter()
                                        .map(|path| path.display().to_string())
                                        .collect::<Vec<_>>()
                                        .join("\n"),
                                ));
                                contents.push(ClipboardContent::Other(
                                    "text/uri-list".into(),
                                    uris.join("\r\n").into_bytes(),
                                ));
                                contents.push(ClipboardContent::Other(
                                    "x-special/gnome-copied-files".into(),
                                    format!("{marker}\n{}", uris.join("\n")).into_bytes(),
                                ));
                                contents.push(ClipboardContent::Other(
                                    "application/x-kde-cutselection".into(),
                                    if payload.is_cut {
                                        b"1".to_vec()
                                    } else {
                                        b"0".to_vec()
                                    },
                                ));
                            }
                            contents.push(ClipboardContent::Other(
                                CLIPBOARD_INTENT_FORMAT.into(),
                                intent,
                            ));
                            context
                                .set(contents)
                                .map_err(|_| "Files could not be placed on the clipboard")?;
                            Ok(payload)
                        }
                        ClipboardCommand::Read => {
                            let files = context.get_files().unwrap_or_default();
                            if files.len() > MAX_OPERATION_ITEMS {
                                return Err("Clipboard contains too many files".into());
                            }
                            let paths = files
                                .into_iter()
                                .map(|file| {
                                    let path = reqwest::Url::parse(&file)
                                        .ok()
                                        .filter(|url| url.scheme() == "file")
                                        .and_then(|url| url.to_file_path().ok())
                                        .unwrap_or_else(|| PathBuf::from(file));
                                    if !path.is_absolute()
                                        || path.as_os_str().len() > MAX_PATH_BYTES
                                    {
                                        return Err("Clipboard path is invalid");
                                    }
                                    Ok(path)
                                })
                                .collect::<Result<Vec<_>, _>>()?;
                            let mut is_cut = false;
                            let mut recorded_identities = None;
                            if let Ok(bytes) = context.get_buffer(CLIPBOARD_INTENT_FORMAT)
                                && bytes.len() as u64 <= crate::shared::constants::MAX_HTTP_BYTES
                                && let Ok((keys, cut, ids)) =
                                    serde_json::from_slice::<(
                                        Vec<Vec<u8>>,
                                        bool,
                                        Vec<(Vec<u8>, String)>,
                                    )>(&bytes)
                            {
                                let recorded = keys
                                    .into_iter()
                                    .map(crate::shared::utils::path_from_key)
                                    .collect::<Vec<_>>();
                                if recorded == paths
                                    && ids.len() <= MAX_OPERATION_ITEMS
                                    && ids.iter().all(|(path, id)| {
                                        path.len() <= MAX_PATH_BYTES && id.len() <= 256
                                    })
                                {
                                    is_cut = cut;
                                    recorded_identities = Some(
                                        ids.into_iter()
                                            .map(|(path, id)| {
                                                (crate::shared::utils::path_from_key(path), id)
                                            })
                                            .collect::<Vec<_>>(),
                                    );
                                }
                            }
                            #[cfg(windows)]
                            if recorded_identities.is_none()
                                && let Ok(bytes) = context.get_buffer("Preferred DropEffect")
                            {
                                if bytes.len() == 4 {
                                    is_cut =
                                        u32::from_le_bytes(bytes.try_into().unwrap_or_default())
                                            & 2
                                            != 0;
                                }
                            }
                            #[cfg(not(windows))]
                            if recorded_identities.is_none() {
                                if let Ok(bytes) =
                                    context.get_buffer("x-special/gnome-copied-files")
                                {
                                    is_cut = bytes.starts_with(b"cut\n");
                                } else if let Ok(bytes) =
                                    context.get_buffer("application/x-kde-cutselection")
                                {
                                    is_cut = bytes.starts_with(b"1");
                                }
                            }
                            let identities = recorded_identities.unwrap_or_else(|| {
                                paths
                                    .iter()
                                    .filter_map(|path| {
                                        crate::shared::utils::file_identity(path)
                                            .map(|id| (path.clone(), id))
                                    })
                                    .collect()
                            });
                            Ok(ClipboardPayload {
                                paths,
                                is_cut,
                                identities,
                            })
                        }
                        ClipboardCommand::Clear => {
                            context
                                .clear()
                                .map_err(|_| "Clipboard could not be cleared")?;
                            Ok(ClipboardPayload {
                                paths: Vec::new(),
                                identities: Vec::new(),
                                is_cut: false,
                            })
                        }
                    }
                })();
                let _ = sender.send(BackgroundMessage::ClipboardReady(result));
            }
        });
        Self { commands }
    }
    pub fn refresh(&self) {
        let _ = self.commands.try_send(ClipboardCommand::Read);
    }
    pub fn write(&self, payload: ClipboardPayload) -> Result<(), String> {
        self.commands
            .try_send(ClipboardCommand::Write(payload))
            .map_err(|_| "Clipboard queue is full".into())
    }
    pub fn clear(&self) {
        let _ = self.commands.try_send(ClipboardCommand::Clear);
    }
}

impl crate::shared::traits::ClipboardRepository for FileClipboard {
    fn refresh(&self) {
        FileClipboard::refresh(self);
    }
    fn write(&self, payload: ClipboardPayload) -> Result<(), String> {
        FileClipboard::write(self, payload)
    }
    fn clear(&self) {
        FileClipboard::clear(self);
    }
}
