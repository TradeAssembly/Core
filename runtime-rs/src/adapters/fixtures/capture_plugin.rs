//! Authored local-only executable fixture. No live providers or brokerage.
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    io::{BufRead, Read, Write},
    net::TcpStream,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
fn digest(v: &Value) -> String {
    Sha256::digest(serde_json_canonicalizer::pipe(&serde_json::to_string(v).unwrap()).unwrap().as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

// Test-only HTTP broker: the plugin has no filesystem state/permissions.
fn synthetic_broker_http(request: &Value, payload: &Value) -> Value {
    let port = payload["config"]["dataApiUrl"].as_str().unwrap()
        .strip_prefix("http://127.0.0.1:").unwrap().parse::<u16>().unwrap();
    let operation=request["metadata"]["operationId"].as_str().unwrap();
    let route=match operation{"broker.paper_order_submit_v2"=>"submit","broker.order_lookup.paper_v2"=>"lookup","account.health"=>"health","broker.cash_settlement.paper_v1"=>"settlement","broker.close_settlement.paper_v1"=>"close-settlement","execution_account"=>"account","execution_metadata"=>"metadata","execution_quote"=>"quote","management_v6_calendar"=>"calendar",_=>panic!("closed synthetic HTTP operation required")};
    let submit=route=="submit";
    let mut body = payload.clone(); body.as_object_mut().unwrap().remove("config");
    if submit { tradeassembly_plugin_sdk::broker_v2::decode_submit_json(&serde_json::to_vec(&body).unwrap()).unwrap(); }
    let bytes=serde_json::to_vec(&body).unwrap(); assert!(bytes.len()<=32768);
    let mut stream=TcpStream::connect(("127.0.0.1",port)).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(25))).unwrap();
    stream.set_write_timeout(Some(Duration::from_secs(2))).unwrap();
    write!(stream,"POST /{} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",route,bytes.len()).unwrap();
    stream.write_all(&bytes).unwrap();let mut raw=Vec::new();stream.take(65537).read_to_end(&mut raw).unwrap();assert!(raw.len()<=65536);
    let response=String::from_utf8(raw).unwrap();let (headers,body)=response.split_once("\r\n\r\n").unwrap();assert!(headers.starts_with("HTTP/1.1 200 ")||headers.starts_with("HTTP/1.0 200 "));
    let value:Value=serde_json::from_str(body).unwrap();
    // Core owns independent SDK/schema/identity validation of this response.
    if route=="health"{assert_eq!(value["account"]["mode"],"paper");}
    else{assert_eq!(value["schemaVersion"],match route{"settlement"=>"tradeassembly.cash_settlement_observation.v1","close-settlement"=>"tradeassembly.close_settlement_observation.v1","account"=>"tradeassembly.execution_account_snapshot.v1","metadata"=>"tradeassembly.execution_instrument_metadata.v1","quote"=>"tradeassembly.execution_quote.v1","calendar"=>"tradeassembly.session_calendar.v1",_=>"tradeassembly.broker_order_observation.v2"});}value
}

