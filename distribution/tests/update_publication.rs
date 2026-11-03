// Copyright (c) 2026 OptionLab LLC. All rights reserved.
//! Fixture-key CLI contracts. No production key/Apple/network/publication.
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
    time::{SystemTime, UNIX_EPOCH},
};
use tradeassembly_distribution::updates::{Installation, UpdateDecision, UpdateProfile};
struct Fixture {
    _dir: tempfile::TempDir,
    proof: PathBuf,
    payload: PathBuf,
    output: PathBuf,
    digest: String,
}
fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let identity = json!({"schema_version":1,"product":"core-relay-customer","channel":"stable","version":"0.3.0","target":"aarch64-apple-darwin","state_compatibility":"customer-delivery-v1","archive_url":"https://github.com/TradeAssembly/Core/releases/download/v0.3.0/TradeAssembly-0.3.0-production-aarch64-apple-darwin.tar.gz","archive_sha256":"a".repeat(64),"bundle_manifest_sha256":"b".repeat(64),"components":{"core":"c".repeat(64),"relay":"d".repeat(64),"helper":"e".repeat(64)}});
        let proof = json!({"schema":"tradeassembly.customer-archive-notary-binding/v1","recordSha256":"1".repeat(64),"nativeInputProofSha256":"2".repeat(64),"coordinatorRevision":"3".repeat(40),"coreRevision":"4".repeat(40),"relayRevision":"5".repeat(40),"appleTeamId":"ABCDE12345","submissionId":"11111111-2222-3333-4444-555555555555","signedContainerSha256":"6".repeat(64),"stapledContainerSha256":"7".repeat(64),"notaryReceiptSha256":"8".repeat(64),"status":"Accepted","identity":identity,"published":false});
        let bytes = serde_json::to_vec_pretty(&proof).unwrap();
        let digest = sha(&bytes);
        let mut payload = identity;
        payload["sequence"] = json!(1);
        payload["issued_at"] = json!(now);
        payload["expires_at"] = json!(now + 180);
        let proof = root.join("proof.json");
        let payload_path = root.join("payload.json");
        fs::write(&proof, bytes).unwrap();
        fs::write(&payload_path, serde_json::to_vec_pretty(&payload).unwrap()).unwrap();
        Self {
            _dir: dir,
            proof,
            payload: payload_path,
            output: root.join("new-signed-metadata.json"),
            digest,
        }
    }
    fn command(&self, sign: bool, fixture_key: bool) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_tradeassembly-distribution"));
        command
            .arg(if sign {
                "update-publication-sign"
            } else {
                "update-publication-verify"
            })
            .arg("--proof")
            .arg(&self.proof)
            .arg("--expected-proof-sha256")
            .arg(&self.digest)
            .arg("--payload")
            .arg(&self.payload)
            .env_remove("TRADEASSEMBLY_BINARY_UPDATE_SIGNING_KEY");
        if sign {
            command.arg("--output").arg(&self.output);
        }
        if fixture_key {
            command.env(
                "TRADEASSEMBLY_BINARY_UPDATE_SIGNING_KEY",
                STANDARD.encode([7u8; 32]),
            );
        }
        command.output().unwrap()
    }
}
#[test]
fn verified_cli_signer_uses_fixture_key_and_never_overwrites_output() {
    let f = Fixture::new();
    assert!(f.command(false, false).status.success());
    let result = f.command(true, true);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let envelope = fs::read(&f.output).unwrap();
    let installed = Installation {
        profile: UpdateProfile::Stable {},
        product: "core-relay-customer".into(),
        version: "0.2.0".into(),
        target: "aarch64-apple-darwin".into(),
        state_compatibility: "customer-delivery-v1".into(),
        accepted_sequence: 0,
        archive_sha256: "f".repeat(64),
        bundle_manifest_sha256: "f".repeat(64),
    };
    let key = ed25519_dalek::SigningKey::from_bytes(&[7; 32]);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    assert!(matches!(
        tradeassembly_distribution::updates::plan(
            &installed,
            &envelope,
            &key.verifying_key().to_bytes(),
            now
        )
        .unwrap(),
        UpdateDecision::Stage { .. }
    ));
    let signed: Value = serde_json::from_slice(&envelope).unwrap();
    assert_eq!(
        STANDARD
            .decode(signed["payload"].as_str().unwrap())
            .unwrap(),
        fs::read(&f.payload).unwrap()
    );
    let retry = f.command(true, false);
    assert!(!retry.status.success());
    assert!(String::from_utf8_lossy(&retry.stderr).contains("publication_output_exists"));
    assert_eq!(fs::read(&f.output).unwrap(), envelope);
}
#[test]
fn missing_proof_and_payload_substitution_fail_before_key_access_or_output() {
    let f = Fixture::new();
    fs::remove_file(&f.proof).unwrap();
    let result = f.command(true, false);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("publication_input_unavailable"));
    assert!(!f.output.exists());
    let f = Fixture::new();
    let mut payload: Value = serde_json::from_slice(&fs::read(&f.payload).unwrap()).unwrap();
    payload["components"]["relay"] = json!("f".repeat(64));
    fs::write(&f.payload, serde_json::to_vec(&payload).unwrap()).unwrap();
    let result = f.command(true, false);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr)
        .contains("publication_payload_identity_or_pins_changed"));
    assert!(!f.output.exists());
}
#[test]
fn uncertain_proof_never_reaches_the_key_loader() {
    let mut f = Fixture::new();
    let mut proof: Value = serde_json::from_slice(&fs::read(&f.proof).unwrap()).unwrap();
    proof["status"] = json!("In Progress");
    let bytes = serde_json::to_vec(&proof).unwrap();
    fs::write(&f.proof, &bytes).unwrap();
    f.digest = sha(&bytes);
    let result = f.command(true, false);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("publication_native_proof_unaccepted"));
    assert!(!f.output.exists());
}

