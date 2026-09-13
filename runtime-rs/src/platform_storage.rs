// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use crate::bus::ConfigError;
pub use crate::bus::RuntimeProfileName;
use serde_json::{json, Value};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq)]
pub struct StorageHealth {
    pub ok: bool,
    pub adapter: String,
    pub profile: String,
    pub details: Value,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContextRepository {
    pub context: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepositoryBundle {
    broker: ContextRepository,
    execution: ContextRepository,
    journal: ContextRepository,
    marketdata: ContextRepository,
    plugins: ContextRepository,
    research: ContextRepository,
    risk: ContextRepository,
    strategy: ContextRepository,
}

impl RepositoryBundle {
    pub fn new() -> Self {
        Self {
            broker: repository("broker"),
            execution: repository("execution"),
            journal: repository("journal"),
            marketdata: repository("marketdata"),
            plugins: repository("plugins"),
            research: repository("research"),
            risk: repository("risk"),
            strategy: repository("strategy"),
        }
    }

    pub fn for_context(&self, context: &str) -> Result<&ContextRepository, String> {
        match context {
            "broker" => Ok(&self.broker),
            "execution" => Ok(&self.execution),
            "journal" => Ok(&self.journal),
            "marketdata" => Ok(&self.marketdata),
            "plugins" => Ok(&self.plugins),
            "research" => Ok(&self.research),
            "risk" => Ok(&self.risk),
            "strategy" => Ok(&self.strategy),
            _ => Err(format!("unknown repository context: {context}")),
        }
    }

    pub fn context_names(&self) -> Vec<&'static str> {
        vec![
            "broker",
            "execution",
            "journal",
            "marketdata",
            "plugins",
            "research",
            "risk",
            "strategy",
        ]
    }
}

impl Default for RepositoryBundle {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OltpStorageAdapter {
    path: Option<String>,
    name: String,
    profile: RuntimeProfileName,
    repositories: RepositoryBundle,
}

impl OltpStorageAdapter {
    pub fn health(&self) -> StorageHealth {
        StorageHealth {
            ok: true,
            adapter: self.name.clone(),
            profile: profile_name(self.profile).to_string(),
            details: json!({"path": self.path.clone().unwrap_or_else(|| ".tradeassembly/tradeassembly.db".to_string())}),
        }
    }

    pub fn repositories(&self) -> RepositoryBundle {
        self.repositories.clone()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct InMemoryOlapStorageAdapter {
    rows: BTreeMap<String, Vec<Value>>,
}

impl InMemoryOlapStorageAdapter {
    pub fn append_row(&mut self, model: &str, row: Value) {
        self.rows.entry(model.to_string()).or_default().push(row);
    }

    pub fn append_projection(&mut self, model: &str, row: Value) {
        self.append_row(model, row);
    }

    pub fn export_rows(&self, model: &str) -> Vec<Value> {
        self.rows.get(model).cloned().unwrap_or_default()
    }

    pub fn export_projection(&self, model: &str) -> Vec<Value> {
        self.export_rows(model)
    }

    pub fn health(&self) -> StorageHealth {
        StorageHealth {
            ok: true,
            adapter: "olap-memory".to_string(),
            profile: "olap".to_string(),
            details: json!({"models": self.rows.keys().cloned().collect::<Vec<_>>()}),
        }
    }
}

pub fn create_oltp_storage(
    path: Option<&str>,
    profile: RuntimeProfileName,
) -> Result<OltpStorageAdapter, ConfigError> {
    if profile == RuntimeProfileName::Local {
        return Ok(OltpStorageAdapter {
            path: path.map(ToString::to_string),
            name: "sqlite".to_string(),
            profile,
            repositories: RepositoryBundle::new(),
        });
    }
    if path.is_some() {
        return Err(configuration_error(
            "distributed profiles require external OLTP storage",
            json!({"profile": profile_name(profile), "path": path}),
        ));
    }
    Ok(OltpStorageAdapter {
        path: None,
        name: "postgres".to_string(),
        profile,
        repositories: RepositoryBundle::new(),
    })
}

pub fn create_olap_storage() -> InMemoryOlapStorageAdapter {
    InMemoryOlapStorageAdapter {
        rows: BTreeMap::new(),
    }
}

fn repository(context: &str) -> ContextRepository {
    ContextRepository {
        context: context.to_string(),
    }
}

fn profile_name(profile: RuntimeProfileName) -> &'static str {
    match profile {
        RuntimeProfileName::Local => "local",
        RuntimeProfileName::DistributedDev => "distributed-dev",
        RuntimeProfileName::Production => "production",
    }
}

fn configuration_error(message: impl Into<String>, details: Value) -> ConfigError {
    ConfigError {
        code: "configuration_error".to_string(),
        message: message.into(),
        details: redact_sensitive_details(details),
    }
}

fn redact_sensitive_details(value: Value) -> Value {
    match value {
        Value::String(text) => Value::String(redact_sensitive_string(&text)),
        Value::Array(items) => {
            Value::Array(items.into_iter().map(redact_sensitive_details).collect())
        }
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(key, value)| (key, redact_sensitive_details(value)))
                .collect(),
        ),
        other => other,
    }
}

fn redact_sensitive_string(value: &str) -> String {
    let lower = value.to_ascii_lowercase();
    if (lower.contains("://") && lower.contains('@'))
        || lower.contains("secret")
        || lower.contains("token")
        || lower.contains("password")
    {
        "***redacted***".to_string()
    } else {
        value.to_string()
    }
}
