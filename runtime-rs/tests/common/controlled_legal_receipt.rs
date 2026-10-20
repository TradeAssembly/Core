//! Disposable signed legal evidence for the isolated Live controlled-broker test.
//! This is test code only. No user acknowledgement or production Hub key is used.

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::{DateTime, Duration, TimeZone, Utc};
use ed25519_dalek::{Signer as _, SigningKey};
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::Path;
use tradeassembly_runtime::local_owner_identity::LocalOwnerIdentity;
use tradeassembly_runtime::ports::{
    LEGAL_CANONICALIZATION_PROFILE, LEGAL_RECEIPT_EXPORT_SCHEMA, LEGAL_RECEIPT_SCHEMA,
    LEGAL_SIGNING_ALGORITHM,
};

const KEY_ID: &str = "f2-controlled-legal-test-key";
pub const RECEIPT_REF: &str = "receipt_stdio_controlled_live";

#[derive(Serialize)]
struct DocumentKey {
    document_id: String,
    version: String,
    locale: String,
}

#[derive(Serialize)]
struct DocumentVersion {
    key: DocumentKey,
    document_type: String,
    body_hash: String,
    published_at: DateTime<Utc>,
    retired_at: Option<DateTime<Utc>>,
    material_change: bool,
    requires_reacceptance: bool,
}

#[derive(Serialize)]
struct PublishedDocument {
    metadata: DocumentVersion,
    body: String,
    hub_signing_key_id: String,
    signing_algorithm: String,
    hub_signature: String,
}

#[derive(Serialize)]
struct PublishedDocumentPayload<'a> {
    metadata: &'a DocumentVersion,
    body: &'a str,
}

#[derive(Serialize)]
struct IdentityAssurance {
    acr: Option<String>,
    amr: Vec<String>,
    auth_time: DateTime<Utc>,
    verified_claim_refs: Vec<String>,
}

#[derive(Serialize)]
struct Acknowledgement {
    schema_version: String,
    acknowledgement_id: String,
    escrow_receipt_id: String,
    document_id: String,
    document_version: String,
    document_locale: String,
    document_hash: String,
    acknowledgement_type: String,
    user_id: String,
    identity_issuer: String,
    identity_subject: String,
    identity_assurance: IdentityAssurance,
    org_id: Option<String>,
    actor_id: String,
    resource_ref: Option<String>,
    resource_version_refs: Vec<String>,
    environment: String,
    application_id: String,
    application_version: String,
    application_build_digest: String,
    rendered_content_hash: String,
    acceptance_statement_hash: String,
    acceptance_challenge_id: String,
    acceptance_request_hash: String,
    presented_at: DateTime<Utc>,
    accepted_at: DateTime<Utc>,
    session_strength: Option<String>,
    ui_surface_ref: String,
    evidence_bundle_id: Option<String>,
    expires_at: Option<DateTime<Utc>>,
    jurisdiction_profile: String,
    escrow_required_for: Vec<String>,
    escrow_confirmed_at: DateTime<Utc>,
    canonicalization_profile: String,
}

#[derive(Serialize)]
struct SignedAcknowledgement {
    acknowledgement: Acknowledgement,
    hub_signing_key_id: String,
    signing_algorithm: String,
    hub_signature: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Export {
    schema_version: String,
    receipt: SignedAcknowledgement,
    document: PublishedDocument,
    lifecycle_events: Vec<Value>,
    later_documents: Vec<Value>,
    checked_at: DateTime<Utc>,
    valid_until: DateTime<Utc>,
    hub_signing_key_id: String,
    signing_algorithm: String,
    hub_signature: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ExportPayload<'a> {
    schema_version: &'a str,
    receipt: &'a SignedAcknowledgement,
    document: &'a PublishedDocument,
    lifecycle_events: &'a [Value],
    later_documents: &'a [Value],
    checked_at: DateTime<Utc>,
    valid_until: DateTime<Utc>,
}

#[derive(Serialize)]
struct SignedPayload<'a, T: Serialize> {
    hub_signing_key_id: &'a str,
    signing_algorithm: &'a str,
    payload: &'a T,
}

