// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use crate::ports::{
    FailureMode, LegalReceiptExpectation, LegalReceiptFailure, LegalReceiptPort, PortDescriptor,
    PortKind, VerifiedLegalReceipt, VersionedPort, LEGAL_CANONICALIZATION_PROFILE,
    LEGAL_RECEIPT_EXPORT_SCHEMA, LEGAL_RECEIPT_SCHEMA, LEGAL_SIGNING_ALGORITHM,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::{DateTime, TimeZone, Utc};
use ed25519_dalek::{Signature, Verifier as _, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

const MAX_KEY_FILE_BYTES: u64 = 64 * 1024;
const MAX_EXPORT_BYTES: u64 = 4 * 1024 * 1024;
const MAX_CLOCK_SKEW_SECONDS: i64 = 300;

#[derive(Clone)]
pub struct FileLegalReceiptVerifier {
    receipt_root: PathBuf,
    trusted_keys_path: PathBuf,
    policy_path: PathBuf,
}

impl FileLegalReceiptVerifier {
    pub fn new(
        receipt_root: impl Into<PathBuf>,
        trusted_keys_path: impl Into<PathBuf>,
        policy_path: impl Into<PathBuf>,
    ) -> Self {
        Self {
            receipt_root: receipt_root.into(),
            trusted_keys_path: trusted_keys_path.into(),
            policy_path: policy_path.into(),
        }
    }

    fn load_export(
        &self,
        receipt_ref: &str,
    ) -> Result<(LegalReceiptExport, Vec<u8>), LegalReceiptFailure> {
        if receipt_ref.is_empty()
            || receipt_ref.len() > 128
            || !receipt_ref
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        {
            return Err(LegalReceiptFailure::Invalid);
        }
        let path = self.receipt_root.join(format!("{receipt_ref}.json"));
        let body = read_bounded(&path, MAX_EXPORT_BYTES, LegalReceiptFailure::Missing)?;
        let export = serde_json::from_slice(&body).map_err(|_| LegalReceiptFailure::Invalid)?;
        Ok((export, body))
    }

    fn load_keys(&self) -> Result<HashMap<String, VerifyingKey>, LegalReceiptFailure> {
        let body = read_bounded(
            &self.trusted_keys_path,
            MAX_KEY_FILE_BYTES,
            LegalReceiptFailure::Unavailable,
        )?;
        let key_file: TrustedKeyFile =
            serde_json::from_slice(&body).map_err(|_| LegalReceiptFailure::Unavailable)?;
        if key_file.keys.is_empty() || key_file.keys.len() > 64 {
            return Err(LegalReceiptFailure::Unavailable);
        }
        let mut keys = HashMap::new();
        for entry in key_file.keys {
            if entry.key_id.trim().is_empty() || keys.contains_key(&entry.key_id) {
                return Err(LegalReceiptFailure::Unavailable);
            }
            let bytes = URL_SAFE_NO_PAD
                .decode(entry.public_key_base64url)
                .map_err(|_| LegalReceiptFailure::Unavailable)?;
            let bytes: [u8; 32] = bytes
                .try_into()
                .map_err(|_| LegalReceiptFailure::Unavailable)?;
            let key =
                VerifyingKey::from_bytes(&bytes).map_err(|_| LegalReceiptFailure::Unavailable)?;
            keys.insert(entry.key_id, key);
        }
        Ok(keys)
    }

    fn load_policy(&self) -> Result<LegalReceiptPolicy, LegalReceiptFailure> {
        let body = read_bounded(
            &self.policy_path,
            MAX_KEY_FILE_BYTES,
            LegalReceiptFailure::Unavailable,
        )?;
        let policy: LegalReceiptPolicy =
            serde_json::from_slice(&body).map_err(|_| LegalReceiptFailure::Unavailable)?;
        if policy.application_id.trim().is_empty()
            || policy.document_id.trim().is_empty()
            || policy.document_version.trim().is_empty()
            || policy.document_locale.trim().is_empty()
            || policy.acknowledgement_type.trim().is_empty()
            || policy.jurisdiction_profile.trim().is_empty()
            || policy.escrow_purpose.trim().is_empty()
            || !(60..=86_400).contains(&policy.max_export_ttl_seconds)
        {
            return Err(LegalReceiptFailure::Unavailable);
        }
        Ok(policy)
    }
}

impl VersionedPort for FileLegalReceiptVerifier {
    fn descriptors(&self) -> Vec<PortDescriptor> {
        let mut descriptor = PortDescriptor::new(PortKind::Evidence, "file.hub-legal-receipts")
            .for_profiles(&["local", "self_hosted"])
            .with_capabilities(&["legal.receipt.verify", "legal.receipt.cached_export"]);
        descriptor.failure_mode = FailureMode::FailClosed;
        descriptor.redaction_rules = vec![
            "receipt_signatures_redacted".to_string(),
            "identity_subject_not_logged".to_string(),
        ];
        vec![descriptor]
    }
}

impl LegalReceiptPort for FileLegalReceiptVerifier {
    fn verify(
        &self,
        expectation: &LegalReceiptExpectation,
        trusted_now_ms: i64,
    ) -> Result<VerifiedLegalReceipt, LegalReceiptFailure> {
        let (export, raw_export) = self.load_export(&expectation.receipt_ref)?;
        let keys = self.load_keys()?;
        let policy = self.load_policy()?;
        let now = Utc
            .timestamp_millis_opt(trusted_now_ms)
            .single()
            .ok_or(LegalReceiptFailure::Invalid)?;

        if export.schema_version != LEGAL_RECEIPT_EXPORT_SCHEMA
            || export.signing_algorithm != LEGAL_SIGNING_ALGORITHM
            || export.receipt.acknowledgement.escrow_receipt_id != expectation.receipt_ref
            || export.valid_until <= export.checked_at
            || export.checked_at > now + chrono::Duration::seconds(MAX_CLOCK_SKEW_SECONDS)
            || export.valid_until < now
            || (export.valid_until - export.checked_at).num_seconds()
                > policy.max_export_ttl_seconds
        {
            return Err(if export.valid_until < now {
                LegalReceiptFailure::Stale
            } else {
                LegalReceiptFailure::Invalid
            });
        }

        verify_export_signature(&export, &keys)?;
        verify_receipt_signature(&export.receipt, &keys)?;
        verify_document_signature(&export.document, &keys)?;
        verify_lifecycle(&export.receipt, &export.lifecycle_events, &keys)?;

        let acknowledgement = &export.receipt.acknowledgement;
        let document = &export.document.metadata;
        if document.key.document_id != acknowledgement.document_id
            || document.key.version != acknowledgement.document_version
            || document.key.locale != acknowledgement.document_locale
            || document.body_hash != acknowledgement.document_hash
            || acknowledgement.identity_issuer != expectation.identity_issuer
            || acknowledgement.identity_subject != expectation.identity_subject
            || acknowledgement.resource_ref.as_deref() != Some(&expectation.resource_ref)
            || !expectation
                .resource_version_refs
                .iter()
                .all(|reference| acknowledgement.resource_version_refs.contains(reference))
            || acknowledgement.environment != expectation.environment
            || acknowledgement.application_id != policy.application_id
            || acknowledgement.application_version != env!("CARGO_PKG_VERSION")
            || acknowledgement.application_build_digest != current_build_digest()
            || acknowledgement.document_id != policy.document_id
            || acknowledgement.document_version != policy.document_version
            || acknowledgement.document_locale != policy.document_locale
            || acknowledgement.acknowledgement_type != policy.acknowledgement_type
            || acknowledgement.jurisdiction_profile != policy.jurisdiction_profile
            || !acknowledgement
                .escrow_required_for
                .iter()
                .any(|purpose| purpose == &policy.escrow_purpose)
            || acknowledgement.accepted_at > export.checked_at
            || acknowledgement.escrow_confirmed_at > export.checked_at
        {
            return Err(LegalReceiptFailure::BindingMismatch);
        }

        if acknowledgement
            .expires_at
            .is_some_and(|expires_at| expires_at <= now)
        {
            return Err(LegalReceiptFailure::Expired);
        }
        if export
            .lifecycle_events
            .iter()
            .any(|event| event.event.effective_at <= now)
        {
            return Err(LegalReceiptFailure::Revoked);
        }
        if export.later_documents.iter().any(|candidate| {
            candidate.key.document_id == document.key.document_id
                && candidate.key.locale == document.key.locale
                && candidate.published_at <= now
                && candidate.material_change
                && candidate.requires_reacceptance
        }) {
            return Err(LegalReceiptFailure::Superseded);
        }

        Ok(VerifiedLegalReceipt {
            receipt_ref: expectation.receipt_ref.clone(),
            acknowledgement_id: acknowledgement.acknowledgement_id.clone(),
            document_id: acknowledgement.document_id.clone(),
            document_version: acknowledgement.document_version.clone(),
            document_locale: acknowledgement.document_locale.clone(),
            identity_issuer: acknowledgement.identity_issuer.clone(),
            identity_subject: acknowledgement.identity_subject.clone(),
            accepted_at: acknowledgement.accepted_at.to_rfc3339(),
            valid_until: export.valid_until.to_rfc3339(),
            export_digest: sha256_hex(&raw_export),
        })
    }
}

fn read_bounded(
    path: &Path,
    maximum: u64,
    missing: LegalReceiptFailure,
) -> Result<Vec<u8>, LegalReceiptFailure> {
    let metadata = fs::metadata(path).map_err(|_| missing)?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > maximum {
        return Err(LegalReceiptFailure::Invalid);
    }
    fs::read(path).map_err(|_| LegalReceiptFailure::Unavailable)
}

fn verify_export_signature(
    export: &LegalReceiptExport,
    keys: &HashMap<String, VerifyingKey>,
) -> Result<(), LegalReceiptFailure> {
    let payload = LegalReceiptExportPayload::from(export);
    verify_signed_payload(
        "legal-receipt-export",
        &export.hub_signing_key_id,
        &export.signing_algorithm,
        &export.hub_signature,
        &payload,
        keys,
    )
}

fn verify_receipt_signature(
    receipt: &SignedLegalAcknowledgement,
    keys: &HashMap<String, VerifyingKey>,
) -> Result<(), LegalReceiptFailure> {
    if receipt.signing_algorithm != LEGAL_SIGNING_ALGORITHM
        || receipt.acknowledgement.schema_version != LEGAL_RECEIPT_SCHEMA
        || receipt.acknowledgement.canonicalization_profile != LEGAL_CANONICALIZATION_PROFILE
        || !valid_digest(&receipt.acknowledgement.acceptance_request_hash)
    {
        return Err(LegalReceiptFailure::Invalid);
    }
    verify_signed_payload(
        "legal-acknowledgement",
        &receipt.hub_signing_key_id,
        &receipt.signing_algorithm,
        &receipt.hub_signature,
        &receipt.acknowledgement,
        keys,
    )
}

fn verify_document_signature(
    document: &PublishedLegalDocument,
    keys: &HashMap<String, VerifyingKey>,
) -> Result<(), LegalReceiptFailure> {
    if document.signing_algorithm != LEGAL_SIGNING_ALGORITHM
        || sha256_hex(document.body.as_bytes()) != document.metadata.body_hash
    {
        return Err(LegalReceiptFailure::Invalid);
    }
    verify_signed_payload(
        "legal-document",
        &document.hub_signing_key_id,
        &document.signing_algorithm,
        &document.hub_signature,
        &PublishedDocumentPayload {
            metadata: &document.metadata,
            body: &document.body,
        },
        keys,
    )
}

fn verify_lifecycle(
    receipt: &SignedLegalAcknowledgement,
    events: &[SignedLifecycleEvent],
    keys: &HashMap<String, VerifyingKey>,
) -> Result<(), LegalReceiptFailure> {
    let mut previous = None;
    for (index, signed) in events.iter().enumerate() {
        if signed.signing_algorithm != LEGAL_SIGNING_ALGORITHM
            || signed.event.escrow_receipt_id != receipt.acknowledgement.escrow_receipt_id
            || signed.event.sequence != index as u64 + 1
            || signed.event.previous_event_hash != previous
            || lifecycle_event_hash(&signed.event)? != signed.event.event_hash
        {
            return Err(LegalReceiptFailure::Invalid);
        }
        verify_signed_payload(
            "legal-lifecycle-event",
            &signed.hub_signing_key_id,
            &signed.signing_algorithm,
            &signed.hub_signature,
            &signed.event,
            keys,
        )?;
        previous = Some(signed.event.event_hash.clone());
    }
    Ok(())
}

fn verify_signed_payload<T: Serialize>(
    domain: &str,
    key_id: &str,
    algorithm: &str,
    encoded_signature: &str,
    payload: &T,
    keys: &HashMap<String, VerifyingKey>,
) -> Result<(), LegalReceiptFailure> {
    let key = keys.get(key_id).ok_or(LegalReceiptFailure::Invalid)?;
    let signature_bytes = URL_SAFE_NO_PAD
        .decode(encoded_signature)
        .map_err(|_| LegalReceiptFailure::Invalid)?;
    let signature =
        Signature::from_slice(&signature_bytes).map_err(|_| LegalReceiptFailure::Invalid)?;
    key.verify(
        &canonical_signed_message(domain, key_id, algorithm, payload)?,
        &signature,
    )
    .map_err(|_| LegalReceiptFailure::Invalid)
}

fn canonical_message<T: Serialize>(
    domain: &str,
    value: &T,
) -> Result<Vec<u8>, LegalReceiptFailure> {
    let payload = serde_json::to_vec(value).map_err(|_| LegalReceiptFailure::Invalid)?;
    let mut message = Vec::with_capacity(domain.len() + payload.len() + 32);
    message.extend_from_slice(b"TradeAssembly Hub\0");
    message.extend_from_slice(LEGAL_CANONICALIZATION_PROFILE.as_bytes());
    message.push(0);
    message.extend_from_slice(domain.as_bytes());
    message.push(0);
    message.extend_from_slice(&(payload.len() as u64).to_be_bytes());
    message.extend_from_slice(&payload);
    Ok(message)
}

#[derive(Serialize)]
struct SignedPayload<'a, T: Serialize> {
    hub_signing_key_id: &'a str,
    signing_algorithm: &'a str,
    payload: &'a T,
}

