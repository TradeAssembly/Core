// Copyright (c) 2026 OptionLab LLC. All rights reserved.
//! Callback-local SQLite handles, never cached application results or a read transaction.
//! Every statement starts an autocommit read and sees concurrent committed data.
//! Read-only handles die at the end of the outer pure-validation callback.
use super::LocalSqliteStorage;
use crate::ports::storage::ReadValidationScope;
use rusqlite::{Connection, OpenFlags};
use std::{cell::RefCell, collections::BTreeMap, marker::PhantomData, rc::Rc};

const MAX_CONNECTIONS: usize = 8;
#[derive(Clone, Debug, PartialEq, Eq)]
struct Identity {
    dev: u64,
    ino: u64,
    uid: u32,
    gid: u32,
    mode: u32,
}
fn identity(path: &str) -> Option<Identity> {
    use std::os::unix::fs::MetadataExt;
    let m = std::fs::metadata(path).ok()?;
    m.is_file().then(|| Identity {
        dev: m.dev(),
        ino: m.ino(),
        uid: m.uid(),
        gid: m.gid(),
        mode: m.mode(),
    })
}
struct Cached {
    connection: Connection,
    identity: Identity,
    schema_version: i64,
}
struct Round {
    depth: usize,
    invalidated: bool,
    connections: BTreeMap<String, Cached>,
    #[cfg(test)]
    opened: usize,
}
thread_local! {
    static ACTIVE: RefCell<Option<Round>> = const { RefCell::new(None) };
}
struct Guard(PhantomData<Rc<()>>);
impl ReadValidationScope for Guard {}
impl Drop for Guard {
    fn drop(&mut self) {
        let _ = ACTIVE.try_with(|active| {
            let mut active = active.borrow_mut();
            if let Some(round) = active.as_mut() {
                round.depth -= 1;
                if round.depth == 0 {
                    *active = None;
                }
            }
        });
    }
}
pub(super) fn begin() -> Box<dyn ReadValidationScope> {
    ACTIVE.with(|active| {
        let mut active = active.borrow_mut();
        match active.as_mut() {
            Some(round) => round.depth += 1,
            None => {
                *active = Some(Round {
                    depth: 1,
                    invalidated: false,
                    connections: BTreeMap::new(),
                    #[cfg(test)]
                    opened: 0,
                })
            }
        }
    });
    Box::new(Guard(PhantomData))
}
fn schema(connection: &Connection) -> Result<i64, String> {
    connection
        .pragma_query_value(None, "schema_version", |r| r.get(0))
        .map_err(super::redacted_sqlite_error)
}
pub(super) fn read<T>(
    store: &LocalSqliteStorage,
    f: impl FnOnce(&Connection) -> Result<T, String>,
) -> Result<T, String> {
    let mut f = Some(f);
    let reused = ACTIVE.with(|active| {
        // An unsupported/reentrant read keeps the original uncached behavior.
        let Ok(mut active) = active.try_borrow_mut() else {
            return None;
        };
        let round = active.as_mut()?;
        if round.invalidated {
            return Some(Err("storage_read_validation_invalidated".into()));
        }
        if store.db_path == ":memory:" {
            return None;
        }
        let current_identity = identity(&store.db_path);
        if let Some(cached) = round.connections.get(&store.db_path) {
            if current_identity.as_ref() != Some(&cached.identity) {
                round.invalidated = true;
                round.connections.remove(&store.db_path);
                return Some(Err("storage_database_changed_during_validation".into()));
            }
            match schema(&cached.connection) {
                Ok(version)
                    if version == cached.schema_version && cached.connection.is_autocommit() => {}
                _ => {
                    round.invalidated = true;
                    round.connections.remove(&store.db_path);
                    return Some(Err("storage_schema_changed_during_validation".into()));
                }
            }
        } else {
            if round.connections.len() >= MAX_CONNECTIONS {
                return None;
            }
            // Preserve the original migration/schema/PRAGMA initialization once.
            // Drop its writable handle before retaining the read-only handle.
            let initialized = match store.connection() {
                Ok(c) => c,
                Err(e) => return Some(Err(e)),
            };
            drop(initialized);
            let after = identity(&store.db_path);
            // Before this handle's first successful read, ordinary initialization
            // may legitimately migrate a legacy JSON file to a new SQLite inode.
            // The read-only open below is bound to the resulting current inode.
            let id = after?;
            let open = || -> Result<Cached, String> {
                let _timing = crate::service::order_plans::financial_timing(
                    "storage.validation_readonly_open",
                );
                let connection =
                    Connection::open_with_flags(&store.db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
                        .map_err(super::redacted_sqlite_error)?;
                connection
                    .busy_timeout(std::time::Duration::from_secs(5))
                    .map_err(super::redacted_sqlite_error)?;
                if identity(&store.db_path).as_ref() != Some(&id) {
                    return Err("storage_database_changed_during_validation".into());
                }
                let schema_version = schema(&connection)?;
                Ok(Cached {
                    connection,
                    identity: id,
                    schema_version,
                })
            };
            match open() {
                Ok(cached) => {
                    round.connections.insert(store.db_path.clone(), cached);
                }
                Err(e) => return Some(Err(e)),
            }
            #[cfg(test)]
            {
                round.opened += 1;
            }
        }
        let cached = &round.connections[&store.db_path];
        Some(f.take().unwrap()(&cached.connection))
    });
    match reused {
        Some(result) => result,
        None => f.take().unwrap()(&store.connection()?),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{storage::StoragePort, AuthorityContext, IdempotencyKey, SideEffectContext};
    use serde_json::json;
    fn ctx() -> SideEffectContext {
        SideEffectContext::new(
            AuthorityContext::local_cli(),
            IdempotencyKey::new("scope-test").unwrap(),
        )
    }
    #[test]
    fn one_callback_reuses_handle_but_reads_fresh_external_commits_and_keeps_cas() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.db").to_string_lossy().into_owned();
        let a = LocalSqliteStorage::new(&path);
        let b = LocalSqliteStorage::new(&path);
        a.put_json("facts", "owner", json!({"allowed":true}), &ctx())
            .unwrap();
        let scope = begin();
        for _ in 0..20 {
            assert_eq!(
                a.get_json("facts", "owner").unwrap(),
                Some(json!({"allowed":true}))
            );
        }
        ACTIVE.with(|r| assert_eq!(r.borrow().as_ref().unwrap().opened, 1));
        b.put_json("facts", "owner", json!({"allowed":false}), &ctx())
            .unwrap();
        assert_eq!(
            a.get_json("facts", "owner").unwrap(),
            Some(json!({"allowed":false}))
        );
        assert_eq!(
            a.compare_and_put_json(
                "facts",
                "owner",
                json!({"allowed":true}),
                json!({"allowed":true}),
                &ctx()
            )
            .unwrap(),
            crate::ports::ComparePutOutcome::Conflict
        );
        assert_eq!(a.list_json("facts").unwrap()[0].1, json!({"allowed":false}));
        drop(scope);
        ACTIVE.with(|r| assert!(r.borrow().is_none()));
        let scope = begin();
        assert_eq!(
            a.get_json("facts", "owner").unwrap(),
            Some(json!({"allowed":false}))
        );
        ACTIVE.with(|r| assert_eq!(r.borrow().as_ref().unwrap().opened, 1));
        drop(scope);
    }
    #[test]
    fn replaced_database_is_refused_and_scope_cleanup_runs_on_unwind() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.db");
        let other = dir.path().join("replacement.db");
        let a = LocalSqliteStorage::new(path.to_string_lossy());
        let b = LocalSqliteStorage::new(other.to_string_lossy());
        a.put_json("facts", "identity", json!(1), &ctx()).unwrap();
        b.put_json("facts", "identity", json!(2), &ctx()).unwrap();
        let scope = begin();
        assert_eq!(a.get_json("facts", "identity").unwrap(), Some(json!(1)));
        std::fs::rename(&path, dir.path().join("old.db")).unwrap();
        std::fs::rename(&other, &path).unwrap();
        assert_eq!(
            a.get_json("facts", "identity").unwrap_err(),
            "storage_database_changed_during_validation"
        );
        assert_eq!(
            a.get_json("facts", "identity").unwrap_err(),
            "storage_read_validation_invalidated"
        );
        drop(scope);
        let result = std::panic::catch_unwind(|| {
            let _scope = begin();
            assert_eq!(a.get_json("facts", "identity").unwrap(), Some(json!(2)));
            panic!("test unwind");
        });
        assert!(result.is_err());
        ACTIVE.with(|r| assert!(r.borrow().is_none()));
    }
    #[test]
    fn schema_change_refuses_without_laundering_a_read_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.db");
        let a = LocalSqliteStorage::new(path.to_string_lossy());
        a.put_json("facts", "v", json!(1), &ctx()).unwrap();
        let _scope = begin();
        a.get_json("facts", "v").unwrap();
        let c = Connection::open(&path).unwrap();
        c.execute_batch("CREATE TABLE extra_table(v INTEGER)")
            .unwrap();
        assert_eq!(
            a.get_json("facts", "v").unwrap_err(),
            "storage_schema_changed_during_validation"
        );
    }
    #[test]
    fn nesting_closes_at_outer_boundary_and_legacy_migration_stays_compatible() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("legacy.db");
        std::fs::write(
            &path,
            r#"{"items":[{"namespace":"facts","key":"v","value":{"n":1}}]}"#,
        )
        .unwrap();
        let store = LocalSqliteStorage::new(path.to_string_lossy());
        let outer = begin();
        assert_eq!(store.get_json("facts", "v").unwrap(), Some(json!({"n":1})));
        let inner = begin();
        assert_eq!(store.get_json("facts", "v").unwrap(), Some(json!({"n":1})));
        drop(inner);
        ACTIVE.with(|r| {
            let r = r.borrow();
            assert_eq!(r.as_ref().unwrap().depth, 1);
            assert_eq!(r.as_ref().unwrap().opened, 1);
        });
        drop(outer);
        ACTIVE.with(|r| assert!(r.borrow().is_none()));
        assert!(path.with_extension("legacy-json.bak").is_file());
    }
}
