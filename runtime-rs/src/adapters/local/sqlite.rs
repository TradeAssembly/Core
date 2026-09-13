// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use crate::ports::storage::StoragePort;
use crate::ports::{
    ComparePutOutcome, FailureMode, ImmutablePutOutcome, PortDescriptor, PortKind,
    SideEffectContext, StorageExpectation, StorageWrite, VersionedPort,
};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde_json::Value;
use std::io::Read;
use std::path::Path;
use std::time::Duration;

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS tradeassembly_kv (
    namespace TEXT NOT NULL,
    item_key TEXT NOT NULL,
    value_json TEXT NOT NULL,
    idempotency_key TEXT NOT NULL,
    authority_actor TEXT NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    PRIMARY KEY (namespace, item_key)
);
CREATE INDEX IF NOT EXISTS tradeassembly_kv_namespace_idx
    ON tradeassembly_kv(namespace, item_key);
"#;

#[derive(Debug)]
pub struct LocalSqliteStorage {
    db_path: String,
}

impl LocalSqliteStorage {
    pub fn new(db_path: impl Into<String>) -> Self {
        Self {
            db_path: db_path.into(),
        }
    }

    pub fn db_path(&self) -> &str {
        &self.db_path
    }

    pub fn verify_wal(&self) -> Result<bool, String> {
        let connection = self.connection()?;
        let mode: String = connection
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .map_err(redacted_sqlite_error)?;
        Ok(mode.eq_ignore_ascii_case("wal") || self.db_path == ":memory:")
    }

    pub(crate) fn prepare_upgrade(&self) -> Result<(), String> {
        self.connection().map(drop)
    }

    fn connection(&self) -> Result<Connection, String> {
        if self.db_path != ":memory:" {
            if let Some(parent) = Path::new(&self.db_path)
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
            {
                std::fs::create_dir_all(parent)
                    .map_err(|error| format!("create local database directory: {error}"))?;
            }
        }
        migrate_legacy_json_store(Path::new(&self.db_path))?;
        let mut connection = Connection::open(&self.db_path).map_err(redacted_sqlite_error)?;
        let was_empty = !super::schema::has_user_tables(&connection)?;
        connection
            .busy_timeout(Duration::from_secs(5))
            .map_err(redacted_sqlite_error)?;
        connection
            .execute_batch(
                "PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON;",
            )
            .map_err(redacted_sqlite_error)?;
        migrate_legacy_sqlite_schema(&mut connection)?;
        connection
            .execute_batch(SCHEMA)
            .map_err(redacted_sqlite_error)?;
        super::schema::mark_current_if_new(&connection, was_empty)?;
        Ok(connection)
    }
}

fn migrate_legacy_sqlite_schema(connection: &mut Connection) -> Result<(), String> {
    let table_exists = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='tradeassembly_kv')",
            [],
            |row| row.get::<_, bool>(0),
        )
        .map_err(redacted_sqlite_error)?;
    if !table_exists {
        return Ok(());
    }

    let columns = {
        let mut statement = connection
            .prepare("PRAGMA table_info(tradeassembly_kv)")
            .map_err(redacted_sqlite_error)?;
        let columns = statement
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(redacted_sqlite_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(redacted_sqlite_error)?;
        columns
    };
    if columns.iter().any(|column| column == "item_key") {
        return Ok(());
    }
    let legacy_columns = ["namespace", "key", "value_json", "updated_at"];
    if !legacy_columns
        .iter()
        .all(|required| columns.iter().any(|column| column == required))
    {
        return Err("unsupported local storage schema".to_string());
    }

    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(redacted_sqlite_error)?;
    transaction
        .execute_batch(
            r#"
            DROP INDEX IF EXISTS tradeassembly_kv_namespace_idx;
            ALTER TABLE tradeassembly_kv RENAME TO tradeassembly_kv_legacy;
            CREATE TABLE tradeassembly_kv (
                namespace TEXT NOT NULL,
                item_key TEXT NOT NULL,
                value_json TEXT NOT NULL,
                idempotency_key TEXT NOT NULL,
                authority_actor TEXT NOT NULL,
                updated_at_ms INTEGER NOT NULL,
                PRIMARY KEY (namespace, item_key)
            );
            CREATE INDEX tradeassembly_kv_namespace_idx
                ON tradeassembly_kv(namespace, item_key);
            INSERT INTO tradeassembly_kv (
                namespace,
                item_key,
                value_json,
                idempotency_key,
                authority_actor,
                updated_at_ms
            )
            SELECT
                namespace,
                key,
                value_json,
                'legacy-sqlite:' || namespace || ':' || key,
                'local.migration',
                COALESCE(
                    CAST(strftime('%s', updated_at) AS INTEGER) * 1000,
                    CAST(unixepoch('subsec') * 1000 AS INTEGER)
                )
            FROM tradeassembly_kv_legacy;
            DROP TABLE tradeassembly_kv_legacy;
            "#,
        )
        .map_err(redacted_sqlite_error)?;
    transaction.commit().map_err(redacted_sqlite_error)
}

