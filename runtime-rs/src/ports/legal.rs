// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use crate::ports::VersionedPort;
use serde::{Deserialize, Serialize};

pub const LEGAL_RECEIPT_EXPORT_SCHEMA: &str = "tradeassembly.legal-receipt-export/v1";
pub const LEGAL_RECEIPT_SCHEMA: &str = "tradeassembly.legal-acknowledgement/v1";
pub const LEGAL_CANONICALIZATION_PROFILE: &str = "tradeassembly.struct-json/v1";
pub const LEGAL_SIGNING_ALGORITHM: &str = "Ed25519";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LegalReceiptExpectation {
    pub receipt_ref: String,
    pub identity_issuer: String,
    pub identity_subject: String,
    pub resource_ref: String,
    pub resource_version_refs: Vec<String>,
    pub environment: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VerifiedLegalReceipt {
    pub receipt_ref: String,
    pub acknowledgement_id: String,
    pub document_id: String,
    pub document_version: String,
    pub document_locale: String,
    pub identity_issuer: String,
    pub identity_subject: String,
    pub accepted_at: String,
    pub valid_until: String,
    pub export_digest: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LegalReceiptFailure {
    Missing,
    Invalid,
    Stale,
    Expired,
    Revoked,
    Superseded,
    BindingMismatch,
    Unavailable,
}

impl LegalReceiptFailure {
    pub fn code(self) -> &'static str {
        match self {
            Self::Missing => "legal_receipt_missing",
            Self::Invalid => "legal_receipt_invalid",
            Self::Stale => "legal_receipt_stale",
            Self::Expired => "legal_receipt_expired",
            Self::Revoked => "legal_receipt_revoked",
            Self::Superseded => "legal_document_superseded",
            Self::BindingMismatch => "legal_receipt_binding_mismatch",
            Self::Unavailable => "legal_receipt_unavailable",
        }
    }
}

pub trait LegalReceiptPort: VersionedPort {
    fn verify(
        &self,
        expectation: &LegalReceiptExpectation,
        trusted_now_ms: i64,
    ) -> Result<VerifiedLegalReceipt, LegalReceiptFailure>;
}