#[test]
fn renewal_cli_is_keyless_and_only_prepares_the_same_release() {
    let f = Fixture::new();
    let mut original: Value = serde_json::from_slice(&fs::read(&f.payload).unwrap()).unwrap();
    original["issued_at"] = json!(original["issued_at"].as_u64().unwrap() - 2);
    fs::write(&f.payload, serde_json::to_vec(&original).unwrap()).unwrap();
    assert!(f.command(true, true).status.success());
    let prior = fs::read(&f.output).unwrap();
    let key = ed25519_dalek::SigningKey::from_bytes(&[7; 32]);
    let run = |digest: &str, extra: bool| {
        let mut c = Command::new(env!("CARGO_BIN_EXE_tradeassembly-distribution"));
        c.arg("update-renewal-plan")
            .arg("--proof")
            .arg(&f.proof)
            .arg("--expected-proof-sha256")
            .arg(&f.digest)
            .arg("--previous-manifest")
            .arg(&f.output)
            .arg("--expected-previous-sha256")
            .arg(digest)
            .arg("--public-key")
            .arg(STANDARD.encode(key.verifying_key().to_bytes()))
            .arg("--sequence")
            .arg("2")
            .env(
                "TRADEASSEMBLY_BINARY_UPDATE_SIGNING_KEY",
                "must-not-be-read",
            );
        if extra {
            c.arg("--unexpected");
        }
        c.output().unwrap()
    };
    let result = run(&sha(&prior), false);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let plan: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(plan["payload"]["sequence"], 2);
    assert_eq!(plan["payload"]["components"], original["components"]);
    assert_eq!(
        plan["payload"]["archive_sha256"],
        original["archive_sha256"]
    );
    assert_eq!(plan["signed"], false);
    assert_eq!(plan["published"], false);
    assert!(!run(&"a".repeat(64), false).status.success());
    assert!(!run(&sha(&prior), true).status.success());
    assert_eq!(fs::read(&f.output).unwrap(), prior);
}
