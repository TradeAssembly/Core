//! Authored local-only executable fixture. No live providers or brokerage.
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    io::{BufRead, Read, Write},
    net::TcpStream,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
fn digest(v: &Value) -> String {
    Sha256::digest(serde_json_canonicalizer::to_vec(v).unwrap())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn main() {
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line).unwrap();
    let request: Value = serde_json::from_str(&line).unwrap();
    eprintln!("fixture operation {}: input parsed",request["metadata"]["operationId"]);
    let payload = &request["payload"]["payload"];
    let (result, schema) = match request["metadata"]["operationId"].as_str().unwrap() {
        "acquire" => {
            assert!(!request["ephemeralCredentialGrant"].is_null());
            let url = payload["config"]["dataApiUrl"].as_str().unwrap();
            let address = url.strip_prefix("http://127.0.0.1:").unwrap();
            let port = address.parse::<u16>().unwrap();
            let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            stream
                .set_write_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            write!(
                stream,
                "GET /observations HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"
            )
            .unwrap();
            let mut raw = Vec::new();
            stream.take(65537).read_to_end(&mut raw).unwrap();
            eprintln!("fixture acquire: HTTP read complete");
            assert!(raw.len() <= 65536);
            let response = String::from_utf8(raw).unwrap();
            let (_, body) = response.split_once("\r\n\r\n").unwrap();
            let mut snapshot: Value = serde_json::from_str(body).unwrap();
            if payload["gap"] == true {
                snapshot["values"].as_array_mut().unwrap().pop();
            }
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis() as i64;
            snapshot["observedAtMs"] =
                json!(now + if payload["future"] == true { 60000 } else { 0 });
            let mut result = json!({"digest":digest(&snapshot),"snapshot":snapshot});
            eprintln!("fixture acquire: digest complete");
            if payload["headers"] == true {
                result["headers"] = json!({"NonSecretFixture":"must-not-persist"});
            }
            (result, "schema://capture-fixture/response@1")
        }
        "chain" | "calendar" => {
            assert!(!request["ephemeralCredentialGrant"].is_null());
            let address = payload["config"]["dataApiUrl"]
                .as_str()
                .unwrap()
                .strip_prefix("http://127.0.0.1:")
                .unwrap();
            let mut stream =
                TcpStream::connect(("127.0.0.1", address.parse::<u16>().unwrap())).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            write!(
                stream,
                "GET /options HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"
            )
            .unwrap();
            let mut response = String::new();
            stream.take(65537).read_to_string(&mut response).unwrap();
            eprintln!("fixture calendar/chain: HTTP read complete");
            assert!(response.len() <= 65536);
            let (_, body) = response.split_once("\r\n\r\n").unwrap();
            (
                serde_json::from_str(body).unwrap(),
                if request["metadata"]["operationId"]=="calendar" {"schema://tradeassembly/session-calendar@1"}else{"schema://capture-fixture/option-chain@1"},
            )
        }
        "replay" => {
            assert!(request["ephemeralCredentialGrant"].is_null());
            let snapshot = &payload["parameters"]["snapshot"];
            assert_eq!(
                digest(snapshot),
                payload["parameters"]["digest"].as_str().unwrap()
            );
            let mut points = vec![];
            for (index, value) in snapshot["values"].as_array().unwrap().iter().enumerate() {
                let start = snapshot["start"].as_i64().unwrap()
                    + index as i64 * snapshot["interval"].as_i64().unwrap();
                let end = start + snapshot["interval"].as_i64().unwrap();
                if start < payload["window"]["eventStartMs"].as_i64().unwrap()
                    || end > payload["window"]["eventEndMs"].as_i64().unwrap()
                {
                    continue;
                }
                let mut point = json!({"instrumentId":snapshot["instrument"],"sourceEventId":format!("fixture:{end}"),"result":{"state":"ready","value":value}});
                let mut proof = point.clone();
                for (k, v) in [
                    ("eventStartMs", json!(start)),
                    ("eventEndMs", json!(end)),
                    ("knownAtMs", snapshot["observedAtMs"].clone()),
                    ("completed", json!(true)),
                    ("sourceRevision", json!("fixture.capture.1")),
                    ("upstreamContentSha256", json!([])),
                ] {
                    proof[k] = v;
                }
                let mut provenance = json!({});
                for key in [
                    "eventStartMs",
                    "eventEndMs",
                    "knownAtMs",
                    "completed",
                    "sourceRevision",
                    "upstreamContentSha256",
                ] {
                    provenance[key] = proof[key].clone();
                }
                provenance["arrivedAtMs"] = snapshot["observedAtMs"].clone();
                provenance["contentSha256"] = json!(digest(&proof));
                point["provenance"] = provenance;
                points.push(point);
            }
            assert!(points.len() as u64 <= payload["maxPoints"].as_u64().unwrap());
            let schema = json!({"type":"array","items":{"type":"number"}});
            (
                json!({"schemaVersion":"tradeassembly.data_evaluation.v1","state":{},"outputs":{"value":{"schema":{"schemaRef":"schema://test/series@1","sha256":digest(&schema)},"shape":"series","points":points}}}),
                "schema://tradeassembly/data-evaluation-response@1",
            )
        }
        "screen" => {
            // Toy selection operator lives in the executable fixture, never in
            // Core. This proves plumbing, not a released financial screener.
            assert!(request["ephemeralCredentialGrant"].is_null());
            assert_eq!(payload["schemaVersion"], "tradeassembly.instrument_screen_evaluation.v1");
            let input=payload["candidateInput"].as_str().unwrap();
            let rows=payload["inputs"][input]["payload"].pointer(payload["candidateRecordsPointer"].as_str().unwrap()).unwrap().as_array().unwrap();
            let parameters=&payload["parameters"];
            let mut eligible=vec![];let mut rejections=vec![];
            for row in rows {
                let id=row["id"].as_str().unwrap();
                let reason=if row["dte"].as_i64().unwrap()<parameters["minDte"].as_i64().unwrap() || row["dte"].as_i64().unwrap()>parameters["maxDte"].as_i64().unwrap() {
                    Some("expiry_out_of_range")
                } else if row["oi"].is_null() {Some("missing_open_interest")}
                else if row["oiAgeDays"].is_null() {Some("missing_open_interest_date")}
                else if row["oiAgeDays"].as_i64().unwrap()>parameters["maxOiAgeDays"].as_i64().unwrap() {Some("stale_open_interest")}
                else {None};
                if let Some(reason)=reason {rejections.push(json!({"instrumentId":id,"reasonCodes":[reason]}));}
                else {eligible.push(row);}
            }
            eligible.sort_by(|a,b|b["oi"].as_i64().unwrap().cmp(&a["oi"].as_i64().unwrap()).then_with(||a["id"].as_str().unwrap().cmp(b["id"].as_str().unwrap())));
            let limit=payload["topN"].as_u64().unwrap() as usize;
            let mut members=vec![];
            for (i,row) in eligible.iter().enumerate() {
                if i<limit {members.push(json!({"instrumentId":row["id"],"assetClass":"equity","instrumentFamily":"equity","rank":i+1,"contextInstruments":["SPY"]}));}
                else {rejections.push(json!({"instrumentId":row["id"],"reasonCodes":["below_rank_cutoff"]}));}
            }
            if parameters["fault"]=="omit_rejections" {rejections.clear();}
            if parameters["fault"]=="undeclared_context" {members[0]["contextInstruments"]=json!(["UNDECLARED"]);}
            if parameters["fault"]=="foreign_member" {members[0]["instrumentId"]=json!("FOREIGN");}
            let status=if members.is_empty() {"complete_empty"} else {"complete_nonempty"};
            (json!({"schemaVersion":"tradeassembly.instrument_screen_result.v1","status":status,"members":members,"rejections":rejections,"issues":[]}),"schema://tradeassembly/instrument-screen-result@1")
        }
        "derive" => {
            assert!(request["ephemeralCredentialGrant"].is_null());
            (
                json!({"schemaVersion":"tradeassembly.data_evaluation.v1","outputs":{"value":payload["inputs"]["source"]},"state":{}}),
                "schema://tradeassembly/data-evaluation-response@1",
            )
        }
        _ => panic!("unsupported fixture operation"),
    };
    eprintln!("fixture: response ready");
    println!(
        "{}",
        json!({"schemaVersion":"1","requestId":request["requestId"],"status":"succeeded","responseSchema":schema,"payload":result,"providerOutcome":{"code":"fixture_observed"},"reconciliation":"not_required"})
    );
    eprintln!("fixture: exit");
}