fn signed_message<T: Serialize>(domain: &str, payload: &T) -> Vec<u8> {
    let signed = SignedPayload {
        hub_signing_key_id: KEY_ID,
        signing_algorithm: LEGAL_SIGNING_ALGORITHM,
        payload,
    };
    let bytes = serde_json::to_vec(&signed).unwrap();
    let mut message = Vec::with_capacity(domain.len() + bytes.len() + 32);
    message.extend_from_slice(b"TradeAssembly Hub\0");
    message.extend_from_slice(LEGAL_CANONICALIZATION_PROFILE.as_bytes());
    message.push(0);
    message.extend_from_slice(domain.as_bytes());
    message.push(0);
    message.extend_from_slice(&(bytes.len() as u64).to_be_bytes());
    message.extend_from_slice(&bytes);
    message
}

fn signature<T: Serialize>(signer: &SigningKey, domain: &str, payload: &T) -> String {
    URL_SAFE_NO_PAD.encode(signer.sign(&signed_message(domain, payload)).to_bytes())
}

fn digest(bytes: impl AsRef<[u8]>) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub fn write(root: &Path, config: &Value, owner: &LocalOwnerIdentity, now_ms: i64) {
    // Qualification fixtures bind to their explicit source checkout, like the packaged binary.
    let revision = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(".."))
        .output()
        .expect("fixture source revision");
    assert!(revision.status.success());
    tradeassembly_runtime::build_identity::install(
        std::str::from_utf8(&revision.stdout).unwrap().trim(),
    )
    .unwrap();
    let now = Utc.timestamp_millis_opt(now_ms).single().unwrap();
    let legal_root = root.join("legal");
    let receipt_root = legal_root.join("receipts");
    std::fs::create_dir_all(&receipt_root).unwrap();
    let signer = SigningKey::from_bytes(&[7_u8; 32]);
    std::fs::write(
        legal_root.join("trusted-keys.json"),
        serde_json::to_vec(&json!({
            "keys":[{"keyId":KEY_ID,
                "publicKeyBase64url":URL_SAFE_NO_PAD.encode(signer.verifying_key().as_bytes())}]
        }))
        .unwrap(),
    )
    .unwrap();
    std::fs::write(
        legal_root.join("policy.json"),
        serde_json::to_vec(&json!({
            "applicationId":"tradeassembly_studio",
            "documentId":"studio-live-disclosure",
            "documentVersion":"2026-07",
            "documentLocale":"en-US",
            "acknowledgementType":"live_trading_disclosure",
            "jurisdictionProfile":"US",
            "escrowPurpose":"live_order_submission",
            "maxExportTtlSeconds":3600
        }))
        .unwrap(),
    )
    .unwrap();
    let body = "TradeAssembly controlled-fixture live disclosure".to_string();
    let metadata = DocumentVersion {
        key: DocumentKey {
            document_id: "studio-live-disclosure".into(),
            version: "2026-07".into(),
            locale: "en-US".into(),
        },
        document_type: "live_trading_disclosure".into(),
        body_hash: digest(body.as_bytes()),
        published_at: now - Duration::days(30),
        retired_at: None,
        material_change: false,
        requires_reacceptance: false,
    };
    let mut document = PublishedDocument {
        metadata,
        body,
        hub_signing_key_id: KEY_ID.into(),
        signing_algorithm: LEGAL_SIGNING_ALGORITHM.into(),
        hub_signature: String::new(),
    };
    document.hub_signature = signature(
        &signer,
        "legal-document",
        &PublishedDocumentPayload {
            metadata: &document.metadata,
            body: &document.body,
        },
    );
    let strategy_id = config["strategyId"].as_str().unwrap();
    let strategy_version_id = config["strategyVersionId"].as_str().unwrap();
    let spec_hash = config["strategySpecHash"].as_str().unwrap();
    assert!(spec_hash.starts_with("sha256:"));
    let mut receipt = SignedAcknowledgement {
        acknowledgement: Acknowledgement {
            schema_version: LEGAL_RECEIPT_SCHEMA.into(),
            acknowledgement_id: "ack_stdio_controlled_live".into(),
            escrow_receipt_id: RECEIPT_REF.into(),
            document_id: document.metadata.key.document_id.clone(),
            document_version: document.metadata.key.version.clone(),
            document_locale: document.metadata.key.locale.clone(),
            document_hash: document.metadata.body_hash.clone(),
            acknowledgement_type: "live_trading_disclosure".into(),
            user_id: "controlled-user".into(),
            identity_issuer: owner.issuer.clone(),
            identity_subject: owner.subject.clone(),
            identity_assurance: IdentityAssurance {
                acr: Some("urn:tradeassembly:loa:1".into()),
                amr: vec!["pwd".into(), "mfa".into()],
                auth_time: now - Duration::minutes(10),
                verified_claim_refs: vec!["controlled-claim".into()],
            },
            org_id: None,
            actor_id: "controlled-actor".into(),
            resource_ref: Some(format!("tradeassembly://strategies/{strategy_id}")),
            resource_version_refs: vec![
                format!("tradeassembly://strategy-versions/{strategy_version_id}"),
                spec_hash.to_ascii_lowercase(),
            ],
            environment: config["legalEnvironment"].as_str().unwrap().into(),
            application_id: "tradeassembly_studio".into(),
            application_version: env!("CARGO_PKG_VERSION").into(),
            application_build_digest: tradeassembly_runtime::build_identity::current()
                .unwrap()
                .application_digest(),
            rendered_content_hash: digest("rendered"),
            acceptance_statement_hash: digest("accepted"),
            acceptance_challenge_id: "controlled-challenge".into(),
            acceptance_request_hash: digest("request"),
            presented_at: now - Duration::minutes(5),
            accepted_at: now - Duration::minutes(4),
            session_strength: Some("mfa".into()),
            ui_surface_ref: "test://controlled-legal".into(),
            evidence_bundle_id: Some("controlled-evidence".into()),
            expires_at: Some(now + Duration::days(30)),
            jurisdiction_profile: "US".into(),
            escrow_required_for: vec!["live_order_submission".into()],
            escrow_confirmed_at: now - Duration::minutes(3),
            canonicalization_profile: LEGAL_CANONICALIZATION_PROFILE.into(),
        },
        hub_signing_key_id: KEY_ID.into(),
        signing_algorithm: LEGAL_SIGNING_ALGORITHM.into(),
        hub_signature: String::new(),
    };
    receipt.hub_signature = signature(&signer, "legal-acknowledgement", &receipt.acknowledgement);
    let mut export = Export {
        schema_version: LEGAL_RECEIPT_EXPORT_SCHEMA.into(),
        receipt,
        document,
        lifecycle_events: Vec::new(),
        later_documents: Vec::new(),
        checked_at: now,
        valid_until: now + Duration::minutes(30),
        hub_signing_key_id: KEY_ID.into(),
        signing_algorithm: LEGAL_SIGNING_ALGORITHM.into(),
        hub_signature: String::new(),
    };
    export.hub_signature = signature(
        &signer,
        "legal-receipt-export",
        &ExportPayload {
            schema_version: &export.schema_version,
            receipt: &export.receipt,
            document: &export.document,
            lifecycle_events: &export.lifecycle_events,
            later_documents: &export.later_documents,
            checked_at: export.checked_at,
            valid_until: export.valid_until,
        },
    );
    std::fs::write(
        receipt_root.join(format!("{RECEIPT_REF}.json")),
        serde_json::to_vec(&export).unwrap(),
    )
    .unwrap();
}
