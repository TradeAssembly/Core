// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use crate::ports::{IdentityClaims, TrustedStudioSession};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use sha2::{Digest, Sha256};

#[derive(Clone, Debug)]
pub struct TrustedStudioVerifier {
    issuer: String,
    audience: String,
    credential_digest: Option<[u8; 32]>,
}

impl TrustedStudioVerifier {
    pub fn new(
        issuer: impl Into<String>,
        audience: impl Into<String>,
        credential: Option<String>,
    ) -> Result<Self, String> {
        let issuer = issuer.into();
        let audience = audience.into();
        if issuer.trim().is_empty() || audience.trim().is_empty() {
            return Err("trusted Studio OIDC authority is incomplete".to_string());
        }
        let credential_digest = credential
            .filter(|value| !value.trim().is_empty())
            .map(|value| {
                if value.len() < 32 {
                    return Err(
                        "trusted Studio credential must contain at least 32 bytes".to_string()
                    );
                }
                Ok(Sha256::digest(value.as_bytes()).into())
            })
            .transpose()?;
        Ok(Self {
            issuer,
            audience,
            credential_digest,
        })
    }

    pub fn authenticate(
        &self,
        bearer_credential: &str,
        session: &TrustedStudioSession,
        now_ms: i64,
    ) -> Result<IdentityClaims, String> {
        let expected = self
            .credential_digest
            .ok_or_else(|| "trusted Studio transport is not configured".to_string())?;
        let provided: [u8; 32] = Sha256::digest(bearer_credential.as_bytes()).into();
        if !constant_time_equal(&expected, &provided) {
            return Err("trusted Studio transport authentication failed".to_string());
        }
        if session.issuer != self.issuer
            || !session.audience.iter().any(|value| value == &self.audience)
            || session.expires_at_ms <= now_ms
            || session.subject.trim().is_empty()
        {
            return Err("trusted Studio OIDC assertion is invalid".to_string());
        }
        if session.actor != stable_actor(&session.issuer, &session.subject) {
            return Err("trusted Studio actor binding is invalid".to_string());
        }
        Ok(IdentityClaims {
            issuer: session.issuer.clone(),
            subject: session.actor.clone(),
            audience: session.audience.clone(),
            assurance: Some("oidc_authorization_code_pkce".to_string()),
            expires_at_ms: session.expires_at_ms,
        })
    }
}

pub(crate) fn stable_actor(issuer: &str, subject: &str) -> String {
    let digest = Sha256::digest(format!("{issuer}\0{subject}").as_bytes());
    format!("oidc:{}", URL_SAFE_NO_PAD.encode(digest))
}

fn constant_time_equal(left: &[u8; 32], right: &[u8; 32]) -> bool {
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}

#[cfg(test)]
mod tests {
    use super::{stable_actor, TrustedStudioVerifier};
    use crate::ports::TrustedStudioSession;

    fn session() -> TrustedStudioSession {
        TrustedStudioSession {
            issuer: "https://id.tradeassembly.test".to_string(),
            subject: "subject-1".to_string(),
            audience: vec!["tradeassembly-local".to_string()],
            actor: stable_actor("https://id.tradeassembly.test", "subject-1"),
            display_name: "Test User".to_string(),
            email: Some("user@example.test".to_string()),
            expires_at_ms: 2_000,
        }
    }

    #[test]
    fn authenticates_bound_oidc_assertion_and_rejects_forgery() {
        let verifier = TrustedStudioVerifier::new(
            "https://id.tradeassembly.test",
            "tradeassembly-local",
            Some("studio-core-test-token-with-32-bytes".to_string()),
        )
        .expect("verifier");
        assert!(verifier
            .authenticate("studio-core-test-token-with-32-bytes", &session(), 1_000)
            .is_ok());

        let mut forged = session();
        forged.actor = "oidc:forged".to_string();
        assert!(verifier
            .authenticate("studio-core-test-token-with-32-bytes", &forged, 1_000)
            .is_err());
        assert!(verifier
            .authenticate("wrong-studio-core-token-with-32-bytes", &session(), 1_000)
            .is_err());
    }

    #[test]
    fn rejects_mismatched_or_expired_oidc_assertions() {
        let verifier = TrustedStudioVerifier::new(
            "https://id.tradeassembly.test",
            "tradeassembly-local",
            Some("studio-core-test-token-with-32-bytes".to_string()),
        )
        .expect("verifier");
        let credential = "studio-core-test-token-with-32-bytes";

        let mut wrong_issuer = session();
        wrong_issuer.issuer = "https://forged.example.test".to_string();
        wrong_issuer.actor = stable_actor(&wrong_issuer.issuer, &wrong_issuer.subject);
        assert!(verifier
            .authenticate(credential, &wrong_issuer, 1_000)
            .is_err());

        let mut wrong_audience = session();
        wrong_audience.audience = vec!["other-client".to_string()];
        assert!(verifier
            .authenticate(credential, &wrong_audience, 1_000)
            .is_err());

        let mut expired = session();
        expired.expires_at_ms = 1_000;
        assert!(verifier.authenticate(credential, &expired, 1_000).is_err());

        let mut blank_subject = session();
        blank_subject.subject = " ".to_string();
        blank_subject.actor = stable_actor(&blank_subject.issuer, &blank_subject.subject);
        assert!(verifier
            .authenticate(credential, &blank_subject, 1_000)
            .is_err());
    }
}