fn migrate_legacy_json_store(path: &Path) -> Result<(), String> {
    if path == Path::new(":memory:") || !path.exists() {
        return Ok(());
    }
    let metadata = path
        .metadata()
        .map_err(|error| format!("read local storage metadata: {error}"))?;
    if metadata.len() == 0 {
        return Ok(());
    }
    let mut first_byte = [0_u8; 1];
    let read = std::fs::File::open(path)
        .and_then(|mut file| file.read(&mut first_byte))
        .map_err(|error| format!("inspect local storage: {error}"))?;
    if read == 0 || !matches!(first_byte[0], b'{' | b'[') {
        return Ok(());
    }

    let backup = path.with_extension("legacy-json.bak");
    if backup.exists() {
        return Err("legacy storage migration backup already exists".to_string());
    }
    std::fs::rename(path, &backup)
        .map_err(|error| format!("back up legacy local storage: {error}"))?;

    let result = (|| {
        let body = std::fs::read_to_string(&backup)
            .map_err(|error| format!("read legacy local storage: {error}"))?;
        let disk: Value = serde_json::from_str(&body)
            .map_err(|error| format!("parse legacy local storage: {error}"))?;
        let mut connection = Connection::open(path).map_err(redacted_sqlite_error)?;
        connection
            .execute_batch(SCHEMA)
            .map_err(redacted_sqlite_error)?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(redacted_sqlite_error)?;
        for item in disk
            .get("items")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let namespace = item
                .get("namespace")
                .and_then(Value::as_str)
                .ok_or_else(|| "legacy storage item missing namespace".to_string())?;
            let key = item
                .get("key")
                .and_then(Value::as_str)
                .ok_or_else(|| "legacy storage item missing key".to_string())?;
            let value_json = serde_json::to_string(item.get("value").unwrap_or(&Value::Null))
                .map_err(|error| format!("serialize legacy storage item: {error}"))?;
            transaction
                .execute(
                    "INSERT INTO tradeassembly_kv(namespace, item_key, value_json, idempotency_key, authority_actor, updated_at_ms) VALUES (?1, ?2, ?3, ?4, ?5, unixepoch('subsec') * 1000)",
                    params![namespace, key, value_json, format!("legacy:{namespace}:{key}"), "local.migration"],
                )
                .map_err(redacted_sqlite_error)?;
        }
        transaction.commit().map_err(redacted_sqlite_error)
    })();

    if result.is_err() {
        let _ = std::fs::remove_file(path);
        let _ = std::fs::rename(&backup, path);
    }
    result
}