fn canonical_signed_message<T: Serialize>(
    domain: &str,
    key_id: &str,
    algorithm: &str,
    payload: &T,
) -> Result<Vec<u8>, LegalReceiptFailure> {
    canonical_message(
        domain,
        &SignedPayload {
            hub_signing_key_id: key_id,
            signing_algorithm: algorithm,
            payload,
        },
    )
}

fn lifecycle_event_hash(event: &ReceiptLifecycleEvent) -> Result<String, LegalReceiptFailure> {
    let mut unsigned = event.clone();
    unsigned.event_hash.clear();
    Ok(sha256_hex(&canonical_message(
        "legal-lifecycle-event-hash",
        &unsigned,
    )?))
}

fn current_build_digest() -> String {
    sha256_hex(env!("TRADEASSEMBLY_CORE_REVISION").as_bytes())
}

fn valid_digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn sha256_hex(value: impl AsRef<[u8]>) -> String {
    format!("{:x}", Sha256::digest(value.as_ref()))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TrustedKeyFile {
    keys: Vec<TrustedKey>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TrustedKey {
    key_id: String,
    public_key_base64url: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LegalReceiptPolicy {
    application_id: String,
    document_id: String,
    document_version: String,
    document_locale: String,
    acknowledgement_type: String,
    jurisdiction_profile: String,
    escrow_purpose: String,
    max_export_ttl_seconds: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct LegalDocumentKey {
    document_id: String,
    version: String,
    locale: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct LegalDocumentVersion {
    key: LegalDocumentKey,
    document_type: String,
    body_hash: String,
    published_at: DateTime<Utc>,
    retired_at: Option<DateTime<Utc>>,
    material_change: bool,
    requires_reacceptance: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct PublishedLegalDocument {
    metadata: LegalDocumentVersion,
    body: String,
    hub_signing_key_id: String,
    signing_algorithm: String,
    hub_signature: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct IdentityAssurance {
    acr: Option<String>,
    amr: Vec<String>,
    auth_time: DateTime<Utc>,
    verified_claim_refs: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct LegalAcknowledgement {
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

#[derive(Clone, Debug, Deserialize, Serialize)]
struct SignedLegalAcknowledgement {
    acknowledgement: LegalAcknowledgement,
    hub_signing_key_id: String,
    signing_algorithm: String,
    hub_signature: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum LifecycleEventKind {
    Revoked,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct ReceiptLifecycleEvent {
    event_id: String,
    escrow_receipt_id: String,
    sequence: u64,
    previous_event_hash: Option<String>,
    kind: LifecycleEventKind,
    effective_at: DateTime<Utc>,
    reason_code: String,
    event_hash: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct SignedLifecycleEvent {
    event: ReceiptLifecycleEvent,
    hub_signing_key_id: String,
    signing_algorithm: String,
    hub_signature: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LegalReceiptExport {
    schema_version: String,
    receipt: SignedLegalAcknowledgement,
    document: PublishedLegalDocument,
    lifecycle_events: Vec<SignedLifecycleEvent>,
    later_documents: Vec<LegalDocumentVersion>,
    checked_at: DateTime<Utc>,
    valid_until: DateTime<Utc>,
    hub_signing_key_id: String,
    signing_algorithm: String,
    hub_signature: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LegalReceiptExportPayload<'a> {
    schema_version: &'a str,
    receipt: &'a SignedLegalAcknowledgement,
    document: &'a PublishedLegalDocument,
    lifecycle_events: &'a [SignedLifecycleEvent],
    later_documents: &'a [LegalDocumentVersion],
    checked_at: DateTime<Utc>,
    valid_until: DateTime<Utc>,
}

impl<'a> From<&'a LegalReceiptExport> for LegalReceiptExportPayload<'a> {
    fn from(export: &'a LegalReceiptExport) -> Self {
        Self {
            schema_version: &export.schema_version,
            receipt: &export.receipt,
            document: &export.document,
            lifecycle_events: &export.lifecycle_events,
            later_documents: &export.later_documents,
            checked_at: export.checked_at,
            valid_until: export.valid_until,
        }
    }
}

#[derive(Serialize)]
struct PublishedDocumentPayload<'a> {
    metadata: &'a LegalDocumentVersion,
    body: &'a str,
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, TimeZone};
    use ed25519_dalek::{Signer as _, SigningKey};
    use tempfile::TempDir;

    const KEY_ID: &str = "hub-test-key";
    const RECEIPT_REF: &str = "receipt_test_123";

    struct Fixture {
        _root: TempDir,
        receipt_path: PathBuf,
        verifier: FileLegalReceiptVerifier,
        expectation: LegalReceiptExpectation,
        signer: SigningKey,
        export: LegalReceiptExport,
        now: DateTime<Utc>,
    }

    fn fixture() -> Fixture {
        let root = tempfile::tempdir().expect("tempdir");
        let receipt_root = root.path().join("receipts");
        fs::create_dir_all(&receipt_root).expect("receipt root");
        let trusted_keys_path = root.path().join("trusted-keys.json");
        let policy_path = root.path().join("policy.json");
        let receipt_path = receipt_root.join(format!("{RECEIPT_REF}.json"));
        let signer = SigningKey::from_bytes(&[7_u8; 32]);
        fs::write(
            &trusted_keys_path,
            serde_json::to_vec(&serde_json::json!({
                "keys": [{
                    "keyId": KEY_ID,
                    "publicKeyBase64url": URL_SAFE_NO_PAD.encode(signer.verifying_key().as_bytes()),
                }]
            }))
            .expect("keys json"),
        )
        .expect("write keys");
        fs::write(
            &policy_path,
            serde_json::to_vec(&serde_json::json!({
                "applicationId": "tradeassembly_studio",
                "documentId": "studio-live-disclosure",
                "documentVersion": "2026-07",
                "documentLocale": "en-US",
                "acknowledgementType": "live_trading_disclosure",
                "jurisdictionProfile": "US",
                "escrowPurpose": "live_order_submission",
                "maxExportTtlSeconds": 3600,
            }))
            .expect("policy json"),
        )
        .expect("write policy");
        let now = Utc
            .with_ymd_and_hms(2026, 8, 3, 18, 0, 0)
            .single()
            .expect("time");
        let body = "TradeAssembly live trading disclosure".to_string();
        let metadata = LegalDocumentVersion {
            key: LegalDocumentKey {
                document_id: "studio-live-disclosure".to_string(),
                version: "2026-07".to_string(),
                locale: "en-US".to_string(),
            },
            document_type: "live_trading_disclosure".to_string(),
            body_hash: sha256_hex(body.as_bytes()),
            published_at: now - Duration::days(30),
            retired_at: None,
            material_change: false,
            requires_reacceptance: false,
        };
        let mut document = PublishedLegalDocument {
            metadata,
            body,
            hub_signing_key_id: KEY_ID.to_string(),
            signing_algorithm: LEGAL_SIGNING_ALGORITHM.to_string(),
            hub_signature: String::new(),
        };
        document.hub_signature = sign_payload(
            &signer,
            "legal-document",
            &PublishedDocumentPayload {
                metadata: &document.metadata,
                body: &document.body,
            },
        );
        let mut receipt = SignedLegalAcknowledgement {
            acknowledgement: LegalAcknowledgement {
                schema_version: LEGAL_RECEIPT_SCHEMA.to_string(),
                acknowledgement_id: "ack_test_123".to_string(),
                escrow_receipt_id: RECEIPT_REF.to_string(),
                document_id: document.metadata.key.document_id.clone(),
                document_version: document.metadata.key.version.clone(),
                document_locale: document.metadata.key.locale.clone(),
                document_hash: document.metadata.body_hash.clone(),
                acknowledgement_type: "live_trading_disclosure".to_string(),
                user_id: "user_123".to_string(),
                identity_issuer: "https://hub.tradeassembly.org".to_string(),
                identity_subject: "subject_123".to_string(),
                identity_assurance: IdentityAssurance {
                    acr: Some("urn:tradeassembly:loa:1".to_string()),
                    amr: vec!["pwd".to_string(), "mfa".to_string()],
                    auth_time: now - Duration::minutes(10),
                    verified_claim_refs: vec!["claim_hash".to_string()],
                },
                org_id: None,
                actor_id: "actor_user_123".to_string(),
                resource_ref: Some("tradeassembly://strategies/strategy_123".to_string()),
                resource_version_refs: vec![
                    "tradeassembly://strategy-versions/version_123".to_string(),
                    "sha256:spec_hash_123".to_string(),
                ],
                environment: "local_live".to_string(),
                application_id: "tradeassembly_studio".to_string(),
                application_version: env!("CARGO_PKG_VERSION").to_string(),
                application_build_digest: current_build_digest(),
                rendered_content_hash: sha256_hex("rendered"),
                acceptance_statement_hash: sha256_hex("accepted"),
                acceptance_challenge_id: "challenge_123".to_string(),
                acceptance_request_hash: sha256_hex("request"),
                presented_at: now - Duration::minutes(5),
                accepted_at: now - Duration::minutes(4),
                session_strength: Some("mfa".to_string()),
                ui_surface_ref: "studio://legal/live-disclosure".to_string(),
                evidence_bundle_id: Some("evidence_123".to_string()),
                expires_at: Some(now + Duration::days(30)),
                jurisdiction_profile: "US".to_string(),
                escrow_required_for: vec!["live_order_submission".to_string()],
                escrow_confirmed_at: now - Duration::minutes(3),
                canonicalization_profile: LEGAL_CANONICALIZATION_PROFILE.to_string(),
            },
            hub_signing_key_id: KEY_ID.to_string(),
            signing_algorithm: LEGAL_SIGNING_ALGORITHM.to_string(),
            hub_signature: String::new(),
        };
        resign_receipt(&signer, &mut receipt);
        let mut export = LegalReceiptExport {
            schema_version: LEGAL_RECEIPT_EXPORT_SCHEMA.to_string(),
            receipt,
            document,
            lifecycle_events: Vec::new(),
            later_documents: Vec::new(),
            checked_at: now,
            valid_until: now + Duration::minutes(30),
            hub_signing_key_id: KEY_ID.to_string(),
            signing_algorithm: LEGAL_SIGNING_ALGORITHM.to_string(),
            hub_signature: String::new(),
        };
        write_export(&signer, &mut export, &receipt_path);
        Fixture {
            _root: root,
            receipt_path,
            verifier: FileLegalReceiptVerifier::new(receipt_root, trusted_keys_path, policy_path),
            expectation: LegalReceiptExpectation {
                receipt_ref: RECEIPT_REF.to_string(),
                identity_issuer: "https://hub.tradeassembly.org".to_string(),
                identity_subject: "subject_123".to_string(),
                resource_ref: "tradeassembly://strategies/strategy_123".to_string(),
                resource_version_refs: vec![
                    "tradeassembly://strategy-versions/version_123".to_string(),
                    "sha256:spec_hash_123".to_string(),
                ],
                environment: "local_live".to_string(),
            },
            signer,
            export,
            now,
        }
    }

    fn sign_payload<T: Serialize>(signer: &SigningKey, domain: &str, payload: &T) -> String {
        URL_SAFE_NO_PAD.encode(
            signer
                .sign(
                    &canonical_signed_message(domain, KEY_ID, LEGAL_SIGNING_ALGORITHM, payload)
                        .expect("canonical message"),
                )
                .to_bytes(),
        )
    }

    #[derive(Serialize)]
    struct ProductContractSignedPayload<'a, T: Serialize> {
        hub_signing_key_id: &'a str,
        signing_algorithm: &'a str,
        payload: &'a T,
    }

    fn product_contract_signed_message<T: Serialize>(domain: &str, payload: &T) -> Vec<u8> {
        let signed = ProductContractSignedPayload {
            hub_signing_key_id: KEY_ID,
            signing_algorithm: LEGAL_SIGNING_ALGORITHM,
            payload,
        };
        let payload = serde_json::to_vec(&signed).expect("Product contract payload");
        let mut message = Vec::with_capacity(domain.len() + payload.len() + 32);
        message.extend_from_slice(b"TradeAssembly Hub\0");
        message.extend_from_slice(LEGAL_CANONICALIZATION_PROFILE.as_bytes());
        message.push(0);
        message.extend_from_slice(domain.as_bytes());
        message.push(0);
        message.extend_from_slice(&(payload.len() as u64).to_be_bytes());
        message.extend_from_slice(&payload);
        message
    }

    fn resign_receipt(signer: &SigningKey, receipt: &mut SignedLegalAcknowledgement) {
        receipt.hub_signature =
            sign_payload(signer, "legal-acknowledgement", &receipt.acknowledgement);
    }

    fn write_export(signer: &SigningKey, export: &mut LegalReceiptExport, path: &Path) {
        export.hub_signature = URL_SAFE_NO_PAD.encode(
            signer
                .sign(&product_contract_signed_message(
                    "legal-receipt-export",
                    &LegalReceiptExportPayload::from(&*export),
                ))
                .to_bytes(),
        );
        fs::write(path, serde_json::to_vec(export).expect("export json")).expect("write export");
    }

    #[test]
    fn verifies_export_signed_by_the_product_contract_fixture() {
        let fixture = fixture();
        let verified = fixture
            .verifier
            .verify(&fixture.expectation, fixture.now.timestamp_millis())
            .expect("valid receipt");
        assert_eq!(verified.receipt_ref, RECEIPT_REF);
        assert_eq!(verified.document_version, "2026-07");
        assert!(valid_digest(&verified.export_digest));
    }

    #[test]
    fn rejects_forged_export_without_leaking_signature() {
        let mut fixture = fixture();
        fixture.export.hub_signature = "forged-secret-signature".to_string();
        fs::write(
            &fixture.receipt_path,
            serde_json::to_vec(&fixture.export).expect("export json"),
        )
        .expect("write forged export");
        let failure = fixture
            .verifier
            .verify(&fixture.expectation, fixture.now.timestamp_millis())
            .expect_err("forgery blocked");
        assert_eq!(failure, LegalReceiptFailure::Invalid);
        assert!(!failure.code().contains("signature"));
    }

    #[test]
    fn rejects_stale_export() {
        let mut fixture = fixture();
        fixture.export.checked_at = fixture.now - Duration::hours(2);
        fixture.export.valid_until = fixture.now - Duration::hours(1);
        write_export(&fixture.signer, &mut fixture.export, &fixture.receipt_path);
        assert_eq!(
            fixture
                .verifier
                .verify(&fixture.expectation, fixture.now.timestamp_millis()),
            Err(LegalReceiptFailure::Stale)
        );
    }

    #[test]
    fn rejects_expired_receipt() {
        let mut fixture = fixture();
        fixture.export.receipt.acknowledgement.expires_at = Some(fixture.now);
        resign_receipt(&fixture.signer, &mut fixture.export.receipt);
        write_export(&fixture.signer, &mut fixture.export, &fixture.receipt_path);
        assert_eq!(
            fixture
                .verifier
                .verify(&fixture.expectation, fixture.now.timestamp_millis()),
            Err(LegalReceiptFailure::Expired)
        );
    }

    #[test]
    fn rejects_revoked_receipt() {
        let mut fixture = fixture();
        let mut event = ReceiptLifecycleEvent {
            event_id: "event_1".to_string(),
            escrow_receipt_id: RECEIPT_REF.to_string(),
            sequence: 1,
            previous_event_hash: None,
            kind: LifecycleEventKind::Revoked,
            effective_at: fixture.now,
            reason_code: "user_revoked".to_string(),
            event_hash: String::new(),
        };
        event.event_hash = lifecycle_event_hash(&event).expect("event hash");
        let signed = SignedLifecycleEvent {
            hub_signature: sign_payload(&fixture.signer, "legal-lifecycle-event", &event),
            event,
            hub_signing_key_id: KEY_ID.to_string(),
            signing_algorithm: LEGAL_SIGNING_ALGORITHM.to_string(),
        };
        fixture.export.lifecycle_events.push(signed);
        write_export(&fixture.signer, &mut fixture.export, &fixture.receipt_path);
        assert_eq!(
            fixture
                .verifier
                .verify(&fixture.expectation, fixture.now.timestamp_millis()),
            Err(LegalReceiptFailure::Revoked)
        );
    }

    #[test]
    fn rejects_materially_superseded_document() {
        let mut fixture = fixture();
        let mut later = fixture.export.document.metadata.clone();
        later.key.version = "2026-08".to_string();
        later.published_at = fixture.now;
        later.material_change = true;
        later.requires_reacceptance = true;
        fixture.export.later_documents.push(later);
        write_export(&fixture.signer, &mut fixture.export, &fixture.receipt_path);
        assert_eq!(
            fixture
                .verifier
                .verify(&fixture.expectation, fixture.now.timestamp_millis()),
            Err(LegalReceiptFailure::Superseded)
        );
    }

    #[test]
    fn rejects_every_dynamic_binding_mismatch() {
        let fixture = fixture();
        let mut mismatches = Vec::new();
        let mut wrong = fixture.expectation.clone();
        wrong.identity_issuer = "https://attacker.example".to_string();
        mismatches.push(wrong);
        let mut wrong = fixture.expectation.clone();
        wrong.identity_subject = "other_subject".to_string();
        mismatches.push(wrong);
        let mut wrong = fixture.expectation.clone();
        wrong.resource_ref = "tradeassembly://strategies/other".to_string();
        mismatches.push(wrong);
        let mut wrong = fixture.expectation.clone();
        wrong.resource_version_refs.push("sha256:other".to_string());
        mismatches.push(wrong);
        let mut wrong = fixture.expectation.clone();
        wrong.environment = "self_hosted_live".to_string();
        mismatches.push(wrong);
        for mismatch in mismatches {
            assert_eq!(
                fixture
                    .verifier
                    .verify(&mismatch, fixture.now.timestamp_millis()),
                Err(LegalReceiptFailure::BindingMismatch)
            );
        }
    }
}