fn main() {
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line).unwrap();
    let request: Value = serde_json::from_str(&line).unwrap();
    eprintln!("fixture operation {}: input parsed",request["metadata"]["operationId"]);
    let payload = &request["payload"]["payload"];
    let (result, schema) = match request["metadata"]["operationId"].as_str().unwrap() {
        "management_v6_calendar" => {
            assert_eq!(request["context"]["mode"],"paper");
            (synthetic_broker_http(&request,payload),tradeassembly_plugin_sdk::session_calendar::SESSION_CALENDAR_SCHEMA)
        }
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
            let declared_snapshot = &payload["parameters"]["snapshot"];
            let mut snapshot = declared_snapshot.clone();
            assert_eq!(
                digest(declared_snapshot),
                payload["parameters"]["digest"].as_str().unwrap()
            );
            // Explicit authored generic fixture source. No indicator formula in Core.
            if snapshot["fixtureDynamicParity"] == true {
                let end=payload["window"]["eventEndMs"].as_i64().unwrap();
                snapshot["start"]=json!(end-1);snapshot["interval"]=json!(1);
                snapshot["observedAtMs"]=json!(end);
                snapshot["values"]=json!([if end%2==0{100}else{99}]);
            }
            if snapshot["fixtureFallingDaily"] == true {
                let end=payload["window"]["eventEndMs"].as_i64().unwrap();
                let interval=86400000;
                let offset=(end-declared_snapshot["observedAtMs"].as_i64().unwrap()).div_euclid(interval);
                snapshot["start"]=json!(end-interval);snapshot["interval"]=json!(interval);
                snapshot["observedAtMs"]=json!(end);snapshot["values"]=json!([100-offset]);
            }
            let schema_ref=if snapshot["fixtureFallingDaily"]==true{"schema://tradeassembly/number-series@1"}else{"schema://test/series@1"};
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
                json!({"schemaVersion":"tradeassembly.data_evaluation.v1","state":{},"outputs":{"value":{"schema":{"schemaRef":schema_ref,"sha256":digest(&schema)},"shape":"series","points":points}}}),
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
        "execution_account" | "execution_metadata" | "execution_quote" => {
            if payload["config"]["dataApiUrl"].is_string(){
                assert_eq!(request["context"]["mode"],"paper");
                let schema=match request["metadata"]["operationId"].as_str().unwrap(){"execution_account"=>tradeassembly_plugin_sdk::execution_snapshot::ACCOUNT_SCHEMA,"execution_metadata"=>tradeassembly_plugin_sdk::execution_snapshot::METADATA_SCHEMA,_=>tradeassembly_plugin_sdk::execution_snapshot::QUOTE_SCHEMA};
                (synthetic_broker_http(&request,payload),schema)
            }else{
            // Entirely authored paper fixture, not a provider observation.
            let now=SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as i64;
            let av=|v:Value|json!({"status":"available","value":v});
            let time=json!({"observedAtMs":now,"knownAtMs":now,"basis":"provider_observed"});let currency=json!({"code":"USD","decimals":2});
            let managed=payload["fixturePhase"].as_str().is_some_and(|p|p.starts_with("managed_"));let price=if payload["fixturePhase"]=="managed_high"{"120"}else if managed{"90"}else{"99"};
            match request["metadata"]["operationId"].as_str().unwrap() {
                "execution_account"=>(json!({"schemaVersion":"tradeassembly.execution_account_snapshot.v1","accountRef":"account://fixture.execution.facts/fixture.paper.provider","providerAccountId":"fixture.paper.provider","mode":"paper","currency":currency,"time":time,"coverage":{"accountComplete":true,"positionsComplete":true,"openOrdersComplete":true},"cashBalance":av(json!(if managed{"60"}else{"100"})),"nonMarginableBuyingPower":av(json!(if managed{"60"}else{"100"})),"borrowingDebt":av(json!("0")),"grossExposure":av(json!(if managed{"40"}else{"0"})),"instrumentPermissions":[{"assetClass":"equity","instrumentFamily":"equity","actions":av(json!(["buy_to_open","sell_to_close"]))}],"positions":av(if managed{json!([{"instrumentId":"SPY","side":"long","quantity":"0.4","grossValue":av(json!("40"))}])}else{json!([])}),"openOrders":av(json!([]))}),"schema://tradeassembly/execution-account-snapshot@1"),
                "execution_metadata"=>(json!({"schemaVersion":"tradeassembly.execution_instrument_metadata.v1","instrumentId":"SPY","providerSymbol":"SPY","assetClass":"equity","instrumentFamily":"equity","time":time,"quantityUnit":av(json!("shares")),"multiplier":av(json!("1")),"quoteCurrency":av(currency),"minimumQuantity":av(json!("0.001")),"quantityIncrement":av(json!("0.001")),"priceIncrement":av(json!("0.01")),"tradable":av(json!(true)),"expiresAtMs":av(Value::Null),"standardDeliverable":av(json!(true))}),"schema://tradeassembly/execution-instrument-metadata@1"),
                _=>(json!({"schemaVersion":"tradeassembly.execution_quote.v1","instrumentId":"SPY","currency":currency,"time":time,"origin":"provider","sourceProfile":"fixture-quote.v1","indicative":av(json!(false)),"bid":av(json!(price)),"ask":av(json!(if managed{price}else{"100"}))}),"schema://tradeassembly/execution-quote@1"),
            }
            }
        }
        "account.health" | "broker.cash_settlement.paper_v1" => {
            if payload["config"]["dataApiUrl"].is_string(){
                assert_eq!(request["context"]["mode"],"paper");assert!(!request["ephemeralCredentialGrant"].is_null());
                (synthetic_broker_http(&request,payload),if request["metadata"]["operationId"]=="account.health"{"schema://fixture/account-health-response@1"}else{tradeassembly_plugin_sdk::cash_settlement::RESPONSE_SCHEMA})
            }else{
            // New explicit synthetic provider identity; old retained archives are unchanged.
            use std::fs::{self,OpenOptions};let root=std::path::Path::new(payload["config"]["fixtureStore"].as_str().unwrap());assert_eq!(fs::read_to_string(root.join("SYNTHETIC_BROKER_ONLY")).unwrap(),"synthetic only\n");assert_eq!(request["context"]["mode"],"paper");assert!(!request["ephemeralCredentialGrant"].is_null());
            let health=request["metadata"]["operationId"]=="account.health";let mut log=OpenOptions::new().create(true).append(true).open(root.join(if health{"health.log"}else{"settlement.log"})).unwrap();writeln!(log,"read").unwrap();log.sync_all().unwrap();
            if health{(json!({"account":{"id":"fixture.paper.provider","mode":"paper","status":"ACTIVE","tradingBlocked":false,"accountBlocked":false,"tradeSuspendedByUser":false}}),"schema://fixture/account-health-response@1")}
            else{let original:Value=serde_json::from_slice(&fs::read(root.join("accepted-request.json")).unwrap()).unwrap();assert_eq!(payload["clientOrderId"],original["order"]["clientOrderId"]);assert_eq!(payload["canonicalAccountSha256"],original["canonicalAccountSha256"]);let now=SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis()as i64;let av=|value:Value|json!({"status":"available","value":value});
            (json!({"schemaVersion":"tradeassembly.cash_settlement_observation.v1","canonicalAccountSha256":original["canonicalAccountSha256"],"order":{"schemaVersion":"tradeassembly.broker_order_observation.v2","canonicalAccountSha256":original["canonicalAccountSha256"],"order":original["order"],"providerOrderId":"fixture.broker.order.1","status":"filled","filledQuantity":av(original["order"]["quantity"].clone()),"cashSpent":av(json!("40")),"observedAtMs":now,"knownAtMs":now},"postingId":"synthetic.final.posting.1","postingSequence":2,"grossCash":"40","feeCash":"0","feesFinal":true,"checkpoint":{"ledgerEpoch":"synthetic.epoch.1","sequence":2,"account":{"schemaVersion":"tradeassembly.execution_account_snapshot.v1","accountRef":"account://fixture.broker.v2/fixture.paper.provider","providerAccountId":"fixture.paper.provider","mode":"paper","currency":{"code":"USD","decimals":2},"time":{"observedAtMs":now,"knownAtMs":now,"basis":"provider_observed"},"coverage":{"accountComplete":true,"positionsComplete":true,"openOrdersComplete":true},"cashBalance":av(json!("60")),"nonMarginableBuyingPower":av(json!("60")),"borrowingDebt":av(json!("0")),"grossExposure":av(json!("40")),"instrumentPermissions":[{"assetClass":"equity","instrumentFamily":"equity","actions":av(json!(["buy_to_open"]))}],"positions":av(json!([{"instrumentId":"SPY","side":"long","quantity":original["order"]["quantity"],"grossValue":av(json!("40"))}])),"openOrders":av(json!([]))}}}),tradeassembly_plugin_sdk::cash_settlement::RESPONSE_SCHEMA)}
            }
        }
        "broker.close_settlement.paper_v1" => {
            if payload["config"]["dataApiUrl"].is_string(){
                assert_eq!(request["context"]["mode"],"paper");assert!(!request["ephemeralCredentialGrant"].is_null());
                (synthetic_broker_http(&request,payload),tradeassembly_plugin_sdk::close_settlement::RESPONSE_SCHEMA)
            }else{
            use std::fs::{self,OpenOptions};let root=std::path::Path::new(payload["config"]["fixtureStore"].as_str().unwrap());assert_eq!(fs::read_to_string(root.join("SYNTHETIC_BROKER_ONLY")).unwrap(),"synthetic only\n");let original:Value=serde_json::from_slice(&fs::read(root.join("close-accepted-request.json")).unwrap()).unwrap();assert_eq!(payload["clientOrderId"],original["order"]["clientOrderId"]);assert_eq!(payload["canonicalAccountSha256"],original["canonicalAccountSha256"]);let mut log=OpenOptions::new().create(true).append(true).open(root.join("close-settlement.log")).unwrap();writeln!(log,"read").unwrap();log.sync_all().unwrap();
            let now=SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis()as i64;let av=|value:Value|json!({"status":"available","value":value});
            let mut document=json!({"schemaVersion":"tradeassembly.close_settlement_observation.v1","canonicalAccountSha256":original["canonicalAccountSha256"],"order":{"schemaVersion":"tradeassembly.broker_order_observation.v2","canonicalAccountSha256":original["canonicalAccountSha256"],"order":original["order"],"providerOrderId":"fixture.close.order.1","status":"filled","filledQuantity":av(original["order"]["quantity"].clone()),"cashSpent":av(json!("0")),"observedAtMs":now,"knownAtMs":now},"postingId":"synthetic.final.close.posting.1","postingSequence":3,"grossProceeds":"36","feeCash":"0","feesFinal":true,"checkpoint":{"ledgerEpoch":"synthetic.epoch.1","sequence":3,"account":{"schemaVersion":"tradeassembly.execution_account_snapshot.v1","accountRef":"account://fixture.broker.v2/fixture.paper.provider","providerAccountId":"fixture.paper.provider","mode":"paper","currency":{"code":"USD","decimals":2},"time":{"observedAtMs":now,"knownAtMs":now,"basis":"provider_observed"},"coverage":{"accountComplete":true,"positionsComplete":true,"openOrdersComplete":true},"cashBalance":av(json!("96")),"nonMarginableBuyingPower":av(json!("96")),"borrowingDebt":av(json!("0")),"grossExposure":av(json!("0")),"instrumentPermissions":[{"assetClass":"equity","instrumentFamily":"equity","actions":av(json!(["buy_to_open","sell_to_close"]))}],"positions":av(json!([])),"openOrders":av(json!([]))}}});
            if let Some(scenario)=payload["config"]["scenario"].as_str().filter(|v|v.starts_with("managed_plan_close_")){
                native_close_outcome_settlement(&mut document,root,scenario);
            }
            (document,tradeassembly_plugin_sdk::close_settlement::RESPONSE_SCHEMA)
            }
        }
        "broker.paper_order_submit_v2" | "broker.order_lookup.paper_v2" => {
            if payload["config"]["dataApiUrl"].is_string() {
                assert_eq!(request["context"]["mode"],"paper");assert!(!request["ephemeralCredentialGrant"].is_null());
                (synthetic_broker_http(&request,payload),tradeassembly_plugin_sdk::broker_v2::OBSERVATION_SCHEMA)
            } else {
            use std::fs::{self,OpenOptions};
            let config=&payload["config"];let root=std::path::Path::new(config["fixtureStore"].as_str().unwrap());assert_eq!(fs::read_to_string(root.join("SYNTHETIC_BROKER_ONLY")).unwrap(),"synthetic only\n");
            assert_eq!(request["context"]["mode"],"paper");assert!(!request["ephemeralCredentialGrant"].is_null());
            let submit=request["metadata"]["operationId"]=="broker.paper_order_submit_v2";
            let mut counts=OpenOptions::new().create(true).append(true).open(root.join("calls.log")).unwrap();writeln!(counts,"{}",if submit{"submit"}else{"lookup"}).unwrap();
            let mut clean=payload.clone();clean.as_object_mut().unwrap().remove("config");
            let closing=clean["order"]["positionAction"]=="sell_to_close"||clean["clientOrderId"].as_str().is_some_and(|s|s.starts_with("ta-close-"));let accepted=root.join(if closing{"close-accepted-request.json"}else{"accepted-request.json"});
            let original=if submit{tradeassembly_plugin_sdk::broker_v2::decode_submit_json(&serde_json::to_vec(&clean).unwrap()).unwrap();let bytes=serde_json::to_vec(&clean).unwrap();let mut file=OpenOptions::new().create_new(true).write(true).open(&accepted).unwrap();file.write_all(&bytes).unwrap();file.sync_all().unwrap();clean}else{let original:Value=serde_json::from_slice(&fs::read(&accepted).unwrap()).unwrap();assert_eq!(clean["clientOrderId"],original["order"]["clientOrderId"]);assert_eq!(clean["canonicalAccountSha256"],original["canonicalAccountSha256"]);original};
            if closing&&config["scenario"].as_str().is_some_and(|v|v.starts_with("managed_plan_close_")){
                (native_close_outcome_observation(root,config["scenario"].as_str().unwrap(),&original,submit),tradeassembly_plugin_sdk::broker_v2::OBSERVATION_SCHEMA)
            }else{
            if submit && config["scenario"]=="timeout_after_accept"{std::thread::sleep(Duration::from_secs(20));std::process::exit(0)}
            let status=if config["scenario"]=="reject"{"rejected"}else if submit{"open"}else{"filled"};
            let filled=if status=="filled"{original["order"]["quantity"].clone()}else{json!("0")};let cash=if status=="filled"&&!closing{json!("40")}else{json!("0")};let now=SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis()as i64;
            (json!({"schemaVersion":"tradeassembly.broker_order_observation.v2","canonicalAccountSha256":original["canonicalAccountSha256"],"order":original["order"],"providerOrderId":if status=="rejected"{Value::Null}else{json!(if closing{"fixture.close.order.1"}else{"fixture.broker.order.1"})},"status":status,"filledQuantity":{"status":"available","value":filled},"cashSpent":{"status":"available","value":cash},"observedAtMs":now,"knownAtMs":now}),tradeassembly_plugin_sdk::broker_v2::OBSERVATION_SCHEMA)
            }
            }
        }
        "plan_calculate" => {
            assert!(request["ephemeralCredentialGrant"].is_null());
            // Explicit quantity is the strategy fixture parameter. The host
            // still checks actual cash, units, increments and hard limits.
            let typed=tradeassembly_plugin_sdk::execution_calculation::decode_host_json(&serde_json::to_vec(payload).unwrap()).unwrap();
            (json!({"schemaVersion":"tradeassembly.execution_calculation_output.v1","quantity":typed.parameters["quantity"].as_str().unwrap(),"limitPrice":typed.quote.ask.required().unwrap().to_string(),"reasonCodes":["fixture_explicit_quantity"]}),"schema://tradeassembly/execution-calculation-output@1")
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

// Explicit synthetic provider scenarios. These author observations and actual
// final postings only; Core still owns order authority, lots and reservations.
fn native_close_outcome_observation(root:&std::path::Path,scenario:&str,original:&Value,submit:bool)->Value{
    let lookup=if submit{0}else{
        let path=root.join("close-outcome-lookups");let next=std::fs::read_to_string(&path).ok().map(|v|v.parse::<u64>().unwrap()).unwrap_or(0)+1;
        let mut file=std::fs::OpenOptions::new().create(true).truncate(true).write(true).open(path).unwrap();write!(file,"{next}").unwrap();file.sync_all().unwrap();next
    };
    if submit&&scenario=="managed_plan_close_timeout_filled"{
        // Acceptance was durably written by the caller before this timeout.
        std::thread::sleep(Duration::from_secs(20));std::process::exit(0);
    }
    let (status,filled,fee)=match scenario{
        "managed_plan_close_partial_cancel" if submit=>("partially_filled","0.1","0"),
        "managed_plan_close_partial_cancel" if lookup==1=>("partially_filled","0.2","0"),
        "managed_plan_close_partial_cancel"=>("cancelled","0.2","0.25"),
        "managed_plan_close_rejected"=>("rejected","0","0"),
        "managed_plan_close_timeout_filled"=>("filled","0.4","0.5"),
        _=>panic!("unsupported authored close outcome"),
    };
    let now=SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis()as i64;
    json!({"schemaVersion":"tradeassembly.broker_order_observation.v2","canonicalAccountSha256":original["canonicalAccountSha256"],"order":original["order"],"providerOrderId":"fixture.close.order.1","status":status,"filledQuantity":{"status":"available","value":filled},"cashSpent":{"status":"available","value":fee},"observedAtMs":now,"knownAtMs":now})
}
fn native_close_outcome_settlement(document:&mut Value,root:&std::path::Path,scenario:&str){
    let (status,filled,gross,fee,cash,remaining,exposure)=match scenario{
        "managed_plan_close_partial_cancel"=>("cancelled","0.2","18","0.25","77.75","0.2","20"),
        "managed_plan_close_rejected"=>("rejected","0","0","0.1","59.9","0.4","40"),
        "managed_plan_close_timeout_filled"=>("filled","0.4","36","0.5","95.5","0","0"),
        _=>panic!("unsupported authored close posting"),
    };
    document["order"]["status"]=json!(status);
    document["order"]["filledQuantity"]["value"]=json!(filled);
    document["order"]["cashSpent"]["value"]=json!(fee);
    document["grossProceeds"]=json!(gross);document["feeCash"]=json!(fee);
    let account=&mut document["checkpoint"]["account"];
    account["cashBalance"]["value"]=json!(cash);account["nonMarginableBuyingPower"]["value"]=json!(cash);
    account["grossExposure"]["value"]=json!(exposure);
    account["positions"]["value"]=if remaining=="0"{json!([])}else{json!([{"instrumentId":"SPY","side":"long","quantity":remaining,"grossValue":{"status":"available","value":exposure}}])};
    if scenario=="managed_plan_close_partial_cancel"{
        let lookups=std::fs::read_to_string(root.join("close-outcome-lookups")).ok().map(|v|v.parse::<u64>().unwrap()).unwrap_or(0);
        if lookups<2{
            // Explicit nonfinal provider evidence must fail the terminal-only
            // settlement schema; it cannot release cash or owned quantity.
            document["order"]["status"]=json!("partially_filled");document["feesFinal"]=json!(false);
        }
    }
}
