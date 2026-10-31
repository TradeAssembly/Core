// Copyright (c) 2026 OptionLab LLC. All rights reserved.
//! Offline producer-proof verification and signing. No network, Apple or
//! publication effects. Production key access happens only after verification.
use crate::{
    updates::{self, BinaryRelease, SignedBinaryRelease},
    Result,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use ed25519_dalek::{Signer, SigningKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
const LIMIT: usize = 65_536;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PublicationIdentity {
    pub schema_version: u32,
    pub product: String,
    pub channel: String,
    pub version: String,
    pub target: String,
    pub state_compatibility: String,
    pub archive_url: String,
    pub archive_sha256: String,
    pub bundle_manifest_sha256: String,
    pub components: BTreeMap<String, String>,
}
impl PublicationIdentity {
    fn release(release: &BinaryRelease) -> Self {
        Self {
            schema_version: release.schema_version,
            product: release.product.clone(),
            channel: release.channel.clone(),
            version: release.version.clone(),
            target: release.target.clone(),
            state_compatibility: release.state_compatibility.clone(),
            archive_url: release.archive_url.clone(),
            archive_sha256: release.archive_sha256.clone(),
            bundle_manifest_sha256: release.bundle_manifest_sha256.clone(),
            components: release.components.clone(),
        }
    }
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PublicationBinding {
    pub schema: String,
    pub record_sha256: String,
    pub native_input_proof_sha256: String,
    pub coordinator_revision: String,
    pub core_revision: String,
    pub relay_revision: String,
    pub apple_team_id: String,
    pub submission_id: String,
    pub signed_container_sha256: String,
    pub stapled_container_sha256: String,
    pub notary_receipt_sha256: String,
    pub status: String,
    pub identity: PublicationIdentity,
    pub published: bool,
}
fn hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        && value.bytes().any(|b| b != b'0')
}
fn uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(i, b)| {
            if [8, 13, 18, 23].contains(&i) {
                b == b'-'
            } else {
                b.is_ascii_hexdigit()
            }
        })
        && value.bytes().any(|b| b != b'0' && b != b'-')
}
fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// Sealed checked request. No public constructor/deserializer or mutable payload.
/// Keeping the exact bytes here prevents verify-then-replace file races.
pub struct VerifiedPublication {
    payload: Vec<u8>,
    payload_sha256: String,
    binding_sha256: String,
    archive_sha256: String,
}
impl VerifiedPublication {
    pub fn payload_sha256(&self) -> &str {
        &self.payload_sha256
    }
    pub fn binding_sha256(&self) -> &str {
        &self.binding_sha256
    }
    pub fn archive_sha256(&self) -> &str {
        &self.archive_sha256
    }
    pub fn receipt(&self) -> serde_json::Value {
        serde_json::json!({"verified":true,"payloadSha256":self.payload_sha256,"bindingSha256":self.binding_sha256,"archiveSha256":self.archive_sha256})
    }
    /// Key loaders are deferred until a sealed publication request exists. The
    /// supplied key must come from approved custody, never input proof/metadata.
    pub fn sign_with_key(
        self,
        key: impl FnOnce() -> Result<[u8; 32]>,
    ) -> Result<SignedBinaryRelease> {
        let seed = key()?;
        let key = SigningKey::from_bytes(&seed);
        let signature = key.sign(&[updates::DOMAIN, self.payload.as_slice()].concat());
        Ok(SignedBinaryRelease {
            payload: STANDARD.encode(&self.payload),
            signature: STANDARD.encode(signature.to_bytes()),
        })
    }
}
/// `expected_binding_sha256` is independent release approval, not a digest read
/// from the proof's adjacent untrusted checksum file. Binding status is evidence
/// from the trusted native producer; this function does not query Apple itself.
pub fn verify(
    binding: &[u8],
    expected_binding_sha256: &str,
    payload: &[u8],
    now: u64,
) -> Result<VerifiedPublication> {
    if binding.is_empty() || payload.is_empty() || binding.len() > LIMIT || payload.len() > LIMIT {
        return Err("publication_proof_or_payload_limit".into());
    }
    if !hex(expected_binding_sha256, 64) || sha(binding) != expected_binding_sha256 {
        return Err("publication_binding_not_approved".into());
    }
    let proof: PublicationBinding =
        serde_json::from_slice(binding).map_err(|_| "publication_binding_invalid")?;
    if proof.schema != "tradeassembly.customer-archive-notary-binding/v1"
        || proof.status != "Accepted"
        || proof.published
        || !uuid(&proof.submission_id)
        || proof.apple_team_id.len() != 10
        || !proof
            .apple_team_id
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
        || [
            &proof.coordinator_revision,
            &proof.core_revision,
            &proof.relay_revision,
        ]
        .iter()
        .any(|v| !hex(v, 40))
        || [
            &proof.record_sha256,
            &proof.native_input_proof_sha256,
            &proof.signed_container_sha256,
            &proof.stapled_container_sha256,
            &proof.notary_receipt_sha256,
        ]
        .iter()
        .any(|v| !hex(v, 64))
    {
        return Err("publication_native_proof_unaccepted".into());
    }
    let release: BinaryRelease =
        serde_json::from_slice(payload).map_err(|_| "publication_payload_invalid")?;
    updates::validate(&release)?;
    if release.product!="core-relay-customer" || release.target!="aarch64-apple-darwin"
        || release.state_compatibility!="customer-delivery-v1"
        || release.archive_url!=format!("https://github.com/TradeAssembly/Core/releases/download/v{}/TradeAssembly-{}-production-aarch64-apple-darwin.tar.gz",release.version,release.version)
        || release.components.keys().map(String::as_str).collect::<Vec<_>>()!=["core","helper","relay"]
        || release.components.values().any(|v|!hex(v,64))
        || !hex(&release.archive_sha256,64) || !hex(&release.bundle_manifest_sha256,64)
        || PublicationIdentity::release(&release)!=proof.identity {
        return Err("publication_payload_identity_or_pins_changed".into());
    }
    if release.issued_at > now || now >= release.expires_at {
        return Err("publication_metadata_expired_or_future".into());
    }
    Ok(VerifiedPublication {
        payload: payload.to_vec(),
        payload_sha256: sha(payload),
        binding_sha256: expected_binding_sha256.into(),
        archive_sha256: release.archive_sha256,
    })
}

