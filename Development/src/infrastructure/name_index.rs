use crate::shared::models::{SearchQuery, SearchScope};
use crate::shared::traits::{NameIndex, PathValidator};
use fff_search::{
    FilePicker, FilePickerOptions, FuzzySearchOptions, PaginationArgs, QueryParser,
    SharedFilePicker, SharedFrecency,
};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

pub struct FffNameIndex {
    roots: Mutex<HashMap<PathBuf, SharedFilePicker>>,
    validator: Arc<dyn PathValidator>,
}
impl FffNameIndex {
    pub fn new(validator: Arc<dyn PathValidator>) -> Self {
        Self {
            validator,
            roots: Mutex::new(HashMap::new()),
        }
    }
}
impl NameIndex for FffNameIndex {
    fn search(&self, query: &SearchQuery) -> Result<Vec<PathBuf>, String> {
        if query.scope != SearchScope::SelectedRoots || query.text.is_empty() {
            return Ok(Vec::new());
        }
        let mut roots = self.roots.lock();
        let mut paths = Vec::new();
        for requested in &query.roots {
            let root = &crate::shared::utils::canonicalize(requested)
                .map_err(|_| "Index root is unavailable")?;
            if self.validator.is_excluded(root) || !root.is_dir() || root.parent().is_none() {
                return Err(
                    "Choose a folder as index root; a whole filesystem root is not supported"
                        .into(),
                );
            }
            if !roots.contains_key(root) {
                let picker = SharedFilePicker::default();
                FilePicker::new_with_shared_state(
                    picker.clone(),
                    SharedFrecency::default(),
                    FilePickerOptions {
                        base_path: root
                            .to_str()
                            .ok_or("Index root must be a UTF-8 path")?
                            .into(),
                        watch: true,
                        follow_symlinks: false,
                        enable_content_indexing: false,
                        enable_mmap_cache: false,
                        enable_home_dir_scanning: true,
                        ..Default::default()
                    },
                )
                .map_err(|_| "Name index could not start")?;
                roots.insert(root.clone(), picker);
            }
            let shared = &roots[root];
            if !shared.wait_for_scan(Duration::from_secs(30)) {
                return Err("Name index is still being prepared. Try again".into());
            }
            let guard = shared.read().map_err(|_| "Name index is unavailable")?;
            let picker = guard.as_ref().ok_or("Name index is not ready")?;
            let parser = QueryParser::default();
            let parsed = parser.parse(&query.text);
            let results = picker.fuzzy_search(
                &parsed,
                None,
                FuzzySearchOptions {
                    max_threads: 2,
                    current_file: None,
                    project_path: None,
                    combo_boost_score_multiplier: 0,
                    min_combo_count: 0,
                    pagination: PaginationArgs {
                        offset: 0,
                        limit: crate::shared::constants::MAX_RESULTS,
                    },
                },
            );
            for item in results.items {
                let path = item.absolute_path(picker, &picker.base_path);
                if !self.validator.is_excluded(&path) {
                    paths.push(path);
                }
            }
        }
        Ok(paths)
    }
    fn reset(&self) {
        for (_, picker) in self.roots.lock().drain() {
            picker.cancel();
        }
    }
}
impl Drop for FffNameIndex {
    fn drop(&mut self) {
        self.reset();
    }
}
