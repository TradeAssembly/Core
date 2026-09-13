// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthorityContext {
    pub actor: String,
    pub surface: String,
    pub account_mode: String,
}

impl AuthorityContext {
    pub fn local_cli() -> Self {
        Self {
            actor: "local-user".to_string(),
            surface: "cli".to_string(),
            account_mode: "paper".to_string(),
        }
    }
}