/// Keyless release-pipeline renewal preparation. A still-valid authenticated
/// predecessor and independent exact proof approval are required. This function
/// never loads a signing key, changes artifact identity, or publishes a feed.
pub fn prepare_renewal(
    binding: &[u8],
    expected_binding_sha256: &str,
    previous: &[u8],
    expected_previous_sha256: &str,
    trusted_public_key: &[u8; 32],
    next_sequence: u64,
    now: u64,
) -> Result<serde_json::Value> {
    if !hex(expected_previous_sha256, 64) || sha(previous) != expected_previous_sha256 {
        return Err("renewal_predecessor_not_approved".into());
    }
    let mut release = updates::authenticate(previous, trusted_public_key, now)?;
    if next_sequence <= release.sequence
        || next_sequence > 9_007_199_254_740_991
        || now <= release.issued_at
    {
        return Err("renewal_sequence_or_clock_not_advanced".into());
    }
    let expires = now.checked_add(604_800).ok_or("renewal_clock_overflow")?;
    if expires <= release.expires_at {
        return Err("renewal_expiry_not_advanced".into());
    }
    release.sequence = next_sequence;
    release.issued_at = now;
    release.expires_at = expires;
    let payload = serde_json::to_vec(&release).map_err(|_| "renewal_payload_invalid")?;
    let verified = verify(binding, expected_binding_sha256, &payload, now)?;
    Ok(
        serde_json::json!({"schema":"tradeassembly.update-renewal-plan/v1",
        "payload":release,"previousMetadataSha256":expected_previous_sha256,
        "verification":verified.receipt(),"signed":false,"published":false}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::Cell, rc::Rc};
    fn fixture() -> (Vec<u8>, Vec<u8>) {
        let release=BinaryRelease {schema_version:1,product:"core-relay-customer".into(),channel:"stable".into(),version:"0.3.0".into(),target:"aarch64-apple-darwin".into(),sequence:1,issued_at:100,expires_at:200,state_compatibility:"customer-delivery-v1".into(),archive_url:"https://github.com/TradeAssembly/Core/releases/download/v0.3.0/TradeAssembly-0.3.0-production-aarch64-apple-darwin.tar.gz".into(),archive_sha256:"a".repeat(64),bundle_manifest_sha256:"b".repeat(64),components:BTreeMap::from([("core".into(),"c".repeat(64)),("relay".into(),"d".repeat(64)),("helper".into(),"e".repeat(64))])};
        let proof = PublicationBinding {
            schema: "tradeassembly.customer-archive-notary-binding/v1".into(),
            record_sha256: "1".repeat(64),
            native_input_proof_sha256: "2".repeat(64),
            coordinator_revision: "3".repeat(40),
            core_revision: "4".repeat(40),
            relay_revision: "5".repeat(40),
            apple_team_id: "ABCDE12345".into(),
            submission_id: "11111111-2222-3333-4444-555555555555".into(),
            signed_container_sha256: "6".repeat(64),
            stapled_container_sha256: "7".repeat(64),
            notary_receipt_sha256: "8".repeat(64),
            status: "Accepted".into(),
            identity: PublicationIdentity::release(&release),
            published: false,
        };
        (
            serde_json::to_vec(&proof).unwrap(),
            serde_json::to_vec_pretty(&release).unwrap(),
        )
    }
    #[test]
    fn renewal_authenticates_predecessor_and_preserves_exact_identity() {
        let (proof, payload) = fixture();
        let key = SigningKey::from_bytes(&[9; 32]);
        let old = verify(&proof, &sha(&proof), &payload, 100)
            .unwrap()
            .sign_with_key(|| Ok(key.to_bytes()))
            .unwrap();
        let old = serde_json::to_vec(&old).unwrap();
        let renew = |bytes: &[u8], digest: &str, seq, now| {
            prepare_renewal(
                &proof,
                &sha(&proof),
                bytes,
                digest,
                &key.verifying_key().to_bytes(),
                seq,
                now,
            )
        };
        let plan = renew(&old, &sha(&old), 2, 150).unwrap();
        let next: BinaryRelease = serde_json::from_value(plan["payload"].clone()).unwrap();
        let previous: BinaryRelease = serde_json::from_slice(&payload).unwrap();
        assert_eq!(
            PublicationIdentity::release(&next),
            PublicationIdentity::release(&previous)
        );
        assert_eq!(
            (next.sequence, next.issued_at, next.expires_at),
            (2, 150, 604_950)
        );
        assert_eq!(plan["signed"], false);
        assert_eq!(plan["published"], false);
        for (seq, now) in [
            (1, 150),
            (0, 150),
            (2, 100),
            (2, 200),
            (9_007_199_254_740_992, 150),
        ] {
            assert!(renew(&old, &sha(&old), seq, now).is_err());
        }
        assert!(renew(&old, &"a".repeat(64), 2, 150).is_err());
        let mut changed: SignedBinaryRelease = serde_json::from_slice(&old).unwrap();
        changed.signature = STANDARD.encode([0u8; 64]);
        let changed = serde_json::to_vec(&changed).unwrap();
        assert!(renew(&changed, &sha(&changed), 2, 150).is_err());
        let mut changed: serde_json::Value = serde_json::from_slice(&proof).unwrap();
        changed["identity"]["components"]["core"] = serde_json::json!("f".repeat(64));
        let changed = serde_json::to_vec(&changed).unwrap();
        assert!(prepare_renewal(
            &changed,
            &sha(&changed),
            &old,
            &sha(&old),
            &key.verifying_key().to_bytes(),
            2,
            150
        )
        .is_err());
    }

    fn try_sign(
        proof: &[u8],
        approved: &str,
        payload: &[u8],
        now: u64,
        calls: Rc<Cell<usize>>,
    ) -> Result<SignedBinaryRelease> {
        verify(proof, approved, payload, now)?.sign_with_key(|| {
            calls.set(calls.get() + 1);
            Ok([7; 32])
        })
    }
    #[test]
    fn signer_requires_proof_and_preserves_exact_verified_payload_bytes() {
        let (proof, payload) = fixture();
        let calls = Rc::new(Cell::new(0));
        let envelope = try_sign(&proof, &sha(&proof), &payload, 150, calls.clone()).unwrap();
        assert_eq!(calls.get(), 1);
        assert_eq!(STANDARD.decode(&envelope.payload).unwrap(), payload);
        let key = SigningKey::from_bytes(&[7; 32]);
        updates::authenticate(
            &serde_json::to_vec(&envelope).unwrap(),
            &key.verifying_key().to_bytes(),
            150,
        )
        .unwrap();
    }
    #[test]
    fn missing_unapproved_malformed_and_uncertain_proof_never_loads_key() {
        let (proof, payload) = fixture();
        for bad in [
            Vec::new(),
            b"{}".to_vec(),
            b"null".to_vec(),
            vec![b' '; LIMIT + 1],
        ] {
            let calls = Rc::new(Cell::new(0));
            assert!(try_sign(&bad, &sha(&bad), &payload, 150, calls.clone()).is_err());
            assert_eq!(calls.get(), 0);
        }
        let calls = Rc::new(Cell::new(0));
        assert!(try_sign(&proof, &"f".repeat(64), &payload, 150, calls.clone()).is_err());
        assert_eq!(calls.get(), 0);
        for (field, value) in [
            ("status", serde_json::json!("In Progress")),
            ("published", serde_json::json!(true)),
            ("submissionId", serde_json::json!("")),
            ("coreRevision", serde_json::json!("0".repeat(40))),
            ("notaryReceiptSha256", serde_json::json!("0".repeat(64))),
        ] {
            let mut v: serde_json::Value = serde_json::from_slice(&proof).unwrap();
            v[field] = value;
            let changed = serde_json::to_vec(&v).unwrap();
            let calls = Rc::new(Cell::new(0));
            assert!(try_sign(&changed, &sha(&changed), &payload, 150, calls.clone()).is_err());
            assert_eq!(calls.get(), 0);
        }
    }
    #[test]
    fn archive_manifest_version_and_each_component_substitution_never_loads_key() {
        let (proof, payload) = fixture();
        for (field, value) in [
            ("archive_sha256", serde_json::json!("f".repeat(64))),
            ("bundle_manifest_sha256", serde_json::json!("f".repeat(64))),
            (
                "archive_url",
                serde_json::json!("https://example.invalid/archive"),
            ),
            ("version", serde_json::json!("0.4.0")),
            ("target", serde_json::json!("x86_64-apple-darwin")),
        ] {
            let mut v: serde_json::Value = serde_json::from_slice(&payload).unwrap();
            v[field] = value;
            let calls = Rc::new(Cell::new(0));
            assert!(try_sign(
                &proof,
                &sha(&proof),
                &serde_json::to_vec(&v).unwrap(),
                150,
                calls.clone()
            )
            .is_err());
            assert_eq!(calls.get(), 0);
        }
        for component in ["core", "relay", "helper"] {
            let mut v: serde_json::Value = serde_json::from_slice(&payload).unwrap();
            v["components"][component] = serde_json::json!("f".repeat(64));
            let calls = Rc::new(Cell::new(0));
            assert!(try_sign(
                &proof,
                &sha(&proof),
                &serde_json::to_vec(&v).unwrap(),
                150,
                calls.clone()
            )
            .is_err());
            assert_eq!(calls.get(), 0);
        }
    }
    #[test]
    fn expiry_unknown_fields_duplicate_fields_and_extra_pins_are_rejected() {
        let (proof, payload) = fixture();
        for now in [99, 200] {
            assert!(verify(&proof, &sha(&proof), &payload, now).is_err());
        }
        let mut v: serde_json::Value = serde_json::from_slice(&payload).unwrap();
        v["components"]["extra"] = serde_json::json!("f".repeat(64));
        assert!(verify(&proof, &sha(&proof), &serde_json::to_vec(&v).unwrap(), 150).is_err());
        let mut v: serde_json::Value = serde_json::from_slice(&proof).unwrap();
        v["override"] = serde_json::json!(true);
        let v = serde_json::to_vec(&v).unwrap();
        assert!(verify(&v, &sha(&v), &payload, 150).is_err());
        let text = String::from_utf8(payload)
            .unwrap()
            .replacen('{', "{\"sequence\":9,", 1);
        assert!(verify(&proof, &sha(&proof), text.as_bytes(), 150).is_err());
    }
}
