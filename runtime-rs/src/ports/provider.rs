// Copyright (c) 2026 OptionLab LLC. All rights reserved.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderCapability {
    pub provider_ref: String,
    pub capability: String,
    pub available: bool,
}

pub trait ProviderPort: VersionedPort {
    fn capabilities(&self, provider_ref: &str) -> Vec<ProviderCapability>;
}

use crate::ports::VersionedPort;
