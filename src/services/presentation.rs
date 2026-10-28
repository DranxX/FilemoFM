use crate::shared::{
    models::*,
    utils::{group_label, natural_compare},
};
use crossbeam_channel::{Receiver, Sender, bounded};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

struct SortRequest {
    generation: u64,
    files: Vec<FileItem>,
    config: FileConfig,
    sizes: HashMap<PathBuf, FolderSize>,
    ranks: HashMap<String, f32>,
}
pub struct PresentationService {
    requests: Sender<SortRequest>,
    pending: Receiver<SortRequest>,
    generation: Arc<AtomicU64>,
}
impl PresentationService {
    pub fn new(sender: EventSink) -> Self {
        let (requests, pending) = bounded::<SortRequest>(1);
        let receiver = pending.clone();
        let generation = Arc::new(AtomicU64::new(0));
        let current = generation.clone();
        std::thread::spawn(move || {
            while let Ok(request) = receiver.recv() {
                if current.load(Ordering::Acquire) != request.generation {
                    continue;
                }
                let SortRequest {
                    generation,
                    mut files,
                    config,
                    sizes,
                    ranks,
                } = request;
                sort_files(&mut files, &config, &sizes, &ranks);
                if current.load(Ordering::Acquire) == generation {
                    let _ = sender.send(BackgroundMessage::CanvasReady { generation, files });
                }
            }
        });
        Self {
            requests,
            pending,
            generation,
        }
    }
    pub fn sort(
        &self,
        files: Vec<FileItem>,
        config: FileConfig,
        sizes: HashMap<PathBuf, FolderSize>,
        ranks: HashMap<String, f32>,
    ) -> u64 {
        let generation = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        let request = SortRequest {
            generation,
            files,
            config,
            sizes,
            ranks,
        };
        if let Err(crossbeam_channel::TrySendError::Full(request)) = self.requests.try_send(request)
        {
            let _ = self.pending.try_recv();
            let _ = self.requests.try_send(request);
        }
        generation
    }
}
fn sort_files(
    files: &mut Vec<FileItem>,
    config: &FileConfig,
    sizes: &HashMap<PathBuf, FolderSize>,
    ranks: &HashMap<String, f32>,
) {
    files.sort_by(|a, b| {
        let folders = if config.folders_first {
            b.is_directory.cmp(&a.is_directory)
        } else {
            std::cmp::Ordering::Equal
        };
        if folders != std::cmp::Ordering::Equal {
            return folders;
        }
        let compare = match config.sort {
            SortColumn::Name => natural_compare(&a.name, &b.name),
            SortColumn::Size => {
                let value = |file: &FileItem| {
                    if file.is_directory {
                        sizes.get(&file.path).map(|size| size.bytes)
                    } else {
                        Some(file.size)
                    }
                };
                match (value(a), value(b)) {
                    (Some(a), Some(b)) => a.cmp(&b),
                    (None, None) => std::cmp::Ordering::Equal,
                    (None, _) => return std::cmp::Ordering::Greater,
                    (_, None) => return std::cmp::Ordering::Less,
                }
            }
            SortColumn::Modified | SortColumn::Created => {
                let (left, right) = if config.sort == SortColumn::Modified {
                    (a.modified_at, b.modified_at)
                } else {
                    (a.created_at, b.created_at)
                };
                match (left == 0, right == 0) {
                    (true, false) => return std::cmp::Ordering::Greater,
                    (false, true) => return std::cmp::Ordering::Less,
                    _ => left.cmp(&right),
                }
            }
            SortColumn::Extension => a.extension.cmp(&b.extension),
            SortColumn::Tags => a
                .tags
                .iter()
                .map(|tag| &tag.name)
                .cmp(b.tags.iter().map(|tag| &tag.name)),
            SortColumn::Relevance => ranks
                .get(&b.id)
                .unwrap_or(&0.0)
                .total_cmp(ranks.get(&a.id).unwrap_or(&0.0)),
        };
        folders
            .then(
                if config.ascending || config.sort == SortColumn::Relevance {
                    compare
                } else {
                    compare.reverse()
                },
            )
            .then_with(|| natural_compare(&a.name, &b.name))
            .then(a.id.cmp(&b.id))
    });
    if config.group != GroupColumn::None {
        // Stable sort keeps the chosen order inside each group; keys are computed once per file.
        files.sort_by_cached_key(|file| group_rank(file, sizes, config.group));
    }
}

/// Orders groups meaningfully: sizes from small to large with Unknown last, dates newest first,
/// everything else alphabetically.
fn group_rank(
    file: &FileItem,
    sizes: &HashMap<PathBuf, FolderSize>,
    group: GroupColumn,
) -> (i64, String) {
    let label = group_label(file, sizes, group);
    let rank = match group {
        GroupColumn::Size => match label.as_str() {
            "Empty" => 0,
            "Under 1 MiB" => 1,
            "1–100 MiB" => 2,
            "100 MiB or larger" => 3,
            _ => 4,
        },
        GroupColumn::Modified | GroupColumn::Created => {
            let timestamp = if group == GroupColumn::Created {
                file.created_at
            } else {
                file.modified_at
            };
            if timestamp == 0 {
                i64::MAX
            } else {
                -timestamp / 86_400
            }
        }
        GroupColumn::Extension if file.is_directory => -1,
        _ => 0,
    };
    (rank, label.to_lowercase())
}
