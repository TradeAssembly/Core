// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use super::{operations::LocalSqliteOperations, sqlite::LocalSqliteStorage};
use rusqlite::{Connection, OpenFlags};
use serde::Serialize;
use std::path::Path;

pub const LOCAL_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalSchemaStatus {
    pub database_exists: bool,
    pub installed_version: u32,
    pub target_version: u32,
    pub action: &'static str,
}

pub fn inspect_local_schema(path: &Path) -> Result<LocalSchemaStatus, String> {
    if path == Path::new(":memory:") {
        return Ok(LocalSchemaStatus {
            database_exists: false,
            installed_version: LOCAL_SCHEMA_VERSION,
            target_version: LOCAL_SCHEMA_VERSION,
            action: "none",
        });
    }
    if !path.is_file() || path.metadata().map_err(redacted_io)?.len() == 0 {
        return Ok(LocalSchemaStatus {
            database_exists: false,
            installed_version: 0,
            target_version: LOCAL_SCHEMA_VERSION,
            action: "initialize",
        });
    }
    let connection = open_read_only(path)?;
    let installed_version = user_version(&connection)?;
    if installed_version > LOCAL_SCHEMA_VERSION {
        return Err("local database schema is newer than this Core release".to_string());
    }
    let action = if installed_version == LOCAL_SCHEMA_VERSION {
        "none"
    } else {
        "upgrade"
    };
    Ok(LocalSchemaStatus {
        database_exists: true,
        installed_version,
        target_version: LOCAL_SCHEMA_VERSION,
        action,
    })
}

pub fn assert_runtime_compatible(path: &Path) -> Result<(), String> {
    if path == Path::new(":memory:") {
        return Ok(());
    }
    let status = inspect_local_schema(path)?;
    if !status.database_exists {
        upgrade_local_schema(path)?;
        return Ok(());
    }
    if status.database_exists && status.installed_version < LOCAL_SCHEMA_VERSION {
        return Err(
            "local database schema upgrade required: run `cargo xtask source-upgrade plan`"
                .to_string(),
        );
    }
    validate_local_schema(path)
}

pub fn upgrade_local_schema(path: &Path) -> Result<LocalSchemaStatus, String> {
    let before = inspect_local_schema(path)?;
    if before.action == "none" {
        validate_local_schema(path)?;
        return Ok(before);
    }
    LocalSqliteStorage::new(path.display().to_string()).prepare_upgrade()?;
    LocalSqliteOperations::new(path.display().to_string(), false).prepare_upgrade()?;
    let connection = Connection::open(path).map_err(redacted_sqlite)?;
    connection
        .execute_batch(&format!("PRAGMA user_version = {LOCAL_SCHEMA_VERSION};"))
        .map_err(redacted_sqlite)?;
    drop(connection);
    validate_local_schema(path)?;
    inspect_local_schema(path)
}

pub fn validate_local_schema(path: &Path) -> Result<(), String> {
    let connection = open_read_only(path)?;
    let version = user_version(&connection)?;
    if version != LOCAL_SCHEMA_VERSION {
        return Err("local database schema version mismatch".to_string());
    }
    let integrity = connection
        .query_row("PRAGMA quick_check", [], |row| row.get::<_, String>(0))
        .map_err(redacted_sqlite)?;
    if integrity != "ok" {
        return Err("local database integrity check failed".to_string());
    }
    for (table, columns) in [
        (
            "tradeassembly_kv",
            &[
                "namespace",
                "item_key",
                "value_json",
                "idempotency_key",
                "authority_actor",
                "updated_at_ms",
            ][..],
        ),
        (
            "runtime_queue",
            &["message_id", "queue_name", "idempotency_key", "state"][..],
        ),
        (
            "runtime_events",
            &["event_id", "stream_name", "sequence_num", "idempotency_key"][..],
        ),
        (
            "runtime_outbox",
            &["outbox_id", "idempotency_key", "state", "fencing_token"][..],
        ),
        (
            "runtime_evidence",
            &["evidence_id", "idempotency_key", "payload_json"][..],
        ),
    ] {
        require_columns(&connection, table, columns)?;
    }
    Ok(())
}

fn require_columns(connection: &Connection, table: &str, required: &[&str]) -> Result<(), String> {
    let mut statement = connection
        .prepare(&format!("PRAGMA table_info(\"{table}\")"))
        .map_err(redacted_sqlite)?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(redacted_sqlite)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(redacted_sqlite)?;
    if required
        .iter()
        .all(|required| columns.iter().any(|column| column == required))
    {
        Ok(())
    } else {
        Err("local database schema validation failed".to_string())
    }
}

fn open_read_only(path: &Path) -> Result<Connection, String> {
    Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(redacted_sqlite)
}

fn user_version(connection: &Connection) -> Result<u32, String> {
    connection
        .query_row("PRAGMA user_version", [], |row| row.get::<_, u32>(0))
        .map_err(redacted_sqlite)
}

pub(crate) fn has_user_tables(connection: &Connection) -> Result<bool, String> {
    connection
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM sqlite_master
                WHERE type='table' AND name NOT LIKE 'sqlite_%'
            )",
            [],
            |row| row.get::<_, bool>(0),
        )
        .map_err(redacted_sqlite)
}

pub(crate) fn mark_current_if_new(connection: &Connection, was_empty: bool) -> Result<(), String> {
    if was_empty {
        connection
            .execute_batch(&format!("PRAGMA user_version = {LOCAL_SCHEMA_VERSION};"))
            .map_err(redacted_sqlite)?;
    }
    Ok(())
}

fn redacted_sqlite(_error: rusqlite::Error) -> String {
    "local database schema operation failed".to_string()
}

fn redacted_io(_error: std::io::Error) -> String {
    "local database schema inspection failed".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;
    use tempfile::TempDir;

    #[test]
    fn unversioned_data_is_upgraded_and_future_schema_fails_closed() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("tradeassembly.db");
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE tradeassembly_kv (
                    namespace TEXT NOT NULL,
                    item_key TEXT NOT NULL,
                    value_json TEXT NOT NULL,
                    idempotency_key TEXT NOT NULL,
                    authority_actor TEXT NOT NULL,
                    updated_at_ms INTEGER NOT NULL,
                    PRIMARY KEY(namespace, item_key)
                );",
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO tradeassembly_kv VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params!["strategy", "one", r#"{"name":"one"}"#, "idem", "user", 1],
            )
            .unwrap();
        drop(connection);

        assert!(assert_runtime_compatible(&path)
            .unwrap_err()
            .contains("upgrade required"));
        let status = upgrade_local_schema(&path).unwrap();
        assert_eq!(status.installed_version, LOCAL_SCHEMA_VERSION);
        let connection = open_read_only(&path).unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT value_json FROM tradeassembly_kv WHERE item_key='one'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            r#"{"name":"one"}"#
        );
        drop(connection);

        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch("PRAGMA user_version = 999;")
            .unwrap();
        drop(connection);
        assert!(inspect_local_schema(&path).unwrap_err().contains("newer"));
    }

    #[test]
    fn in_memory_profiles_do_not_use_file_upgrade_bookkeeping() {
        let path = Path::new(":memory:");
        assert_runtime_compatible(path).unwrap();
        assert_eq!(inspect_local_schema(path).unwrap().action, "none");
    }
}
