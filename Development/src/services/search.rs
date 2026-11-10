use crate::shared::constants::*;
use crate::shared::models::*;
use crate::shared::traits::{FileInspector, MetadataRepository, NameIndex, VectorSearch};
use std::collections::HashMap;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

pub struct SearchService {
    requests: crossbeam_channel::Sender<(u64, SearchQuery, SearchMode)>,
    generation: Arc<AtomicU64>,
}
impl SearchService {
    pub fn new(
        repository: Arc<dyn MetadataRepository>,
        names: Arc<dyn NameIndex>,
        inspector: Arc<dyn FileInspector>,
        vectors: Arc<dyn VectorSearch>,
        sender: EventSink,
    ) -> Self {
        let (requests, receiver) =
            crossbeam_channel::bounded::<(u64, SearchQuery, SearchMode)>(CHANNEL_CAPACITY);
        let generation = Arc::new(AtomicU64::new(0));
        let current = generation.clone();
        std::thread::spawn(move || {
            while let Ok((generation, query, mode)) = receiver.recv() {
                if current.load(Ordering::Acquire) != generation {
                    continue;
                }
                let results = (|| -> Result<Vec<SearchResult>, String> {
                    let mut keyword = Vec::new();
                    if mode != SearchMode::Semantic {
                        keyword = repository
                            .search_keyword(&query)
                            .map_err(|error| error.to_string())?;
                        if query.scope == SearchScope::SelectedRoots && !query.text.is_empty() {
                            let paths = names.search(&query).unwrap_or_else(|error| {
                                let _ = sender.try_send(BackgroundMessage::Diagnostic(Err(format!(
                                    "Name index unavailable; showing metadata results only: {error}"
                                ))));
                                Vec::new()
                            });
                            let mut files: Vec<_> = paths
                                .iter()
                                .filter_map(|path| inspector.inspect(path))
                                .collect();
                            repository
                                .upsert_files(&mut files)
                                .map_err(|error| error.to_string())?;
                            let ranks: HashMap<_, _> = paths
                                .into_iter()
                                .enumerate()
                                .map(|(rank, path)| (path, rank))
                                .collect();
                            let mut filtered = query.clone();
                            filtered.text.clear();
                            filtered.limit = MAX_RESULTS;
                            let mut candidates = Vec::new();
                            for batch in files.chunks(MAX_RESULTS) {
                                filtered.ids = batch.iter().map(|file| file.id.clone()).collect();
                                candidates.extend(
                                    repository
                                        .search_keyword(&filtered)
                                        .map_err(|error| error.to_string())?,
                                );
                            }
                            for mut hit in candidates {
                                if let Some(rank) = ranks.get(&hit.file.path) {
                                    hit.score = 1.0 / (RRF_OFFSET + *rank as f32 + 1.0);
                                    if !keyword.iter().any(|other| other.file.id == hit.file.id) {
                                        keyword.push(hit);
                                    }
                                }
                            }
                            keyword.sort_by(|a, b| b.score.total_cmp(&a.score));
                        }
                    }
                    if mode == SearchMode::Keyword {
                        keyword.truncate(query.limit);
                        return Ok(keyword);
                    }
                    let semantic = match vectors.search(query.clone()) {
                        Ok(results) => results,
                        Err(error) if mode == SearchMode::Hybrid => {
                            let _ = sender.try_send(BackgroundMessage::Diagnostic(Err(format!(
                                "Hybrid search is using keyword results only: {error}"
                            ))));
                            keyword.truncate(query.limit);
                            return Ok(keyword);
                        }
                        Err(error) => return Err(error),
                    };
                    if mode == SearchMode::Semantic {
                        return Ok(semantic);
                    }
                    let mut fused = HashMap::<String, SearchResult>::new();
                    for ranking in [keyword, semantic] {
                        for (rank, mut hit) in ranking.into_iter().enumerate() {
                            let score = 1.0 / (RRF_OFFSET + rank as f32 + 1.0);
                            if let Some(previous) = fused.get_mut(&hit.file.id) {
                                previous.score += score;
                            } else {
                                hit.score = score;
                                fused.insert(hit.file.id.clone(), hit);
                            }
                        }
                    }
                    let mut results: Vec<_> = fused.into_values().collect();
                    results.sort_by(|a, b| {
                        b.score
                            .total_cmp(&a.score)
                            .then_with(|| a.file.name.cmp(&b.file.name))
                            .then_with(|| a.file.id.cmp(&b.file.id))
                    });
                    results.truncate(query.limit);
                    Ok(results)
                })();
                if current.load(Ordering::Acquire) == generation {
                    let _ = sender.send(BackgroundMessage::SearchResults {
                        generation,
                        results,
                    });
                }
            }
        });
        Self {
            requests,
            generation,
        }
    }
    pub fn search(&self, query: SearchQuery, mode: SearchMode) -> Result<u64, String> {
        let generation = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        self.requests
            .try_send((generation, query, mode))
            .map_err(|_| "Search queue is full")?;
        Ok(generation)
    }
    pub fn cancel(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
    }
}
impl Drop for SearchService {
    fn drop(&mut self) {
        self.cancel();
    }
}
