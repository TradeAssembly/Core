// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use crate::schedule_observation::ScheduleObservation;
use rusqlite::{Connection, OptionalExtension};

pub(super) fn observe(
    connection: &Connection,
    schedule_key: &str,
) -> rusqlite::Result<Option<ScheduleObservation>> {
    connection
        .query_row(
            "SELECT state, due_at_ms FROM runtime_schedules WHERE schedule_key=?1",
            [schedule_key],
            |row| {
                Ok(ScheduleObservation {
                    state: row.get(0)?,
                    due_at_ms: row.get(1)?,
                })
            },
        )
        .optional()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seed(connection: &Connection) {
        connection.execute_batch(
            "CREATE TABLE runtime_schedules (schedule_key TEXT PRIMARY KEY, state TEXT NOT NULL, due_at_ms INTEGER NOT NULL, lease_owner TEXT, lease_until_ms INTEGER, fencing_token INTEGER NOT NULL DEFAULT 0);
             INSERT INTO runtime_schedules VALUES ('selected', 'pending', 1000, NULL, NULL, 0);
             INSERT INTO runtime_schedules VALUES ('other', 'pending', 2000, 'other-worker', 3000, 7);",
        ).unwrap();
    }

    #[test]
    fn reads_do_not_claim_renew_or_change_another_schedule() {
        let connection = Connection::open_in_memory().unwrap();
        seed(&connection);
        for _ in 0..3 {
            assert_eq!(
                observe(&connection, "selected")
                    .unwrap()
                    .unwrap()
                    .next_due_at_ms(),
                Some(1000)
            );
        }
        assert_eq!(observe(&connection, "missing").unwrap(), None);
        let state: (Option<String>, Option<i64>, i64) = connection.query_row(
            "SELECT lease_owner, lease_until_ms, fencing_token FROM runtime_schedules WHERE schedule_key='selected'", [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        ).unwrap();
        assert_eq!(state, (None, None, 0));
        assert_eq!(
            observe(&connection, "other")
                .unwrap()
                .unwrap()
                .next_due_at_ms(),
            Some(2000)
        );
        let other: (String, i64, i64) = connection.query_row(
            "SELECT lease_owner, lease_until_ms, fencing_token FROM runtime_schedules WHERE schedule_key='other'", [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        ).unwrap();
        assert_eq!(other, ("other-worker".to_string(), 3000, 7));
    }

    #[test]
    fn missing_schema_is_an_error_instead_of_invented_timing() {
        assert!(observe(&Connection::open_in_memory().unwrap(), "missing").is_err());
    }

    #[test]
    fn reopening_preserves_due_time_and_terminal_work_has_no_next_due() {
        let path =
            std::env::temp_dir().join(format!("schedule-observation-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let connection = Connection::open(&path).unwrap();
        seed(&connection);
        drop(connection);
        let reopened = Connection::open(&path).unwrap();
        assert_eq!(
            observe(&reopened, "selected")
                .unwrap()
                .unwrap()
                .next_due_at_ms(),
            Some(1000)
        );
        for state in ["complete", "canceled"] {
            reopened
                .execute(
                    "UPDATE runtime_schedules SET state=?1 WHERE schedule_key='selected'",
                    [state],
                )
                .unwrap();
            assert_eq!(
                observe(&reopened, "selected")
                    .unwrap()
                    .unwrap()
                    .next_due_at_ms(),
                None
            );
        }
        drop(reopened);
        let _ = std::fs::remove_file(path);
    }
}
