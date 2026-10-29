// Copyright (c) 2026 OptionLab LLC. All rights reserved.
//! Offline, authenticated binary-update planning. No network or install side effects.
//! The signing key is supplied by the trusted release configuration, never the feed.

use crate::{is_digest, Result, TARGETS};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use ed25519_dalek::{Signature, VerifyingKey};
use semver::Version;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use url::Url;

const DOMAIN: &[u8] = b"tradeassembly.binary-update.v1\0";
const MAX_METADATA: usize = 65_536;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum UpdateProfile {
    Stable {},
    Pinned { version: String },
    Development { version: String },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Installation {
    pub profile: UpdateProfile,
    pub product: String,
    pub version: String,
    pub target: String,
    pub state_compatibility: String,
    pub accepted_sequence: u64,
    pub archive_sha256: String,
    pub bundle_manifest_sha256: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BinaryRelease {
    pub schema_version: u32,
    pub product: String,
    pub channel: String,
    pub version: String,
    pub target: String,
    pub sequence: u64,
    pub issued_at: u64,
    pub expires_at: u64,
    pub state_compatibility: String,
    pub archive_url: String,
    pub archive_sha256: String,
    pub bundle_manifest_sha256: String,
    /// Exact constituent identities; this bundle updates as one unit.
    pub components: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SignedBinaryRelease {
    /// Base64 of the exact JSON bytes. Signatures do not depend on reserialization.
    pub payload: String,
    pub signature: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum UpdateDecision {
    Disabled {
        version: String,
    },
    Current {
        sequence: u64,
    },
    Stage {
        version: String,
        sequence: u64,
        archive_url: String,
        archive_sha256: String,
        bundle_manifest_sha256: String,
        components: BTreeMap<String, String>,
    },
}

/// Return a staging decision only. A caller must retain the sequence durably,
/// authenticate downloaded bytes and recheck stopped state under the install lock.
/// Offline/invalid metadata leaves the installed version running unchanged.
pub fn plan(
    installed: &Installation,
    envelope: &[u8],
    trusted_public_key: &[u8; 32],
    now: u64,
) -> Result<UpdateDecision> {
    let current =
        Version::parse(&installed.version).map_err(|_| "update_installed_version_invalid")?;
    match &installed.profile {
        UpdateProfile::Pinned { version } | UpdateProfile::Development { version } => {
            if version != &installed.version {
                return Err("update_profile_version_mismatch".into());
            }
            return Ok(UpdateDecision::Disabled {
                version: version.clone(),
            });
        }
        UpdateProfile::Stable {} => {}
    }
    if !current.pre.is_empty() || !current.build.is_empty() {
        return Err("update_stable_installation_version_invalid".into());
    }
    let release = authenticate(envelope, trusted_public_key, now)?;
    if release.product != installed.product || release.target != installed.target {
        return Err("update_installation_identity_mismatch".into());
    }
    if release.state_compatibility != installed.state_compatibility {
        return Err("update_state_migration_required".into());
    }
    if now < release.issued_at || now >= release.expires_at {
        return Err("update_metadata_expired_or_future".into());
    }
    if release.sequence < installed.accepted_sequence {
        return Err("update_metadata_replayed".into());
    }
    let next = Version::parse(&release.version).map_err(|_| "update_version_invalid")?;
    if next < current {
        return Err("update_downgrade_denied".into());
    }
    if next == current {
        if installed.archive_sha256 != release.archive_sha256
            || installed.bundle_manifest_sha256 != release.bundle_manifest_sha256
        {
            return Err("update_same_version_changed".into());
        }
        return Ok(UpdateDecision::Current {
            sequence: release.sequence,
        });
    }
    if release.sequence == installed.accepted_sequence {
        return Err("update_sequence_not_advanced".into());
    }
    Ok(UpdateDecision::Stage {
        version: release.version,
        sequence: release.sequence,
        archive_url: release.archive_url,
        archive_sha256: release.archive_sha256,
        bundle_manifest_sha256: release.bundle_manifest_sha256,
        components: release.components,
    })
}

pub(crate) fn authenticate(
    envelope: &[u8],
    trusted_public_key: &[u8; 32],
    now: u64,
) -> Result<BinaryRelease> {
    if envelope.len() > MAX_METADATA {
        return Err("update_metadata_limit".into());
    }
    let signed: SignedBinaryRelease =
        serde_json::from_slice(envelope).map_err(|_| "update_envelope_invalid")?;
    let payload = STANDARD
        .decode(&signed.payload)
        .map_err(|_| "update_payload_invalid")?;
    let signature = STANDARD
        .decode(&signed.signature)
        .map_err(|_| "update_signature_invalid")?;
    let signature = Signature::from_slice(&signature).map_err(|_| "update_signature_invalid")?;
    let key = VerifyingKey::from_bytes(trusted_public_key).map_err(|_| "update_key_invalid")?;
    let message = [DOMAIN, payload.as_slice()].concat();
    key.verify_strict(&message, &signature)
        .map_err(|_| "update_signature_invalid")?;
    let release: BinaryRelease =
        serde_json::from_slice(&payload).map_err(|_| "update_payload_invalid")?;
    validate(&release)?;
    if now < release.issued_at || now >= release.expires_at {
        return Err("update_metadata_expired_or_future".into());
    }
    Ok(release)
}

fn validate(release: &BinaryRelease) -> Result<()> {
    let version = Version::parse(&release.version).map_err(|_| "update_version_invalid")?;
    if release.schema_version != 1
        || !matches!(
            release.product.as_str(),
            "core-relay-customer" | "core-standalone" | "studio-local"
        )
        || release.channel != "stable"
        || !version.pre.is_empty()
        || !version.build.is_empty()
        || !TARGETS.contains(&release.target.as_str())
        || release.sequence == 0
        || release.issued_at >= release.expires_at
        || release.expires_at - release.issued_at > 7 * 24 * 60 * 60
        || release.state_compatibility.is_empty()
        || release.state_compatibility.len() > 128
        || !is_digest(&release.archive_sha256)
        || !is_digest(&release.bundle_manifest_sha256)
        || release.components.is_empty()
        || release.components.len() > 32
        || release.components.iter().any(|(name, digest)| {
            name.is_empty()
                || name.len() > 64
                || !name
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
                || !is_digest(digest)
        })
    {
        return Err("update_contract_invalid".into());
    }
    let url = Url::parse(&release.archive_url).map_err(|_| "update_url_invalid")?;
    // Only immutable assets from the product's release repositories are supported.
    // Downloaders must validate every redirect and never forward credentials.
    let repository = match release.product.as_str() {
        "core-relay-customer" | "core-standalone" => "Core",
        "studio-local" => "Studio",
        _ => unreachable!(),
    };
    let prefix = format!(
        "/TradeAssembly/{repository}/releases/download/v{}/",
        release.version
    );
    let asset = url.path().strip_prefix(&prefix).unwrap_or("");
    if release.product == "core-relay-customer"
        && !["production", "staging"].iter().any(|environment| {
            asset
                == format!(
                    "TradeAssembly-{}-{environment}-{}.tar.gz",
                    release.version, release.target
                )
        })
    {
        return Err("update_customer_archive_kind_mismatch".into());
    }
    if release.product == "core-standalone"
        && (release.target != TARGETS[0]
            || asset != format!("tradeassembly-core-{}-macos-arm64.tar.gz", release.version))
    {
        return Err("update_standalone_archive_kind_or_target_mismatch".into());
    }
    if release.archive_url.len() > 2048
        || url.scheme() != "https"
        || url.host_str() != Some("github.com")
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || asset.is_empty()
        || !asset
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
    {
        return Err("update_url_invalid".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    fn fixture() -> (Installation, BinaryRelease, SigningKey) {
        (
            Installation {
                profile: UpdateProfile::Stable {},
                product: "core-relay-customer".into(),
                version: "0.2.0".into(),
                target: "aarch64-apple-darwin".into(),
                state_compatibility: "f2-local-v1".into(),
                accepted_sequence: 1,
                archive_sha256: "e".repeat(64),
                bundle_manifest_sha256: "f".repeat(64),
            },
            BinaryRelease {
                schema_version: 1,
                product: "core-relay-customer".into(),
                channel: "stable".into(),
                version: "0.3.0".into(),
                target: "aarch64-apple-darwin".into(),
                sequence: 2,
                issued_at: 100,
                expires_at: 200,
                state_compatibility: "f2-local-v1".into(),
                archive_url:
                    "https://github.com/TradeAssembly/Core/releases/download/v0.3.0/TradeAssembly-0.3.0-production-aarch64-apple-darwin.tar.gz"
                        .into(),
                archive_sha256: "a".repeat(64),
                bundle_manifest_sha256: "b".repeat(64),
                components: BTreeMap::from([
                    ("core".into(), "c".repeat(64)),
                    ("warden".into(), "d".repeat(64)),
                ]),
            },
            SigningKey::from_bytes(&[7; 32]),
        )
    }

    fn signed(release: &BinaryRelease, key: &SigningKey) -> Vec<u8> {
        let payload = serde_json::to_vec(release).unwrap();
        let signature = key.sign(&[DOMAIN, payload.as_slice()].concat());
        serde_json::to_vec(&SignedBinaryRelease {
            payload: STANDARD.encode(payload),
            signature: STANDARD.encode(signature.to_bytes()),
        })
        .unwrap()
    }

    #[test]
    fn stable_stages_exact_bundle_without_installing() {
        let (installed, release, key) = fixture();
        let result = plan(
            &installed,
            &signed(&release, &key),
            &key.verifying_key().to_bytes(),
            150,
        )
        .unwrap();
        assert!(
            matches!(result, UpdateDecision::Stage { components, archive_sha256, .. }
            if components == release.components && archive_sha256 == release.archive_sha256)
        );
    }

    #[test]
    fn customer_and_standalone_archive_identities_cannot_be_confused() {
        let (mut installed, mut release, key) = fixture();
        release.archive_url = "https://github.com/TradeAssembly/Core/releases/download/v0.3.0/tradeassembly-core-0.3.0-macos-arm64.tar.gz".into();
        assert!(plan(
            &installed,
            &signed(&release, &key),
            &key.verifying_key().to_bytes(),
            150
        )
        .is_err());
        installed.product = "core-standalone".into();
        release.product = installed.product.clone();
        assert!(matches!(
            plan(
                &installed,
                &signed(&release, &key),
                &key.verifying_key().to_bytes(),
                150
            )
            .unwrap(),
            UpdateDecision::Stage { .. }
        ));
        release.archive_url = "https://github.com/TradeAssembly/Core/releases/download/v0.3.0/TradeAssembly-0.3.0-production-aarch64-apple-darwin.tar.gz".into();
        assert!(plan(
            &installed,
            &signed(&release, &key),
            &key.verifying_key().to_bytes(),
            150
        )
        .is_err());
    }

    #[test]
    fn pinned_and_development_never_consume_feed() {
        let (mut installed, _, _) = fixture();
        for profile in [
            UpdateProfile::Pinned {
                version: "0.2.0".into(),
            },
            UpdateProfile::Development {
                version: "0.2.0".into(),
            },
        ] {
            installed.profile = profile;
            assert!(matches!(
                plan(&installed, b"offline or malicious", &[0; 32], 0).unwrap(),
                UpdateDecision::Disabled { .. }
            ));
        }
        installed.version = "0.3.0".into();
        assert!(plan(&installed, b"", &[0; 32], 0).is_err());
    }

    #[test]
    fn current_identity_and_sequence_are_immutable() {
        let (mut installed, release, key) = fixture();
        installed.version = release.version.clone();
        assert!(plan(
            &installed,
            &signed(&release, &key),
            &key.verifying_key().to_bytes(),
            150
        )
        .is_err());
        installed.archive_sha256 = release.archive_sha256.clone();
        installed.bundle_manifest_sha256 = release.bundle_manifest_sha256.clone();
        assert_eq!(
            plan(
                &installed,
                &signed(&release, &key),
                &key.verifying_key().to_bytes(),
                150
            )
            .unwrap(),
            UpdateDecision::Current { sequence: 2 }
        );
        installed.accepted_sequence = 3;
        assert!(plan(
            &installed,
            &signed(&release, &key),
            &key.verifying_key().to_bytes(),
            150
        )
        .is_err());
    }

    #[test]
    fn tampering_and_untrusted_keys_fail() {
        let (installed, release, key) = fixture();
        let mut envelope: SignedBinaryRelease =
            serde_json::from_slice(&signed(&release, &key)).unwrap();
        let mut changed = release.clone();
        changed.archive_sha256 = "f".repeat(64);
        envelope.payload = STANDARD.encode(serde_json::to_vec(&changed).unwrap());
        assert!(plan(
            &installed,
            &serde_json::to_vec(&envelope).unwrap(),
            &key.verifying_key().to_bytes(),
            150
        )
        .is_err());
        let wrong = SigningKey::from_bytes(&[8; 32]);
        assert!(plan(
            &installed,
            &signed(&release, &key),
            &wrong.verifying_key().to_bytes(),
            150
        )
        .is_err());
    }

    #[test]
    fn incompatible_stale_and_unsafe_releases_fail_closed() {
        let (installed, release, key) = fixture();
        let mut cases = Vec::new();
        macro_rules! case {
            ($field:ident, $value:expr) => {{
                let mut r = release.clone();
                r.$field = $value;
                cases.push(r);
            }};
        }
        case!(target, "x86_64-unknown-linux-gnu".into());
        case!(product, "studio-local".into());
        case!(state_compatibility, "next-schema".into());
        case!(sequence, 0);
        case!(sequence, 1);
        case!(expires_at, 150);
        case!(issued_at, 151);
        case!(expires_at, 1_000_000);
        case!(version, "0.1.0".into());
        case!(version, "0.3.0-beta.1".into());
        case!(channel, "beta".into());
        case!(components, BTreeMap::new());
        for url in [
            "http://github.com/x",
            "https://evil.example/core.tar.gz",
            "https://github.com/TradeAssembly/Core/releases/latest/core.tar.gz",
            "https://github.com/TradeAssembly/Core/releases/download/v0.3.0/TradeAssembly-0.3.0-production-aarch64-apple-darwin.tar.gz?token=x",
            "https://github.com/TradeAssembly/Studio/releases/download/v0.3.0/core.tar.gz",
        ] {
            case!(archive_url, url.into());
        }
        for r in cases {
            assert!(
                plan(
                    &installed,
                    &signed(&r, &key),
                    &key.verifying_key().to_bytes(),
                    150
                )
                .is_err(),
                "{r:?}"
            );
        }
        assert!(plan(
            &installed,
            &vec![0; MAX_METADATA + 1],
            &key.verifying_key().to_bytes(),
            150
        )
        .is_err());
        assert!(
            serde_json::from_str::<UpdateProfile>(r#"{"mode":"stable","version":"0.2.0"}"#)
                .is_err()
        );
    }
}
