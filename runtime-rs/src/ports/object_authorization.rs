// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use crate::ports::{ImmutablePutOutcome, SideEffectContext, VersionedPort};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ObjectOwner {
    pub issuer: String,
    pub subject: String,
    pub tenant_ref: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ObjectScope {
    pub object_type: String,
    pub object_id: String,
    pub owner: ObjectOwner,
    pub parent_type: Option<String>,
    pub parent_id: Option<String>,
}

pub trait ObjectAuthorizationPort: VersionedPort {
    fn bind(
        &self,
        scope: &ObjectScope,
        context: &SideEffectContext,
    ) -> Result<ImmutablePutOutcome, String>;

    fn scope(&self, object_type: &str, object_id: &str) -> Result<Option<ObjectScope>, String>;

    fn list_visible(
        &self,
        owner: &ObjectOwner,
        object_type: &str,
    ) -> Result<Vec<ObjectScope>, String>;
}
