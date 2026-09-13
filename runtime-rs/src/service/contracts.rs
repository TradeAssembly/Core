// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use serde_json::{json, Value};

pub(crate) fn validation_result(ok: bool, errors: Value) -> Value {
    json!({
        "ok": ok,
        "errors": errors,
        "deterministic": true,
    })
}