impl VersionedPort for LocalSqliteStorage {
    fn descriptors(&self) -> Vec<PortDescriptor> {
        let mut descriptor = PortDescriptor::new(PortKind::Storage, "local.sqlite-wal")
            .for_profiles(&["local"])
            .with_capabilities(&["storage.json", "storage.transactional", "storage.wal"]);
        descriptor.failure_mode = FailureMode::LocalOnly;
        descriptor.configuration_schema = serde_json::json!({
            "type": "object",
            "required": ["databasePath"],
            "properties": {"databasePath": {"type": "string"}},
            "additionalProperties": false
        });
        vec![descriptor]
    }
}

impl StoragePort for LocalSqliteStorage {
    fn adapter_name(&self) -> &'static str {
        "sqlite-wal"
    }

    fn put_json(
        &self,
        namespace: &str,
        key: &str,
        value: Value,
        context: &SideEffectContext,
    ) -> Result<(), String> {
        validate_key(namespace, key)?;
        let value_json = serde_json::to_string(&value).map_err(|error| error.to_string())?;
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(redacted_sqlite_error)?;
        transaction
            .execute(
                r#"INSERT INTO tradeassembly_kv
                   (namespace, item_key, value_json, idempotency_key, authority_actor, updated_at_ms)
                   VALUES (?1, ?2, ?3, ?4, ?5, unixepoch('subsec') * 1000)
                   ON CONFLICT(namespace, item_key) DO UPDATE SET
                     value_json=excluded.value_json,
                     idempotency_key=excluded.idempotency_key,
                     authority_actor=excluded.authority_actor,
                     updated_at_ms=excluded.updated_at_ms"#,
                params![
                    namespace,
                    key,
                    value_json,
                    context.idempotency_key.as_str(),
                    context.authority.actor
                ],
            )
            .map_err(redacted_sqlite_error)?;
        transaction.commit().map_err(redacted_sqlite_error)
    }

    fn put_json_if_absent(
        &self,
        namespace: &str,
        key: &str,
        value: Value,
        context: &SideEffectContext,
    ) -> Result<ImmutablePutOutcome, String> {
        validate_key(namespace, key)?;
        let value_json = serde_json::to_string(&value).map_err(|error| error.to_string())?;
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(redacted_sqlite_error)?;
        let inserted = transaction
            .execute(
                r#"INSERT OR IGNORE INTO tradeassembly_kv
                   (namespace, item_key, value_json, idempotency_key, authority_actor, updated_at_ms)
                   VALUES (?1, ?2, ?3, ?4, ?5, unixepoch('subsec') * 1000)"#,
                params![
                    namespace,
                    key,
                    value_json,
                    context.idempotency_key.as_str(),
                    context.authority.actor
                ],
            )
            .map_err(redacted_sqlite_error)?;
        if inserted == 1 {
            transaction.commit().map_err(redacted_sqlite_error)?;
            return Ok(ImmutablePutOutcome::Created);
        }
        let existing: String = transaction
            .query_row(
                "SELECT value_json FROM tradeassembly_kv WHERE namespace=?1 AND item_key=?2",
                params![namespace, key],
                |row| row.get(0),
            )
            .map_err(redacted_sqlite_error)?;
        let existing: Value = serde_json::from_str(&existing)
            .map_err(|_| "immutable_storage_existing_value_invalid".to_string())?;
        if existing != value {
            return Err("immutable_storage_conflict".to_string());
        }
        transaction.commit().map_err(redacted_sqlite_error)?;
        Ok(ImmutablePutOutcome::AlreadyPresent)
    }

    fn compare_and_put_json(
        &self,
        namespace: &str,
        key: &str,
        expected: Value,
        replacement: Value,
        context: &SideEffectContext,
    ) -> Result<ComparePutOutcome, String> {
        validate_key(namespace, key)?;
        let expected_json = serde_json::to_string(&expected).map_err(|error| error.to_string())?;
        let replacement_json =
            serde_json::to_string(&replacement).map_err(|error| error.to_string())?;
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(redacted_sqlite_error)?;
        let updated = transaction
            .execute(
                r#"UPDATE tradeassembly_kv
                   SET value_json=?3, idempotency_key=?4, authority_actor=?5,
                       updated_at_ms=unixepoch('subsec') * 1000
                   WHERE namespace=?1 AND item_key=?2 AND value_json=?6"#,
                params![
                    namespace,
                    key,
                    replacement_json,
                    context.idempotency_key.as_str(),
                    context.authority.actor,
                    expected_json,
                ],
            )
            .map_err(redacted_sqlite_error)?;
        transaction.commit().map_err(redacted_sqlite_error)?;
        Ok(if updated == 1 {
            ComparePutOutcome::Updated
        } else {
            ComparePutOutcome::Conflict
        })
    }

    fn put_json_batch(
        &self,
        writes: &[StorageWrite],
        expectations: &[StorageExpectation],
    ) -> Result<ComparePutOutcome, String> {
        for write in writes {
            validate_key(&write.namespace, &write.key)?;
        }
        for expectation in expectations {
            validate_key(&expectation.namespace, &expectation.key)?;
        }
        let serialized = writes
            .iter()
            .map(|write| {
                serde_json::to_string(&write.value)
                    .map(|value| (write, value))
                    .map_err(|error| error.to_string())
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(redacted_sqlite_error)?;
        for expectation in expectations {
            let current = transaction
                .query_row(
                    "SELECT value_json FROM tradeassembly_kv WHERE namespace=?1 AND item_key=?2",
                    params![expectation.namespace, expectation.key],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(redacted_sqlite_error)?
                .map(|body| serde_json::from_str::<Value>(&body))
                .transpose()
                .map_err(|_| "storage expectation value invalid".to_string())?;
            if current != expectation.value {
                return Ok(ComparePutOutcome::Conflict);
            }
        }
        for (write, value_json) in serialized {
            transaction
                .execute(
                    r#"INSERT INTO tradeassembly_kv
                       (namespace, item_key, value_json, idempotency_key, authority_actor, updated_at_ms)
                       VALUES (?1, ?2, ?3, ?4, ?5, unixepoch('subsec') * 1000)
                       ON CONFLICT(namespace, item_key) DO UPDATE SET
                         value_json=excluded.value_json,
                         idempotency_key=excluded.idempotency_key,
                         authority_actor=excluded.authority_actor,
                         updated_at_ms=excluded.updated_at_ms"#,
                    params![
                        write.namespace,
                        write.key,
                        value_json,
                        write.context.idempotency_key.as_str(),
                        write.context.authority.actor,
                    ],
                )
                .map_err(redacted_sqlite_error)?;
        }
        transaction.commit().map_err(redacted_sqlite_error)?;
        Ok(ComparePutOutcome::Updated)
    }

    fn get_json(&self, namespace: &str, key: &str) -> Result<Option<Value>, String> {
        validate_key(namespace, key)?;
        let connection = self.connection()?;
        let value = connection
            .query_row(
                "SELECT value_json FROM tradeassembly_kv WHERE namespace=?1 AND item_key=?2",
                params![namespace, key],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(redacted_sqlite_error)?;
        value
            .map(|body| serde_json::from_str(&body).map_err(|error| error.to_string()))
            .transpose()
    }

    fn list_json(&self, namespace: &str) -> Result<Vec<(String, Value)>, String> {
        if namespace.trim().is_empty() {
            return Err("storage namespace is required".to_string());
        }
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT item_key, value_json FROM tradeassembly_kv WHERE namespace=?1 ORDER BY item_key",
            )
            .map_err(redacted_sqlite_error)?;
        let rows = statement
            .query_map(params![namespace], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(redacted_sqlite_error)?;
        rows.map(|row| {
            let (key, body) = row.map_err(redacted_sqlite_error)?;
            let value = serde_json::from_str(&body).map_err(|error| error.to_string())?;
            Ok((key, value))
        })
        .collect()
    }

    fn list_json_page(
        &self,
        namespace: &str,
        after_key: Option<&str>,
        limit: usize,
    ) -> Result<Vec<(String, Value)>, String> {
        if namespace.trim().is_empty() {
            return Err("storage namespace is required".to_string());
        }
        if limit == 0 {
            return Ok(Vec::new());
        }
        let limit =
            i64::try_from(limit).map_err(|_| "storage page limit is invalid".to_string())?;
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT item_key, value_json FROM tradeassembly_kv \
                 WHERE namespace=?1 AND (?2 IS NULL OR item_key > ?2) \
                 ORDER BY item_key LIMIT ?3",
            )
            .map_err(redacted_sqlite_error)?;
        let rows = statement
            .query_map(params![namespace, after_key, limit], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(redacted_sqlite_error)?;
        rows.map(|row| {
            let (key, body) = row.map_err(redacted_sqlite_error)?;
            let value = serde_json::from_str(&body).map_err(|error| error.to_string())?;
            Ok((key, value))
        })
        .collect()
    }

    fn clear_namespace(&self, namespace: &str) -> Result<(), String> {
        if namespace.trim().is_empty() {
            return Err("storage namespace is required".to_string());
        }
        let connection = self.connection()?;
        connection
            .execute(
                "DELETE FROM tradeassembly_kv WHERE namespace=?1",
                params![namespace],
            )
            .map_err(redacted_sqlite_error)?;
        Ok(())
    }
}

fn validate_key(namespace: &str, key: &str) -> Result<(), String> {
    if namespace.trim().is_empty() || key.trim().is_empty() {
        return Err("storage namespace and key are required".to_string());
    }
    Ok(())
}

fn redacted_sqlite_error(error: rusqlite::Error) -> String {
    format!(
        "sqlite operation failed: {}",
        error
            .sqlite_error_code()
            .map_or("unknown".to_string(), |code| format!("{code:?}"))
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{AuthorityContext, IdempotencyKey};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn test_path() -> String {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        std::env::temp_dir()
            .join(format!("tradeassembly-real-sqlite-{stamp}.db"))
            .to_string_lossy()
            .to_string()
    }

    fn context() -> SideEffectContext {
        SideEffectContext::new(
            AuthorityContext::local_cli(),
            IdempotencyKey::new("storage-test").expect("idempotency key"),
        )
    }

    #[test]
    fn batch_write_rolls_back_every_item_when_one_statement_fails() {
        let storage = LocalSqliteStorage::new(test_path());
        storage
            .connection()
            .expect("storage connection")
            .execute_batch(
                r#"
                CREATE TRIGGER fail_second_batch_write
                BEFORE INSERT ON tradeassembly_kv
                WHEN NEW.namespace = 'batch_second'
                BEGIN
                    SELECT RAISE(ABORT, 'injected failure');
                END;
                "#,
            )
            .expect("failure trigger");
        let writes = vec![
            StorageWrite::new(
                "batch_first",
                "one",
                serde_json::json!({"ok": true}),
                context(),
            ),
            StorageWrite::new(
                "batch_second",
                "two",
                serde_json::json!({"ok": true}),
                context(),
            ),
        ];

        assert!(storage.put_json_batch(&writes, &[]).is_err());
        assert_eq!(
            storage
                .get_json("batch_first", "one")
                .expect("first batch lookup"),
            None
        );
        assert_eq!(
            storage
                .get_json("batch_second", "two")
                .expect("second batch lookup"),
            None
        );
    }

    #[test]
    fn batch_expectation_conflict_writes_nothing() {
        let storage = LocalSqliteStorage::new(test_path());
        storage
            .put_json(
                "clock",
                "activation",
                serde_json::json!({"last": 2}),
                &context(),
            )
            .expect("clock watermark");
        let writes = [StorageWrite::new(
            "activation",
            "one",
            serde_json::json!({"state": "active"}),
            context(),
        )];
        let expectations = [StorageExpectation::new(
            "clock",
            "activation",
            Some(serde_json::json!({"last": 1})),
        )];

        assert_eq!(
            storage
                .put_json_batch(&writes, &expectations)
                .expect("batch conflict"),
            ComparePutOutcome::Conflict
        );
        assert_eq!(
            storage
                .get_json("activation", "one")
                .expect("activation lookup"),
            None
        );
    }

    #[test]
    fn storage_is_real_sqlite_wal_and_persists_across_instances() {
        let path = test_path();
        let first = LocalSqliteStorage::new(&path);
        first
            .put_json(
                "strategy",
                "one",
                serde_json::json!({"name": "one"}),
                &context(),
            )
            .expect("put");
        assert!(first.verify_wal().expect("wal mode"));
        drop(first);

        let second = LocalSqliteStorage::new(&path);
        assert_eq!(
            second.get_json("strategy", "one").expect("get"),
            Some(serde_json::json!({"name": "one"}))
        );
        let header = std::fs::read(&path).expect("sqlite file");
        assert!(header.starts_with(b"SQLite format 3\0"));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn predecessor_sqlite_schema_is_migrated_without_data_loss() {
        let path = test_path();
        let connection = Connection::open(&path).expect("legacy sqlite database");
        connection
            .execute_batch(
                r#"
                CREATE TABLE tradeassembly_kv (
                    namespace TEXT NOT NULL,
                    key TEXT NOT NULL,
                    value_json TEXT NOT NULL,
                    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                    PRIMARY KEY (namespace, key)
                );
                INSERT INTO tradeassembly_kv(namespace, key, value_json)
                VALUES ('strategy', 'legacy', '{"name":"legacy"}');
                "#,
            )
            .expect("legacy schema");
        drop(connection);

        let storage = LocalSqliteStorage::new(&path);
        assert_eq!(
            storage.get_json("strategy", "legacy").expect("legacy read"),
            Some(serde_json::json!({"name": "legacy"}))
        );
        storage
            .put_json(
                "strategy",
                "current",
                serde_json::json!({"name": "current"}),
                &context(),
            )
            .expect("current write");
        assert_eq!(
            storage
                .get_json("strategy", "current")
                .expect("current read"),
            Some(serde_json::json!({"name": "current"}))
        );

        let connection = Connection::open(&path).expect("migrated sqlite database");
        let columns = connection
            .prepare("PRAGMA table_info(tradeassembly_kv)")
            .expect("table info")
            .query_map([], |row| row.get::<_, String>(1))
            .expect("columns")
            .collect::<Result<Vec<_>, _>>()
            .expect("column names");
        assert!(columns.iter().any(|column| column == "item_key"));
        assert!(!columns.iter().any(|column| column == "key"));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn compare_and_put_rejects_stale_state() {
        let path = test_path();
        let storage = LocalSqliteStorage::new(&path);
        let initial = serde_json::json!({"state": "ingesting", "fencingToken": 1});
        storage
            .put_json("ingestions", "one", initial.clone(), &context())
            .expect("put");
        assert_eq!(
            storage
                .compare_and_put_json(
                    "ingestions",
                    "one",
                    initial.clone(),
                    serde_json::json!({"state": "canceled", "fencingToken": 1}),
                    &context(),
                )
                .expect("cancel"),
            ComparePutOutcome::Updated
        );
        assert_eq!(
            storage
                .compare_and_put_json(
                    "ingestions",
                    "one",
                    initial,
                    serde_json::json!({"state": "completed", "fencingToken": 1}),
                    &context(),
                )
                .expect("stale write"),
            ComparePutOutcome::Conflict
        );
        assert_eq!(
            storage.get_json("ingestions", "one").expect("get"),
            Some(serde_json::json!({"state": "canceled", "fencingToken": 1}))
        );
        let _ = std::fs::remove_file(path);
    }
}
