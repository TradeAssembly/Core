// Opt-in integration against an independently built data calculation package.
// NativeTestSandbox is a process fixture, not production sandbox qualification.
use super::*;
use crate::{data_pipeline::canonical_sha256, ports::{AuthorityContext, IdempotencyKey, PluginPackageInstallRequest}};
use tradeassembly_plugin_sdk::{data_contract::*,data_evaluation::*};

#[test]
#[ignore = "requires DATA_CALCULATION_TEST_PACKAGE built for this host"]
fn real_data_calculation_package_installs_prepares_and_executes_without_credentials() {
    let path = std::path::PathBuf::from(std::env::var_os("DATA_CALCULATION_TEST_PACKAGE").expect("built data package required"));
    let archive = fs::read(&path).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("data-host.db").to_string_lossy().into_owned();
    let runtime = crate::adapters::local::test_runtime(&db);
    // Manifest bytes are read from the supplied local archive, then verified by
    // the actual installer against the supplied archive and canonical manifest.
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(archive.as_slice()));
    let manifest:Value = tar.entries().unwrap().filter_map(Result::ok).find(|entry| entry.path().unwrap().as_ref()==std::path::Path::new("manifest.json")).map(|mut entry| {let mut bytes=vec![];std::io::Read::read_to_end(&mut entry,&mut bytes).unwrap();serde_json::from_slice(&bytes).unwrap()}).expect("manifest required");
    let package=runtime.plugin_packages.install(&PluginPackageInstallRequest{source_type:"file".into(),locator:path.to_string_lossy().into(),package_sha256:format!("{:x}",Sha256::digest(&archive)),manifest_sha256:canonical_sha256(&manifest).unwrap(),offline:true}).unwrap();
    assert!(runtime.plugin_packages.data_contract(&package.package_sha256,"indicator.ema").unwrap().is_some());
    let ctx=SideEffectContext::new(AuthorityContext::local_cli(),IdempotencyKey::new("calculation-integration").unwrap());
    runtime.plugins.install_manifest(&package.plugin_ref,json!({"pluginRef":package.plugin_ref,"manifest":manifest,"manifestDigest":package.manifest_sha256,"package":{"packageSha256":package.package_sha256},"trust":{"level":"test"}}),&ctx).unwrap();
    runtime.plugins.put_instance("calculation",json!({"instanceRef":"calculation","pluginRef":package.plugin_ref,"enabled":true,"configuration":{},"activePackageSha256":package.package_sha256}),&ctx).unwrap();
    let schema=SchemaIdentity{schema_ref:"schema://tradeassembly/number-series@1".into(),sha256:canonical_sha256(&json!({"type":"array","items":{"type":"number"}})).unwrap()};
    let host=ExternalPluginHost::new(runtime.plugins.clone(),runtime.credentials.clone(),runtime.plugin_packages.clone(),runtime.clock.clone(),Arc::new(NativeTestSandbox));
    let mut states=std::collections::BTreeMap::new();
    let mut results=vec![];
    for (index,value) in [1,4,2,8].into_iter().enumerate() {
        let start=index as i64*86_400_000;
        let mut point=DataPoint{instrument_id:"BTC/USD".into(),source_event_id:format!("fixture:{index}"),provenance:BatchProvenance{content_sha256:String::new(),upstream_content_sha256:vec![],event_start_ms:start,event_end_ms:start+86_400_000,known_at_ms:start+86_400_000,arrived_at_ms:start+86_400_000,completed:true,source_revision:"fixture.1".into()},result:PointValue::Ready(json!(value))};
        point.provenance.content_sha256=point_sha256(&point).unwrap();
        let evaluation=DataEvaluationRequest{schema_version:EvaluationVersion::V1,plan_sha256:"test-plan".into(),node_id:"ema".into(),semantics_version:"tradeassembly.ema.sma-seed.v1".into(),instruments:vec!["BTC/USD".into()],window:EvaluationWindow{event_start_ms:start,event_end_ms:start+86_400_000,as_of_ms:start+86_400_000},parameters:json!({"period":3}),inputs:std::collections::BTreeMap::from([("source".into(),DataBatch{schema:schema.clone(),shape:DataShape::Series,points:vec![point]})]),prior_state:states,max_points:1};
        let mut request=operation_request();
        request.plugin_instance_ref="calculation".into();request.plugin_ref=package.plugin_ref.clone();request.manifest_fingerprint=package.manifest_sha256.clone();request.operation_id="indicator.ema".into();request.capability="indicator.ema".into();request.account_ref=None;request.purpose="data_calculation".into();request.input=serde_json::to_value(evaluation).unwrap();request.evaluation_tick_id=format!("frame-{index}");
        let mut context=ctx.clone();context.idempotency_key=IdempotencyKey::new(format!("calculation-frame-{index}")).unwrap();
        let result=host.invoke(&request,&context).unwrap();
        assert!(result.deterministic && result.replayable);
        let decoded:DataEvaluationResponse=serde_json::from_value(result.payload).unwrap();
        results.push(decoded.outputs["value"].points[0].result.clone());states=decoded.state;
    }
    assert_eq!(results[..2],[PointValue::WarmingUp,PointValue::WarmingUp]);
    let PointValue::Ready(value)=&results[3] else {panic!("EMA not ready")};
    assert!((value.as_f64().unwrap()-31.0/6.0).abs()<1e-12);
    assert!(runtime.plugins.get_instance("calculation").unwrap().unwrap().get("credentialRef").is_none());
}
