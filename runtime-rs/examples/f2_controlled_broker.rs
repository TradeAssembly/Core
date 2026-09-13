//! Offline test-only controlled broker fixture.
use rusqlite::{params, Connection};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::io::{self, BufReader};
use tradeassembly_plugin_sdk::{
    read_request, write_response, PluginResponse, ProviderOutcome, Reconciliation, RequestPayload,
    ResponseStatus, DEFAULT_MAX_ENVELOPE_BYTES,
};

fn run() -> Result<(), &'static str> {
    const ACCOUNT_REF: &str = "account://mandate-live/controlled";
    if !std::fs::symlink_metadata(".f2-controlled-broker-fixture")
        .map(|m| m.file_type().is_file())
        .unwrap_or(false)
    {
        return Err("controlled_fixture_marker_required");
    }
    let request = read_request(
        BufReader::new(io::stdin().lock()),
        DEFAULT_MAX_ENVELOPE_BYTES,
    )
    .map_err(|_| "controlled_request_invalid")?;
    let body = match &request.payload {
        RequestPayload::Operation(body) | RequestPayload::BrokerOrder(body) => body,
        _ => return Err("controlled_operation_required"),
    };
    if request.metadata.operation_id == "marketdata.quote.read" {
        let symbol = body["symbol"]
            .as_str()
            .filter(|s| !s.is_empty() && s.len() <= 32)
            .ok_or("controlled_symbol_invalid")?;
        let response = PluginResponse {
            schema_version: "1".into(),
            request_id: request.request_id,
            status: ResponseStatus::Succeeded,
            response_schema: "schema://tradeassembly.f2-controlled-broker/quote@1".into(),
            payload: json!({"quote":{"symbol":symbol,"bid":"1.00","ask":"1.01","timestamp":chrono::Utc::now().to_rfc3339()}}),
            provider_outcome: ProviderOutcome {
                code: "ok".into(),
                provider_request_id: None,
                provider_reference: None,
            },
            reconciliation: Reconciliation::Reconciled,
            evidence_references: vec![],
            redacted_diagnostics: vec![],
        };
        return write_response(io::stdout().lock(), &response, DEFAULT_MAX_ENVELOPE_BYTES)
            .map_err(|_| "controlled_response_write_failed");
    }
    if request.metadata.operation_id != "broker.live_order_submit"
        && request.metadata.operation_id != "broker.order_submit"
        && request.metadata.operation_id != "broker.order_lookup"
    {
        return Err("controlled_operation_unsupported");
    }
    let client_id = body["clientOrderId"]
        .as_str()
        .filter(|s| {
            !s.is_empty()
                && s.len() <= 64
                && s.bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"-_".contains(&c))
        })
        .ok_or("controlled_client_id_invalid")?;
    let db_path = ".f2-controlled-broker.sqlite";
    if std::fs::symlink_metadata(db_path).is_ok_and(|m| !m.file_type().is_file()) {
        return Err("controlled_state_path_invalid");
    }
    let is_lookup = request.metadata.operation_id == "broker.order_lookup";
    if is_lookup && !std::path::Path::new(db_path).exists() {
        return Err("controlled_order_not_found");
    }
    let mut db = if is_lookup {
        Connection::open_with_flags(db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
    } else {
        Connection::open(db_path)
    }
    .map_err(|_| "controlled_storage_failed")?;
    db.busy_timeout(std::time::Duration::from_secs(2))
        .map_err(|_| "controlled_storage_failed")?;
    if !is_lookup {
        db.execute_batch("PRAGMA synchronous=FULL; CREATE TABLE IF NOT EXISTS orders (client_id TEXT PRIMARY KEY, account_ref TEXT NOT NULL, provider_order_id TEXT NOT NULL, symbol TEXT NOT NULL, side TEXT NOT NULL, quantity TEXT NOT NULL, order_type TEXT NOT NULL, time_in_force TEXT NOT NULL, digest TEXT NOT NULL, submissions INTEGER NOT NULL);").map_err(|_| "controlled_storage_failed")?;
        let digest = format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(body).map_err(|_| "controlled_request_invalid")?)
        );
        let tx = db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|_| "controlled_storage_failed")?;
        tx.execute("INSERT INTO orders(client_id,account_ref,provider_order_id,symbol,side,quantity,order_type,time_in_force,digest,submissions) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,1) ON CONFLICT(client_id) DO UPDATE SET submissions=submissions+1", params![client_id, ACCOUNT_REF, client_id, body["symbol"].as_str().ok_or("controlled_request_invalid")?, body["side"].as_str().ok_or("controlled_request_invalid")?, body["quantity"].as_str().ok_or("controlled_request_invalid")?, body["orderType"].as_str().ok_or("controlled_request_invalid")?, body["timeInForce"].as_str().ok_or("controlled_request_invalid")?, digest]).map_err(|_| "controlled_storage_failed")?;
        let stored: String = tx
            .query_row(
                "SELECT digest FROM orders WHERE client_id=?1",
                [client_id],
                |row| row.get(0),
            )
            .map_err(|_| "controlled_storage_failed")?;
        tx.commit().map_err(|_| "controlled_storage_failed")?;
        if stored != digest {
            return Err("controlled_idempotency_conflict");
        }
        if std::fs::symlink_metadata(".f2-controlled-broker-lose-response")
            .map(|m| m.file_type().is_file())
            .unwrap_or(false)
        {
            return Err("controlled_response_lost");
        }
    }
    let (account_ref, provider_order_id, symbol, side, quantity, order_type, time_in_force, stored, submissions): (String, String, String, String, String, String, String, String, i64) = db
        .query_row(
            "SELECT account_ref,provider_order_id,symbol,side,quantity,order_type,time_in_force,digest,submissions FROM orders WHERE client_id=?1",
            [client_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                ))
            },
        )
        .map_err(|_| "controlled_order_not_found")?;
    let response = PluginResponse {
        schema_version: "1".into(),
        request_id: request.request_id,
        status: ResponseStatus::Succeeded,
        response_schema: "schema://tradeassembly.f2-controlled-broker/order@1".into(),
        payload: json!({"accountRef":account_ref,"clientOrderId":client_id,"providerOrderId":provider_order_id,"symbol":symbol,"side":side,"quantity":quantity,"orderType":order_type,"timeInForce":time_in_force,"status":"accepted","intentDigest":stored,"submissionCount":submissions}),
        provider_outcome: ProviderOutcome {
            code: "ok".into(),
            provider_request_id: None,
            provider_reference: Some(client_id.into()),
        },
        reconciliation: Reconciliation::Reconciled,
        evidence_references: vec![],
        redacted_diagnostics: vec![],
    };
    write_response(io::stdout().lock(), &response, DEFAULT_MAX_ENVELOPE_BYTES)
        .map_err(|_| "controlled_response_write_failed")
}
fn main() {
    if let Err(code) = run() {
        eprintln!("{code}");
        std::process::exit(2);
    }
}
