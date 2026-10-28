use crate::shared::constants::*;
use crate::shared::models::{BackgroundMessage, EventSink, FileItem, FilePreview};
use std::io::Read;
use std::path::Path;

pub fn request_preview(file: FileItem, generation: u64, sender: EventSink) {
    rayon::spawn(move || {
        let result = generate_preview(&file.path, &file.extension, file.native_id.as_deref());
        let file_id = file.id;
        let message = match result {
            Ok(preview) => BackgroundMessage::PreviewReady {
                generation,
                file_id,
                preview,
            },
            Err(error) => BackgroundMessage::PreviewFailed {
                generation,
                file_id,
                error,
            },
        };
        let _ = sender.send(message);
    });
}

fn generate_preview(
    path: &Path,
    extension: &str,
    identity: Option<&str>,
) -> Result<FilePreview, String> {
    let metadata = std::fs::symlink_metadata(path).map_err(|_| "File cannot be accessed")?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Ok(FilePreview::None);
    }
    if PREVIEWABLE_IMAGE_EXTENSIONS.contains(&extension) {
        if metadata.len() > MAX_PREVIEW_IMAGE_BYTES {
            return Ok(FilePreview::TooLarge);
        }
        let file = crate::shared::utils::open_verified_file(path, identity)?;
        let mut bytes = Vec::new();
        file.take(MAX_PREVIEW_IMAGE_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| "File could not be read")?;
        if bytes.len() as u64 > MAX_PREVIEW_IMAGE_BYTES {
            return Ok(FilePreview::TooLarge);
        }
        let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
            .with_guessed_format()
            .map_err(|_| "Unknown image format")?;
        let mut limits = image::Limits::default();
        limits.max_alloc = Some(MAX_PREVIEW_IMAGE_BYTES * 4);
        reader.limits(limits);
        let image = reader
            .decode()
            .map_err(|_| "Image could not be decoded")?
            .thumbnail(PREVIEW_MAX_DIMENSION, PREVIEW_MAX_DIMENSION)
            .to_rgba8();
        return Ok(FilePreview::Image {
            width: image.width(),
            height: image.height(),
            rgba_data: image.into_raw(),
        });
    }
    if !PREVIEWABLE_TEXT_EXTENSIONS.contains(&extension) {
        return Ok(FilePreview::None);
    }
    if metadata.len() > MAX_PREVIEW_TEXT_BYTES {
        return Ok(FilePreview::TooLarge);
    }
    let file = crate::shared::utils::open_verified_file(path, identity)?;
    let mut bytes = Vec::new();
    file.take(MAX_PREVIEW_TEXT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "File could not be read")?;
    if bytes.len() as u64 > MAX_PREVIEW_TEXT_BYTES {
        return Ok(FilePreview::TooLarge);
    }
    Ok(String::from_utf8(bytes)
        .map(FilePreview::Text)
        .unwrap_or(FilePreview::UnsupportedEncoding))
}
