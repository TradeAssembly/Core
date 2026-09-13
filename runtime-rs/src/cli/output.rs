// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use serde_json::{json, Value};

pub fn success(data: Value, command: &str) -> Value {
    json!({
        "ok": true,
        "data": data,
        "warnings": [],
        "errors": [],
        "journal_id": null,
        "authority": {
            "actor": "local-user",
            "surface": "cli",
            "account_mode": "paper"
        },
        "idempotency_key": format!("cli:{command}"),
    })
}

pub fn failure(data: Value, command: &str) -> Value {
    json!({
        "ok": false,
        "data": data,
        "warnings": [],
        "errors": [{"code": "command_failed"}],
        "journal_id": null,
        "authority": {
            "actor": "local-user",
            "surface": "cli",
            "account_mode": "paper"
        },
        "idempotency_key": format!("cli:{command}"),
    })
}
