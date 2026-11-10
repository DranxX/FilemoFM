import json
import re
import sqlite3
import uuid
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SOURCE = (ROOT / "src/infrastructure/sqlite.rs").read_text()


def sql_constant(name):
    match = re.search(rf'const {name}: &str = "(.*?)";', SOURCE, re.S)
    if match is None:
        raise RuntimeError(f"Missing SQL constant {name}")
    return match.group(1)


def main():
    connection = sqlite3.connect(":memory:")
    connection.execute("PRAGMA foreign_keys=ON")
    connection.executescript(sql_constant("SCHEMA"))
    original = str(uuid.uuid4())
    replacement = str(uuid.uuid4())
    reference = str(uuid.uuid4())
    key = b"/data/report.md"
    connection.execute("INSERT INTO files VALUES(?,?,?,?,?,?,?,?,?,?)", (original, key.decode(), "report.md", "md", 12, 0, 1, 1, 1, "quarterly report"))
    connection.executescript(sql_constant("MIGRATION"))
    connection.execute("UPDATE files SET path_key=?,parent_key=? WHERE id=?", (key, b"/data", original))
    migration = re.search(r'tx.execute_batch\("(ALTER TABLE files ADD COLUMN native_id.*?)"\)\?;', SOURCE)
    if migration is None:
        raise RuntimeError("Missing native identity migration")
    connection.executescript(migration.group(1))
    passed = []
    assert connection.execute("SELECT description FROM files WHERE id=?", (original,)).fetchone()[0] == "quarterly report"
    assert connection.execute("SELECT max(version) FROM schema_migrations").fetchone()[0] == 4
    passed.append("legacy metadata survives schema migration")
    connection.execute("INSERT INTO tags(name,color_h,color_s,color_l) VALUES(?,?,?,?)", ("Work", 210, 0.6, 0.5))
    tag = connection.execute("SELECT id FROM tags").fetchone()[0]
    previous = connection.execute("SELECT revision FROM files WHERE id=?", (original,)).fetchone()[0]
    connection.execute("INSERT INTO file_tags VALUES(?,?,?)", (original, tag, 1))
    current = connection.execute("SELECT revision FROM files WHERE id=?", (original,)).fetchone()[0]
    assert current == previous + 1
    assert connection.execute("SELECT revision FROM vector_outbox WHERE id=?", (original,)).fetchone()[0] == current
    passed.append("tag changes advance revision and durable outbox together")
    assert connection.execute("UPDATE files SET description=? WHERE id=? AND revision=?", ("new text", original, previous)).rowcount == 0
    assert connection.execute("UPDATE files SET description=? WHERE id=? AND revision=?", ("updated quarterly report", original, current)).rowcount == 1
    passed.append("stale description writes cannot overwrite committed metadata")
    ranked = connection.execute("SELECT f.id FROM files f WHERE f.rowid IN(SELECT rowid FROM file_fts WHERE file_fts MATCH ?) ORDER BY coalesce((SELECT bm25(file_fts) FROM file_fts WHERE rowid=f.rowid AND file_fts MATCH ?),0)", ('"quarterly"', '"quarterly"')).fetchall()
    assert ranked == [(original,)]
    passed.append("FTS trigger refresh and correlated BM25 query execute")
    connection.execute("INSERT INTO files(id,path,path_key,parent_key,name,size,is_directory,created_at,modified_at,updated_at,description) VALUES(?,?,?,?,?,?,?,?,?,?,?)", (reference, "/data/index.md", b"/data/index.md", b"/data", "index.md", 0, 0, 1, 1, 1, "reference"))
    connection.execute("INSERT INTO file_references VALUES(?,?)", (reference, original))
    connection.execute("UPDATE files SET missing=1,path='missing:'||id WHERE id=?", (original,))
    connection.execute("INSERT INTO files(id,path,path_key,parent_key,name,size,is_directory,created_at,modified_at,updated_at) VALUES(?,?,?,?,?,?,?,?,?,?)", (replacement, key.decode(), key, b"/data", "report.md", 12, 0, 1, 1, 1))
    assert connection.execute("SELECT target_id FROM file_references WHERE source_id=?", (reference,)).fetchone()[0] == original
    assert connection.execute("SELECT id FROM files WHERE path_key=? AND missing=0", (key,)).fetchone()[0] == replacement
    assert connection.execute("SELECT description FROM files WHERE id=?", (original,)).fetchone()[0] == "updated quarterly report"
    assert connection.execute("SELECT deleted FROM vector_outbox WHERE id=?", (original,)).fetchone()[0] == 1
    passed.append("reusing a deleted path cannot rebind stable references or descriptions")
    connection.execute("UPDATE files SET missing=1,path='missing:'||id WHERE id=?", (replacement,))
    connection.execute("UPDATE files SET missing=0,path=? WHERE id=?", (key.decode(), original))
    assert connection.execute("SELECT id FROM files WHERE path_key=? AND missing=0", (key,)).fetchone()[0] == original
    assert connection.execute("SELECT count(*) FROM file_tags WHERE file_id=?", (original,)).fetchone()[0] == 1
    passed.append("trash restore can reactivate the original identity with tags")
    parent = b"/data"
    root_prefix = parent + b"/"
    scope = re.search(r'"(\(f\.path_key=\{\} OR substr\(f\.path_key,1,\{\}\)=\{\}\))"', SOURCE)
    if scope is None:
        raise RuntimeError("Missing selected-root SQL predicate")
    predicate = scope.group(1).format("?", "?", "?")
    outside = str(uuid.uuid4())
    native = str(uuid.uuid4())
    for identity, native_key in [(outside, b"/database/report.md"), (native, b"/data/\xff.md")]:
        connection.execute("INSERT INTO files(id,path,path_key,parent_key,name,size,is_directory,created_at,modified_at,updated_at) VALUES(?,?,?,?,?,?,?,?,?,?)", (identity, "file:///" + identity, native_key, native_key.rsplit(b"/", 1)[0], "report.md", 0, 0, 1, 1, 1))
    scoped = {row[0] for row in connection.execute("SELECT f.id FROM files f WHERE f.missing=0 AND " + predicate, (parent, len(root_prefix), root_prefix))}
    assert original in scoped and native in scoped and outside not in scoped
    direct = {row[0] for row in connection.execute("SELECT id FROM files WHERE missing=0 AND parent_key=?", (parent,))}
    assert native in direct and outside not in direct
    passed.append("root and parent filters preserve native bytes and exclude similarly named roots")
    connection.execute("INSERT INTO app_state VALUES('migration',?)", ('{"phase":"cutover"}',))
    connection.commit()
    connection.execute("BEGIN")
    connection.execute("INSERT INTO app_state VALUES('dataset',?)", ('{"active":"filemofm_fixture"}',))
    connection.execute("DELETE FROM app_state WHERE key='migration'")
    connection.rollback()
    assert connection.execute("SELECT count(*) FROM app_state WHERE key='migration'").fetchone()[0] == 1
    assert connection.execute("SELECT count(*) FROM app_state WHERE key='dataset'").fetchone()[0] == 0
    passed.append("interrupted cutover retains checkpoint and previous active pointer")
    connection.execute("DELETE FROM tags WHERE id=?", (tag,))
    assert connection.execute("SELECT count(*) FROM file_tags WHERE file_id=?", (original,)).fetchone()[0] == 0
    passed.append("tag deletion removes assignments through enforced foreign keys")
    assert connection.execute("PRAGMA integrity_check").fetchone()[0] == "ok"
    assert connection.execute("PRAGMA foreign_key_check").fetchall() == []
    print(json.dumps({"sqlite": sqlite3.sqlite_version, "passed": passed, "rust_compilation_run": False, "scope": "SQL schema and invariants through Python SQLite; Rust adapters and bundled SQLite are unverified"}, indent=2))


if __name__ == "__main__":
    main()
