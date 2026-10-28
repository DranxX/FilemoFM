use crate::shared::constants::*;
use crate::shared::errors::StorageError;
use crate::shared::models::*;
use crate::shared::traits::*;
use crate::shared::utils::{path_from_key, path_key, reference_nodes};
use chrono::Utc;
use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, Row, params, params_from_iter, types::Value};
use std::collections::{HashMap, HashSet};
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

const FILE_COLUMNS: &str = "f.id, f.path_key, f.name, f.extension, f.size, f.is_directory, f.created_at, f.modified_at, f.updated_at, f.description, f.revision, f.missing, f.native_id, f.hidden";
const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS schema_migrations(version INTEGER PRIMARY KEY, description TEXT NOT NULL, applied_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS files(id TEXT PRIMARY KEY, path TEXT NOT NULL UNIQUE, name TEXT NOT NULL, extension TEXT NOT NULL DEFAULT '', size INTEGER NOT NULL, is_directory INTEGER NOT NULL, created_at INTEGER NOT NULL, modified_at INTEGER NOT NULL, updated_at INTEGER NOT NULL, description TEXT NOT NULL DEFAULT '');
CREATE TABLE IF NOT EXISTS tags(id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT NOT NULL UNIQUE, color_h REAL NOT NULL, color_s REAL NOT NULL, color_l REAL NOT NULL);
CREATE TABLE IF NOT EXISTS file_tags(file_id TEXT NOT NULL REFERENCES files(id) ON DELETE CASCADE, tag_id INTEGER NOT NULL REFERENCES tags(id) ON DELETE CASCADE, assigned_at INTEGER NOT NULL, PRIMARY KEY(file_id, tag_id));
CREATE TABLE IF NOT EXISTS history(id INTEGER PRIMARY KEY AUTOINCREMENT, path TEXT NOT NULL UNIQUE, visit_count INTEGER NOT NULL DEFAULT 1, last_visited_at INTEGER NOT NULL);
";
const MIGRATION: &str = "
ALTER TABLE files ADD COLUMN path_key BLOB;
ALTER TABLE files ADD COLUMN parent_key BLOB;
ALTER TABLE files ADD COLUMN revision INTEGER NOT NULL DEFAULT 1;
ALTER TABLE files ADD COLUMN missing INTEGER NOT NULL DEFAULT 0;
CREATE UNIQUE INDEX files_native_path ON files(path_key);
CREATE INDEX files_parent ON files(parent_key);
CREATE TABLE vector_outbox(id TEXT PRIMARY KEY, revision INTEGER NOT NULL, deleted INTEGER NOT NULL);
CREATE TABLE file_references(source_id TEXT NOT NULL REFERENCES files(id), target_id TEXT NOT NULL REFERENCES files(id), PRIMARY KEY(source_id, target_id));
CREATE TABLE folder_sizes(path_key BLOB PRIMARY KEY, bytes INTEGER NOT NULL, state TEXT NOT NULL, measured_at INTEGER NOT NULL);
CREATE TABLE operation_journal(id TEXT NOT NULL, source BLOB NOT NULL, target BLOB NOT NULL, phase TEXT NOT NULL, updated_at INTEGER NOT NULL, PRIMARY KEY(id, source));
CREATE TABLE app_state(key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE VIRTUAL TABLE file_fts USING fts5(name, path, description, content='files', content_rowid='rowid', tokenize='unicode61');
CREATE TRIGGER files_insert AFTER INSERT ON files BEGIN
 INSERT INTO file_fts(rowid,name,path,description) VALUES(new.rowid,new.name,new.path,new.description);
 INSERT INTO vector_outbox(id,revision,deleted) VALUES(new.id,new.revision,new.missing) ON CONFLICT(id) DO UPDATE SET revision=new.revision,deleted=new.missing;
END;
CREATE TRIGGER files_fts_update AFTER UPDATE OF name,path,description ON files BEGIN
 INSERT INTO file_fts(file_fts,rowid,name,path,description) VALUES('delete',old.rowid,old.name,old.path,old.description);
 INSERT INTO file_fts(rowid,name,path,description) VALUES(new.rowid,new.name,new.path,new.description);
END;
CREATE TRIGGER files_revision AFTER UPDATE OF name,path,description,size,modified_at,missing ON files
WHEN old.name IS NOT new.name OR old.path IS NOT new.path OR old.description IS NOT new.description OR old.size IS NOT new.size OR old.modified_at IS NOT new.modified_at OR old.missing IS NOT new.missing BEGIN
 UPDATE files SET revision=old.revision+1,updated_at=unixepoch() WHERE id=new.id;
END;
CREATE TRIGGER files_outbox AFTER UPDATE OF revision ON files BEGIN
 INSERT INTO vector_outbox(id,revision,deleted) VALUES(new.id,new.revision,new.missing) ON CONFLICT(id) DO UPDATE SET revision=new.revision,deleted=new.missing;
END;
CREATE TRIGGER tag_added AFTER INSERT ON file_tags BEGIN UPDATE files SET revision=revision+1,updated_at=unixepoch() WHERE id=new.file_id; END;
CREATE TRIGGER tag_removed AFTER DELETE ON file_tags BEGIN UPDATE files SET revision=revision+1,updated_at=unixepoch() WHERE id=old.file_id; END;
INSERT INTO file_fts(file_fts) VALUES('rebuild');
INSERT INTO vector_outbox SELECT id,revision,missing FROM files;
INSERT OR REPLACE INTO schema_migrations VALUES(3,'FilemoFM native identity, FTS, references, outbox and recovery',unixepoch());
";

pub struct SqliteStorage {
    conn: Mutex<Connection>,
    database: PathBuf,
    _owner: File,
}

impl SqliteStorage {
    pub fn new(paths: &RuntimePaths) -> Result<Self, StorageError> {
        let owner = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(paths.data.join(".dataset.lock"))
            .map_err(|_| StorageError::QueryFailed)?;
        fs2::FileExt::try_lock_exclusive(&owner).map_err(|_| StorageError::DatabaseLocked)?;
        let mut conn = Connection::open(&paths.database)?;
        conn.execute_batch(
            // NORMAL is the recommended WAL setting: still crash-consistent, but it avoids an
            // fsync per transaction, which made first visits to large folders slow.
            "PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA busy_timeout=5000;",
        )?;
        conn.execute_batch(SCHEMA)?;
        let has_description: bool = conn
            .prepare("PRAGMA table_info(files)")?
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<Result<Vec<_>, _>>()?
            .iter()
            .any(|name| name == "description");
        if !has_description {
            conn.execute_batch(
                "ALTER TABLE files ADD COLUMN description TEXT NOT NULL DEFAULT '';",
            )?;
        }
        let version: i64 = conn.query_row(
            "SELECT coalesce(max(version),0) FROM schema_migrations",
            [],
            |row| row.get(0),
        )?;
        if version < 3 {
            let tx = conn.transaction()?;
            tx.execute_batch(MIGRATION)?;
            let rows = {
                let mut stmt = tx.prepare("SELECT id,path FROM files")?;
                stmt.query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?
                .collect::<Result<Vec<_>, _>>()?
            };
            for (id, path) in rows {
                let path = PathBuf::from(path);
                tx.execute(
                    "UPDATE files SET path_key=?1,parent_key=?2 WHERE id=?3",
                    params![path_key(&path), path.parent().map(path_key), id],
                )?;
            }
            tx.commit()?;
        }
        if version < 4 {
            let tx = conn.transaction()?;
            tx.execute_batch("ALTER TABLE files ADD COLUMN native_id TEXT; ALTER TABLE files ADD COLUMN hidden INTEGER NOT NULL DEFAULT 0; DROP INDEX files_native_path; CREATE UNIQUE INDEX files_native_path ON files(path_key) WHERE missing=0; UPDATE files SET path='missing:'||id WHERE missing=1; INSERT INTO schema_migrations VALUES(4,'Native snapshots and immutable tombstones',unixepoch());")?;
            tx.commit()?;
        }
        Ok(Self {
            conn: Mutex::new(conn),
            database: paths.database.clone(),
            _owner: owner,
        })
    }

    fn read_file(row: &Row<'_>) -> rusqlite::Result<FileItem> {
        Ok(FileItem {
            id: row.get(0)?,
            path: path_from_key(row.get(1)?),
            name: row.get(2)?,
            extension: row.get(3)?,
            size: row.get::<_, i64>(4)?.max(0) as u64,
            is_directory: row.get(5)?,
            created_at: row.get(6)?,
            modified_at: row.get(7)?,
            updated_at: row.get(8)?,
            description: row.get(9)?,
            revision: row.get(10)?,
            is_missing: row.get(11)?,
            native_id: row.get(12)?,
            is_hidden: row.get(13)?,
            tags: Vec::new(),
        })
    }

    fn read_files(
        conn: &Connection,
        sql: &str,
        values: &[Value],
    ) -> Result<Vec<FileItem>, StorageError> {
        let mut files = conn
            .prepare(sql)?
            .query_map(params_from_iter(values), Self::read_file)?
            .collect::<Result<Vec<_>, _>>()?;
        Self::attach_tags(conn, &mut files)?;
        Ok(files)
    }

    /// Loads tags for many files with one query per chunk instead of one per file.
    fn attach_tags(conn: &Connection, files: &mut [FileItem]) -> Result<(), StorageError> {
        const CHUNK: usize = 500;
        let mut tags: HashMap<String, Vec<Tag>> = HashMap::new();
        for chunk in files.chunks(CHUNK) {
            let placeholders = (1..=chunk.len())
                .map(|index| format!("?{index}"))
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT ft.file_id,t.id,t.name,t.color_h,t.color_s,t.color_l FROM file_tags ft JOIN tags t ON t.id=ft.tag_id WHERE ft.file_id IN({placeholders}) ORDER BY t.name"
            );
            let mut stmt = conn.prepare(&sql)?;
            let rows =
                stmt.query_map(params_from_iter(chunk.iter().map(|file| &file.id)), |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        Tag {
                            id: row.get(1)?,
                            name: row.get(2)?,
                            color_h: row.get(3)?,
                            color_s: row.get(4)?,
                            color_l: row.get(5)?,
                        },
                    ))
                })?;
            for row in rows {
                let (id, tag) = row?;
                tags.entry(id).or_default().push(tag);
            }
        }
        for file in files {
            file.tags = tags.remove(&file.id).unwrap_or_default();
        }
        Ok(())
    }

    /// Live records at `root` or below it, as `(id, path)`. Uses the path-key prefix so the
    /// whole table is never scanned; the separator suffix keeps `/a/b` from matching `/a/bc`.
    fn records_under(
        conn: &Connection,
        root: &Path,
    ) -> Result<Vec<(String, PathBuf)>, StorageError> {
        let key = path_key(root);
        let mut prefix = key.clone();
        let separator = path_key(Path::new(std::path::MAIN_SEPARATOR_STR));
        if !prefix.ends_with(&separator) {
            prefix.extend(separator);
        }
        let mut stmt = conn.prepare(
            "SELECT id,path_key FROM files WHERE missing=0 AND (path_key=?1 OR substr(path_key,1,?2)=?3)",
        )?;
        let rows = stmt
            .query_map(params![key, prefix.len() as i64, prefix], |row| {
                Ok((row.get::<_, String>(0)?, path_from_key(row.get(1)?)))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    fn tombstone(tx: &rusqlite::Transaction<'_>, id: &str) -> Result<(), StorageError> {
        tx.execute(
            "UPDATE files SET missing=1,path='missing:'||id WHERE id=?1",
            [id],
        )?;
        tx.execute("UPDATE files SET revision=revision+1 WHERE id IN(SELECT source_id FROM file_references WHERE target_id=?1)", [id])?;
        Ok(())
    }

    fn resolve(conn: &Connection, source: &str) -> Result<String, StorageError> {
        let mut result = source.to_string();
        for (range, id, label) in reference_nodes(source).into_iter().rev() {
            let target: Option<(String, String, bool)> = conn
                .query_row(
                    "SELECT name,path,missing FROM files WHERE id=?1",
                    [&id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()?;
            let text = match target {
                Some((name, path, false)) => format!("@{} ({path})", label.unwrap_or(name)),
                Some((name, _, true)) => {
                    format!("@{} [target unavailable]", label.unwrap_or(name))
                }
                _ => "@[reference unavailable]".into(),
            };
            result.replace_range(range, &text);
        }
        Ok(result)
    }
}

impl FileRepository for SqliteStorage {
    fn upsert_files(&self, files: &mut [FileItem]) -> Result<(), StorageError> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        for file in files.iter_mut() {
            if file.size > i64::MAX as u64 || file.path.as_os_str().is_empty() {
                return Err(StorageError::ConstraintViolation);
            }
            let key = path_key(&file.path);
            let stored_path = crate::shared::utils::config_path(&file.path);
            // The same path keeps its ID even when the native identity changes: editors that save
            // by writing a temp file and renaming it, and remounted drives, change inode/file IDs on
            // every save. Only a file replaced by a folder (or vice versa) becomes a new record.
            let previous: Option<(String, bool)> = tx
                .query_row(
                    "SELECT id,is_directory FROM files WHERE path_key=?1 AND missing=0",
                    [&key],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            if let Some((id, is_directory)) = previous
                && is_directory != file.is_directory
            {
                Self::tombstone(&tx, &id)?;
                file.id = uuid::Uuid::new_v4().to_string();
            }
            tx.execute("INSERT INTO files(id,path,path_key,parent_key,name,extension,size,is_directory,created_at,modified_at,updated_at,description,native_id,hidden) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,'',?12,?13) ON CONFLICT(path_key) WHERE missing=0 DO UPDATE SET name=excluded.name,extension=excluded.extension,size=excluded.size,is_directory=excluded.is_directory,modified_at=excluded.modified_at,native_id=excluded.native_id,hidden=excluded.hidden", params![file.id,stored_path,key,file.path.parent().map(path_key),file.name,file.extension,file.size as i64,file.is_directory,file.created_at,file.modified_at,file.updated_at,file.native_id,file.is_hidden])?;
            let sql =
                format!("SELECT {FILE_COLUMNS} FROM files f WHERE f.path_key=?1 AND f.missing=0");
            *file = tx.query_row(&sql, [key], Self::read_file)?;
        }
        Self::attach_tags(&tx, files)?;
        tx.commit()?;
        Ok(())
    }

    fn get_files_by_directory(&self, directory: &Path) -> Result<Vec<FileItem>, StorageError> {
        Self::read_files(
            &self.conn.lock(),
            &format!("SELECT {FILE_COLUMNS} FROM files f WHERE f.parent_key=?1 AND f.missing=0"),
            &[Value::Blob(path_key(directory))],
        )
    }

    fn get_file(&self, id: &str) -> Result<Option<FileItem>, StorageError> {
        Ok(Self::read_files(
            &self.conn.lock(),
            &format!("SELECT {FILE_COLUMNS} FROM files f WHERE f.id=?1"),
            &[Value::Text(id.into())],
        )?
        .into_iter()
        .next())
    }

    fn get_file_by_path(&self, path: &Path) -> Result<Option<FileItem>, StorageError> {
        Ok(Self::read_files(
            &self.conn.lock(),
            &format!("SELECT {FILE_COLUMNS} FROM files f WHERE f.path_key=?1 AND f.missing=0"),
            &[Value::Blob(path_key(path))],
        )?
        .into_iter()
        .next())
    }

    fn update_path(&self, old: &Path, new: &Path) -> Result<(), StorageError> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        let records = Self::records_under(&tx, old)?;
        let moving: HashSet<&str> = records.iter().map(|(id, _)| id.as_str()).collect();
        let mut affected = Vec::new();
        for (id, path) in &records {
            let Ok(relative) = path.strip_prefix(old) else {
                continue;
            };
            let target = if relative.as_os_str().is_empty() {
                new.to_path_buf()
            } else {
                new.join(relative)
            };
            // A stale live record already at the target (deleted outside the app) would block the
            // unique path key; it no longer describes a file there, so retire it.
            let stale: Option<String> = tx
                .query_row(
                    "SELECT id FROM files WHERE path_key=?1 AND missing=0",
                    [path_key(&target)],
                    |row| row.get(0),
                )
                .optional()?;
            if let Some(stale) = stale.filter(|stale| !moving.contains(stale.as_str())) {
                Self::tombstone(&tx, &stale)?;
            }
            let name = target
                .file_name()
                .unwrap_or(target.as_os_str())
                .to_string_lossy()
                .into_owned();
            let extension = target
                .extension()
                .map(|value| value.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            // Descendants keep their native identity on a rename; only the moved root is re-read.
            let native = if target == new {
                crate::shared::utils::file_identity(&target)
            } else {
                None
            };
            tx.execute("UPDATE files SET path=?1,path_key=?2,parent_key=?3,name=?4,extension=?5,missing=0,native_id=coalesce(?7,native_id) WHERE id=?6", params![crate::shared::utils::config_path(&target),path_key(&target),target.parent().map(path_key),name,extension,id,native])?;
            affected.push(id.clone());
        }
        for id in affected {
            tx.execute("UPDATE files SET revision=revision+1,updated_at=unixepoch() WHERE id IN(SELECT source_id FROM file_references WHERE target_id=?1)", [&id])?;
        }
        tx.commit()?;
        Ok(())
    }

    fn files_under(&self, root: &Path) -> Result<Vec<(String, PathBuf)>, StorageError> {
        Self::records_under(&self.conn.lock(), root)
    }

    fn relocate(
        &self,
        old: &Path,
        new: &Path,
        revisions: &[(String, i64)],
    ) -> Result<(), StorageError> {
        if !old.is_absolute()
            || old.exists()
            || !new.is_absolute()
            || !new.is_dir()
            || revisions.is_empty()
            || revisions.len() > MAX_RELOCATION_FILES
        {
            return Err(StorageError::ConstraintViolation);
        }
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        let mut identities = HashSet::new();
        for (id, revision) in revisions {
            if uuid::Uuid::parse_str(id).is_err() || !identities.insert(id) {
                return Err(StorageError::ConstraintViolation);
            }
            let file = tx.query_row(
                &format!("SELECT {FILE_COLUMNS} FROM files f WHERE f.id=?1"),
                [id],
                Self::read_file,
            )?;
            if file.revision != *revision {
                return Err(StorageError::RevisionConflict);
            }
            let relative = file
                .path
                .strip_prefix(old)
                .map_err(|_| StorageError::ConstraintViolation)?;
            let target = new.join(relative);
            let canonical = crate::shared::utils::canonicalize(&target)
                .map_err(|_| StorageError::ConstraintViolation)?;
            if !canonical.starts_with(new) || canonical != target {
                return Err(StorageError::ConstraintViolation);
            }
            let metadata = std::fs::symlink_metadata(&target)
                .map_err(|_| StorageError::ConstraintViolation)?;
            if metadata.file_type().is_symlink() || metadata.is_dir() != file.is_directory {
                return Err(StorageError::ConstraintViolation);
            }
            let native = crate::shared::utils::file_identity(&target)
                .ok_or(StorageError::ConstraintViolation)?;
            let size = metadata.len().min(i64::MAX as u64) as i64;
            let name = target
                .file_name()
                .unwrap_or(target.as_os_str())
                .to_string_lossy();
            tx.execute("UPDATE files SET path=?1,path_key=?2,parent_key=?3,native_id=?4,missing=0,size=?5,name=?6 WHERE id=?7", params![crate::shared::utils::config_path(&target),path_key(&target),target.parent().map(path_key),native,size,name,id])?;
            tx.execute("UPDATE files SET revision=revision+1 WHERE id IN(SELECT source_id FROM file_references WHERE target_id=?1)",[id])?;
        }
        tx.commit()?;
        Ok(())
    }
    fn mark_missing(&self, path: &Path) -> Result<(), StorageError> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        for (id, _) in Self::records_under(&tx, path)? {
            Self::tombstone(&tx, &id)?;
        }
        tx.commit()?;
        Ok(())
    }

    fn restore_files(&self, files: &[(String, PathBuf)]) -> Result<(), StorageError> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        for (id, path) in files {
            if !path.is_absolute() || uuid::Uuid::parse_str(id).is_err() {
                return Err(StorageError::ConstraintViolation);
            }
            let native = crate::shared::utils::file_identity(path)
                .ok_or(StorageError::ConstraintViolation)?;
            let changed = tx.execute("UPDATE files SET missing=0,path=?1,path_key=?2,parent_key=?3,native_id=?4 WHERE id=?5 AND missing=1 AND path_key=?2",params![crate::shared::utils::config_path(path),path_key(path),path.parent().map(path_key),native,id])?;
            if changed != 1 {
                return Err(StorageError::ConstraintViolation);
            }
            tx.execute("UPDATE files SET revision=revision+1 WHERE id IN(SELECT source_id FROM file_references WHERE target_id=?1)",[id])?;
        }
        tx.commit()?;
        Ok(())
    }
    fn search_keyword(&self, query: &SearchQuery) -> Result<Vec<SearchResult>, StorageError> {
        if query.limit == 0 || query.limit > MAX_RESULTS || query.text.len() > MAX_QUERY_BYTES {
            return Err(StorageError::ConstraintViolation);
        }
        let conn = self.conn.lock();
        let mut clauses = vec!["f.missing=0".to_string()];
        let mut values = Vec::<Value>::new();
        let mut bind = |value: Value| {
            values.push(value);
            format!("?{}", values.len())
        };
        if query.scope == SearchScope::CurrentFolder {
            clauses.push(format!(
                "f.parent_key={}",
                bind(Value::Blob(path_key(&query.directory)))
            ));
        }
        if query.scope == SearchScope::SelectedRoots {
            if query.roots.is_empty() || query.roots.len() > 32 {
                return Err(StorageError::ConstraintViolation);
            }
            let mut roots = Vec::new();
            for root in &query.roots {
                let key = path_key(root);
                let mut prefix = key.clone();
                let separator = path_key(Path::new(std::path::MAIN_SEPARATOR_STR));
                if !prefix.ends_with(&separator) {
                    prefix.extend(separator);
                }
                roots.push(format!(
                    "(f.path_key={} OR substr(f.path_key,1,{})={})",
                    bind(Value::Blob(key)),
                    bind(Value::Integer(prefix.len() as i64)),
                    bind(Value::Blob(prefix))
                ));
            }
            clauses.push(format!("({})", roots.join(" OR ")));
        }
        if !query.ids.is_empty() {
            if query.ids.len() > MAX_RESULTS
                || query
                    .ids
                    .iter()
                    .any(|id| uuid::Uuid::parse_str(id).is_err())
            {
                return Err(StorageError::ConstraintViolation);
            }
            let ids = query
                .ids
                .iter()
                .map(|id| bind(Value::Text(id.clone())))
                .collect::<Vec<_>>()
                .join(",");
            clauses.push(format!("f.id IN({ids})"));
        }
        let mut fts = String::new();
        for term in query.text.split_whitespace() {
            let literal = term
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_");
            let parameter = bind(Value::Text(format!("%{literal}%")));
            let token = format!("\"{}\"", term.replace('"', "\"\""));
            let token_parameter = bind(Value::Text(token));
            clauses.push(format!("(f.name LIKE {parameter} ESCAPE '\\' OR f.path LIKE {parameter} ESCAPE '\\' OR f.description LIKE {parameter} ESCAPE '\\' OR f.rowid IN(SELECT rowid FROM file_fts WHERE file_fts MATCH {token_parameter}))"));
            if !fts.is_empty() {
                fts.push_str(" AND ");
            }
            fts.push_str(&format!("\"{}\"", term.replace('"', "\"\"")));
        }
        for filter in &query.filters {
            match filter {
                SearchFilter::Title(value) => {
                    let literal = value.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
                    clauses.push(format!("f.name LIKE {} ESCAPE '\\'", bind(Value::Text(format!("%{literal}%")))));
                }
                SearchFilter::Extension(value) => clauses.push(format!("lower(f.extension)={}", bind(Value::Text(value.to_lowercase())))),
                SearchFilter::Category(value) => {
                    let extensions: &[&str] = match value.as_str() {
                        "document" => DOCUMENT_EXTENSIONS,
                        "image" => PREVIEWABLE_IMAGE_EXTENSIONS,
                        "video" => &["mp4", "mkv", "avi", "mov", "webm"],
                        "audio" => &["mp3", "wav", "flac", "ogg", "m4a"],
                        "archive" => &["zip", "7z", "rar", "tar", "gz"],
                        "folder" => { clauses.push("f.is_directory=1".into()); continue },
                        _ => return Err(StorageError::ConstraintViolation),
                    };
                    let parameters = extensions.iter().map(|extension| bind(Value::Text((*extension).into()))).collect::<Vec<_>>().join(",");
                    clauses.push(format!("f.extension IN({parameters})"));
                }
                SearchFilter::Tag(value) => clauses.push(format!("EXISTS(SELECT 1 FROM file_tags ft JOIN tags t ON t.id=ft.tag_id WHERE ft.file_id=f.id AND t.name={} COLLATE NOCASE)", bind(Value::Text(value.clone())))),
                SearchFilter::Date(start, end) => clauses.push(format!("(f.modified_at>={} AND f.modified_at<{})", bind(Value::Integer(*start)), bind(Value::Integer(*end)))),
            }
        }
        // A bare integer in ORDER BY is a column index, so "ORDER BY 0" is an SQL error; a
        // filter-only query (`tag:work`, `type:xlsx`) simply orders by name.
        let ranking = if fts.is_empty() {
            "f.is_directory DESC".to_string()
        } else {
            format!(
                "coalesce((SELECT bm25(file_fts) FROM file_fts WHERE rowid=f.rowid AND file_fts MATCH {}),0)",
                bind(Value::Text(fts))
            )
        };
        let sql = format!(
            "SELECT {FILE_COLUMNS} FROM files f WHERE {} ORDER BY {ranking},f.name COLLATE NOCASE,f.id LIMIT {} OFFSET {}",
            clauses.join(" AND "),
            bind(Value::Integer(query.limit as i64)),
            bind(Value::Integer(query.offset.min(i64::MAX as usize) as i64))
        );
        let files = Self::read_files(&conn, &sql, &values)?;
        Ok(files
            .into_iter()
            .enumerate()
            .map(|(rank, file)| SearchResult {
                file,
                score: 1.0 / (RRF_OFFSET + rank as f32 + 1.0),
            })
            .collect())
    }

    fn update_description(
        &self,
        id: &str,
        revision: i64,
        description: &str,
    ) -> Result<(), StorageError> {
        if description.len() > MAX_DESCRIPTION_BYTES || description.contains('\0') {
            return Err(StorageError::ConstraintViolation);
        }
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        if tx.execute(
            "UPDATE files SET description=?1 WHERE id=?2 AND revision=?3",
            params![description, id, revision],
        )? != 1
        {
            return Err(StorageError::RevisionConflict);
        }
        tx.execute("DELETE FROM file_references WHERE source_id=?1", [id])?;
        for (_, target, _) in reference_nodes(description) {
            tx.execute("INSERT OR IGNORE INTO file_references(source_id,target_id) SELECT ?1,id FROM files WHERE id=?2", params![id,target])?;
        }
        tx.commit()?;
        Ok(())
    }

    fn resolve_description(&self, description: &str) -> Result<String, StorageError> {
        Self::resolve(&self.conn.lock(), description)
    }
}

impl TagRepository for SqliteStorage {
    fn set_tags(&self, id: &str, revision: i64, tags: &[i64]) -> Result<(), StorageError> {
        if uuid::Uuid::parse_str(id).is_err()
            || tags.len() > MAX_FILE_TAGS
            || tags.iter().any(|id| *id <= 0)
        {
            return Err(StorageError::ConstraintViolation);
        }
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        let current: Option<i64> = tx
            .query_row(
                "SELECT revision FROM files WHERE id=?1 AND missing=0",
                [id],
                |row| row.get(0),
            )
            .optional()?;
        if current != Some(revision) {
            return Err(StorageError::RevisionConflict);
        }
        tx.execute("DELETE FROM file_tags WHERE file_id=?1", [id])?;
        for tag in tags {
            tx.execute(
                "INSERT OR IGNORE INTO file_tags VALUES(?1,?2,unixepoch())",
                params![id, tag],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
    fn create_tag(&self, name: &str, h: f32, s: f32, l: f32) -> Result<Tag, StorageError> {
        if name.trim().is_empty()
            || name.len() > MAX_TAG_NAME_LENGTH
            || name.chars().any(char::is_control)
            || !h.is_finite()
            || !(0.0..=360.0).contains(&h)
            || !s.is_finite()
            || !(0.0..=1.0).contains(&s)
            || !l.is_finite()
            || !(0.0..=1.0).contains(&l)
        {
            return Err(StorageError::ConstraintViolation);
        }
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO tags(name,color_h,color_s,color_l) VALUES(?1,?2,?3,?4)",
            params![name.trim(), h, s, l],
        )?;
        Ok(Tag {
            id: conn.last_insert_rowid(),
            name: name.trim().into(),
            color_h: h,
            color_s: s,
            color_l: l,
        })
    }
    fn delete_tag(&self, id: i64) -> Result<(), StorageError> {
        self.conn
            .lock()
            .execute("DELETE FROM tags WHERE id=?1", [id])?;
        Ok(())
    }
    fn get_all_tags(&self) -> Result<Vec<Tag>, StorageError> {
        Ok(self
            .conn
            .lock()
            .prepare("SELECT id,name,color_h,color_s,color_l FROM tags ORDER BY name")?
            .query_map([], |row| {
                Ok(Tag {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    color_h: row.get(2)?,
                    color_s: row.get(3)?,
                    color_l: row.get(4)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?)
    }
    fn assign_tag(&self, file_id: &str, tag_id: i64) -> Result<(), StorageError> {
        self.conn.lock().execute(
            "INSERT OR IGNORE INTO file_tags(file_id,tag_id,assigned_at) VALUES(?1,?2,unixepoch())",
            params![file_id, tag_id],
        )?;
        Ok(())
    }
    fn remove_tag_from_file(&self, file_id: &str, tag_id: i64) -> Result<(), StorageError> {
        self.conn.lock().execute(
            "DELETE FROM file_tags WHERE file_id=?1 AND tag_id=?2",
            params![file_id, tag_id],
        )?;
        Ok(())
    }
}

impl HistoryRepository for SqliteStorage {
    fn record_visit(&self, path: &Path) -> Result<(), StorageError> {
        self.conn.lock().execute("INSERT INTO history(path,visit_count,last_visited_at) VALUES(?1,1,unixepoch()) ON CONFLICT(path) DO UPDATE SET visit_count=visit_count+1,last_visited_at=excluded.last_visited_at", [crate::shared::utils::config_path(path)])?;
        Ok(())
    }
    fn get_frequent(&self, limit: usize) -> Result<Vec<PathBuf>, StorageError> {
        Ok(self
            .conn
            .lock()
            .prepare("SELECT path FROM history ORDER BY visit_count DESC LIMIT ?1")?
            .query_map([limit.min(HISTORY_FREQUENT_LIMIT) as i64], |row| {
                let value: String = row.get(0)?;
                crate::shared::utils::parse_config_path(&value)
                    .map_err(|_| rusqlite::Error::InvalidQuery)
            })?
            .collect::<Result<Vec<_>, _>>()?)
    }
}

impl SyncRepository for SqliteStorage {
    fn pending(&self, limit: usize) -> Result<Vec<OutboxItem>, StorageError> {
        Ok(self
            .conn
            .lock()
            .prepare("SELECT id,revision,deleted FROM vector_outbox ORDER BY revision,id LIMIT ?1")?
            .query_map([limit.min(MAX_RESULTS) as i64], |row| {
                Ok(OutboxItem {
                    id: row.get(0)?,
                    revision: row.get(1)?,
                    is_deleted: row.get(2)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?)
    }
    fn pending_count(&self) -> Result<usize, StorageError> {
        let count: i64 =
            self.conn
                .lock()
                .query_row("SELECT count(*) FROM vector_outbox", [], |row| row.get(0))?;
        Ok(count.max(0) as usize)
    }
    fn acknowledge(&self, id: &str, revision: i64) -> Result<(), StorageError> {
        self.conn.lock().execute(
            "DELETE FROM vector_outbox WHERE id=?1 AND revision=?2",
            params![id, revision],
        )?;
        Ok(())
    }
    fn enqueue_all(&self) -> Result<(), StorageError> {
        self.conn.lock().execute("INSERT INTO vector_outbox SELECT id,revision,missing FROM files WHERE true ON CONFLICT(id) DO UPDATE SET revision=excluded.revision,deleted=excluded.deleted", [])?;
        Ok(())
    }
    fn dataset(&self) -> Result<Option<DatasetManifest>, StorageError> {
        let value: Option<String> = self
            .conn
            .lock()
            .query_row(
                "SELECT value FROM app_state WHERE key='dataset'",
                [],
                |row| row.get(0),
            )
            .optional()?;
        value
            .map(|text| serde_json::from_str(&text).map_err(|_| StorageError::QueryFailed))
            .transpose()
    }
    fn save_dataset(&self, manifest: &DatasetManifest) -> Result<(), StorageError> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        tx.execute("INSERT INTO app_state(key,value) VALUES('dataset',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",[serde_json::to_string(manifest).map_err(|_| StorageError::ConstraintViolation)?])?;
        tx.execute("DELETE FROM app_state WHERE key='migration'", [])?;
        tx.commit()?;
        Ok(())
    }
    fn migration(&self) -> Result<Option<MigrationCheckpoint>, StorageError> {
        let value: Option<String> = self
            .conn
            .lock()
            .query_row(
                "SELECT value FROM app_state WHERE key='migration'",
                [],
                |row| row.get(0),
            )
            .optional()?;
        value
            .map(|value| serde_json::from_str(&value).map_err(|_| StorageError::QueryFailed))
            .transpose()
    }
    fn save_migration(&self, checkpoint: &MigrationCheckpoint) -> Result<(), StorageError> {
        self.conn.lock().execute("INSERT INTO app_state(key,value) VALUES('migration',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",[serde_json::to_string(checkpoint).map_err(|_| StorageError::ConstraintViolation)?])?;
        Ok(())
    }
    fn projection(&self, collection: &str, id: &str) -> Result<Option<String>, StorageError> {
        Ok(self
            .conn
            .lock()
            .query_row(
                "SELECT value FROM app_state WHERE key=?1",
                [format!("projection:{collection}:{id}")],
                |row| row.get(0),
            )
            .optional()?)
    }
    fn clear_projection(&self, collection: &str, id: Option<&str>) -> Result<(), StorageError> {
        let conn = self.conn.lock();
        if let Some(id) = id {
            conn.execute(
                "DELETE FROM app_state WHERE key=?1",
                [format!("projection:{collection}:{id}")],
            )?;
        } else {
            let prefix = format!("projection:{collection}:");
            conn.execute(
                "DELETE FROM app_state WHERE substr(key,1,?1)=?2",
                params![prefix.len(), prefix],
            )?;
        }
        Ok(())
    }
    fn save_projection(&self, collection: &str, id: &str, hash: &str) -> Result<(), StorageError> {
        if hash.len() != 64 || !hash.chars().all(|ch| ch.is_ascii_hexdigit()) {
            return Err(StorageError::ConstraintViolation);
        }
        self.conn.lock().execute("INSERT INTO app_state(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",params![format!("projection:{collection}:{id}"),hash])?;
        Ok(())
    }
    fn all_files(&self) -> Result<Vec<FileItem>, StorageError> {
        Self::read_files(
            &self.conn.lock(),
            &format!("SELECT {FILE_COLUMNS} FROM files f ORDER BY f.id"),
            &[],
        )
    }
}

impl CacheRepository for SqliteStorage {
    fn get_size(&self, path: &Path) -> Result<Option<FolderSize>, StorageError> {
        let result: Option<(i64, String, i64)> = self
            .conn
            .lock()
            .query_row(
                "SELECT bytes,state,measured_at FROM folder_sizes WHERE path_key=?1",
                [path_key(path)],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        Ok(result.map(|(bytes, state, measured_at)| FolderSize {
            path: path.into(),
            bytes: bytes.max(0) as u64,
            state: if Utc::now().timestamp() - measured_at > SIZE_CACHE_TTL_SECS {
                SizeState::Stale
            } else if state == "complete" {
                SizeState::Complete
            } else {
                SizeState::Partial
            },
            measured_at,
        }))
    }
    fn save_size(&self, size: &FolderSize) -> Result<(), StorageError> {
        self.conn.lock().execute("INSERT INTO folder_sizes VALUES(?1,?2,?3,?4) ON CONFLICT(path_key) DO UPDATE SET bytes=excluded.bytes,state=excluded.state,measured_at=excluded.measured_at", params![path_key(&size.path),size.bytes.min(i64::MAX as u64) as i64,if size.state==SizeState::Complete { "complete" } else { "partial" },size.measured_at])?;
        Ok(())
    }
    fn invalidate_sizes(&self, path: &Path) -> Result<(), StorageError> {
        let conn = self.conn.lock();
        for ancestor in path.ancestors() {
            conn.execute(
                "DELETE FROM folder_sizes WHERE path_key=?1",
                [path_key(ancestor)],
            )?;
        }
        Ok(())
    }
    fn clear_caches(&self) -> Result<(), StorageError> {
        self.conn.lock().execute("DELETE FROM folder_sizes", [])?;
        Ok(())
    }
    fn backup(&self, destination: &Path) -> Result<(), StorageError> {
        if !destination.is_absolute() || destination.exists() || destination == self.database {
            return Err(StorageError::ConstraintViolation);
        }
        let mut output = Connection::open(destination)?;
        let conn = self.conn.lock();
        let backup = rusqlite::backup::Backup::new(&conn, &mut output)?;
        backup.run_to_completion(128, std::time::Duration::from_millis(10), None)?;
        Ok(())
    }
    fn restore(&self, source: &Path) -> Result<(), StorageError> {
        if !source.is_absolute()
            || crate::shared::utils::file_identity(source)
                == crate::shared::utils::file_identity(&self.database)
        {
            return Err(StorageError::ConstraintViolation);
        }
        let input =
            Connection::open_with_flags(source, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let integrity: String = input.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
        let version: i64 =
            input.query_row("SELECT max(version) FROM schema_migrations", [], |row| {
                row.get(0)
            })?;
        if integrity != "ok" || version != 4 {
            return Err(StorageError::ConstraintViolation);
        }
        let mut conn = self.conn.lock();
        let backup = rusqlite::backup::Backup::new(&input, &mut conn)?;
        backup.run_to_completion(128, std::time::Duration::from_millis(10), None)?;
        Ok(())
    }
}

impl OperationRepository for SqliteStorage {
    fn journal(
        &self,
        id: &str,
        source: &Path,
        target: &Path,
        phase: &str,
    ) -> Result<(), StorageError> {
        self.conn.lock().execute("INSERT INTO operation_journal VALUES(?1,?2,?3,?4,unixepoch()) ON CONFLICT(id,source) DO UPDATE SET target=excluded.target,phase=excluded.phase,updated_at=excluded.updated_at", params![id,path_key(source),path_key(target),phase])?;
        Ok(())
    }
    fn finish_operation(&self, id: &str, phase: &str) -> Result<(), StorageError> {
        // Committed phases mean the filesystem changed but metadata may lag; startup recovery
        // needs them, so only steps that never reached the filesystem are closed here.
        self.conn.lock().execute(
            "UPDATE operation_journal SET phase=?2,updated_at=unixepoch() WHERE id=?1 AND phase NOT IN('complete','cancelled','failed','move:committed','rename:intermediate')",
            params![id, phase],
        )?;
        Ok(())
    }
    fn unfinished_operations(
        &self,
    ) -> Result<Vec<(String, PathBuf, PathBuf, String)>, StorageError> {
        Ok(self.conn.lock().prepare("SELECT id,source,target,phase FROM operation_journal WHERE phase NOT IN('complete','cancelled','failed')")?.query_map([], |row| Ok((row.get(0)?,path_from_key(row.get(1)?),path_from_key(row.get(2)?),row.get(3)?)))?.collect::<Result<Vec<_>,_>>()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn storage() -> (SqliteStorage, PathBuf) {
        let root = std::env::temp_dir().join(format!("filemofm-test-{}", uuid::Uuid::new_v4()));
        let data = root.join("data");
        std::fs::create_dir_all(&data).unwrap();
        let paths = RuntimePaths {
            config: root.join("config.toml"),
            database: data.join(DATABASE_NAME),
            data,
            logs: root.join("logs"),
            models: root.join("models"),
            runtime: root.join("runtime"),
            trash: root.join("trash"),
            root: root.clone(),
        };
        (SqliteStorage::new(&paths).unwrap(), root)
    }

    fn item(path: &Path, is_directory: bool) -> FileItem {
        FileItem {
            id: uuid::Uuid::new_v4().to_string(),
            path: path.into(),
            name: path.file_name().unwrap().to_string_lossy().into_owned(),
            extension: path
                .extension()
                .map(|value| value.to_string_lossy().into_owned())
                .unwrap_or_default(),
            size: 1,
            is_directory,
            created_at: 1,
            modified_at: 1,
            updated_at: 1,
            revision: 1,
            native_id: Some(uuid::Uuid::new_v4().to_string()),
            is_hidden: false,
            is_missing: false,
            tags: Vec::new(),
            description: String::new(),
        }
    }

    #[test]
    fn identity_survives_atomic_save_and_rename() {
        let (storage, root) = storage();
        let folder = root.join("docs");
        let file = folder.join("note.md");
        let mut files = vec![item(&folder, true), item(&file, false)];
        storage.upsert_files(&mut files).unwrap();
        let id = files[1].id.clone();
        let tag = storage.create_tag("Work", 210.0, 0.5, 0.5).unwrap();
        storage.assign_tag(&id, tag.id).unwrap();

        // Same path, new inode (editor saved via temp file + rename): ID and tags stay.
        let mut saved = vec![item(&file, false)];
        storage.upsert_files(&mut saved).unwrap();
        assert_eq!(saved[0].id, id);
        assert_eq!(saved[0].tags.len(), 1);

        // Renaming the parent moves descendants by prefix, not `docs2` siblings.
        let sibling = root.join("docs2");
        storage.upsert_files(&mut [item(&sibling, true)]).unwrap();
        storage.update_path(&folder, &root.join("archive")).unwrap();
        let moved = storage.get_file(&id).unwrap().unwrap();
        assert_eq!(moved.path, root.join("archive").join("note.md"));
        assert_eq!(storage.files_under(&sibling).unwrap().len(), 1);

        // Case-insensitive tag filter.
        let query = SearchQuery {
            text: String::new(),
            filters: vec![SearchFilter::Tag("work".into())],
            ids: Vec::new(),
            scope: SearchScope::IndexedFiles,
            directory: root.clone(),
            roots: Vec::new(),
            limit: 10,
            offset: 0,
        };
        assert_eq!(storage.search_keyword(&query).unwrap().len(), 1);

        // Folder replaced by a file of the same name becomes a new record.
        let mut replaced = vec![item(&sibling, false)];
        storage.upsert_files(&mut replaced).unwrap();
        assert!(storage.files_under(&sibling).unwrap().len() == 1);

        storage.mark_missing(&root.join("archive")).unwrap();
        assert!(storage.get_file(&id).unwrap().unwrap().is_missing);
        assert!(storage.pending_count().unwrap() > 0);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn journal_keeps_committed_steps() {
        let (storage, root) = storage();
        let a = root.join("a");
        storage.journal("op", &a, &a, "transfer:prepared").unwrap();
        storage
            .journal("op", &root.join("b"), &a, "move:committed")
            .unwrap();
        storage.finish_operation("op", "failed").unwrap();
        let open = storage.unfinished_operations().unwrap();
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].3, "move:committed");
        let _ = std::fs::remove_dir_all(root);
    }
}
