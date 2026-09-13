// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use crate::ports::{
    FailureMode, ImmutablePutOutcome, ObjectAuthorizationPort, ObjectOwner, ObjectScope,
    PortDescriptor, PortKind, SideEffectContext, StoragePort, VersionedPort,
};
use std::sync::Arc;

const SCOPES_NS: &str = "object_authorization_scopes_v1";

pub struct StorageObjectAuthorization {
    storage: Arc<dyn StoragePort>,
}

impl StorageObjectAuthorization {
    pub fn new(storage: Arc<dyn StoragePort>) -> Self {
        Self { storage }
    }
}

impl VersionedPort for StorageObjectAuthorization {
    fn descriptors(&self) -> Vec<PortDescriptor> {
        let mut descriptor = PortDescriptor::new(
            PortKind::Storage,
            format!("{}.object-authorization", self.storage.adapter_name()),
        )
        .for_profiles(&["local", "self_hosted"])
        .with_capabilities(&[
            "object.scope.immutable",
            "object.scope.owner_qualified",
            "object.scope.non_enumerating",
        ]);
        descriptor.failure_mode = FailureMode::FailClosed;
        vec![descriptor]
    }
}

impl ObjectAuthorizationPort for StorageObjectAuthorization {
    fn bind(
        &self,
        scope: &ObjectScope,
        context: &SideEffectContext,
    ) -> Result<ImmutablePutOutcome, String> {
        validate_scope(scope)?;
        let key = scope_key(&scope.object_type, &scope.object_id);
        let value = serde_json::to_value(scope)
            .map_err(|_| "object_scope_serialization_failed".to_string())?;
        match self
            .storage
            .put_json_if_absent(SCOPES_NS, &key, value, context)
        {
            Ok(ImmutablePutOutcome::Created) => Ok(ImmutablePutOutcome::Created),
            Ok(ImmutablePutOutcome::AlreadyPresent) => {
                match self.scope(&scope.object_type, &scope.object_id)? {
                    Some(existing) if existing == *scope => Ok(ImmutablePutOutcome::AlreadyPresent),
                    Some(_) => Err("object_scope_conflict".to_string()),
                    None => Err("object_scope_integrity_failed".to_string()),
                }
            }
            Err(error) if error == "immutable_storage_conflict" => {
                match self.scope(&scope.object_type, &scope.object_id)? {
                    Some(existing) if existing == *scope => Ok(ImmutablePutOutcome::AlreadyPresent),
                    Some(_) => Err("object_scope_conflict".to_string()),
                    None => Err(error),
                }
            }
            Err(error) => Err(error),
        }
    }

    fn scope(&self, object_type: &str, object_id: &str) -> Result<Option<ObjectScope>, String> {
        self.storage
            .get_json(SCOPES_NS, &scope_key(object_type, object_id))?
            .map(|value| {
                let scope: ObjectScope = serde_json::from_value(value)
                    .map_err(|_| "object_scope_integrity_failed".to_string())?;
                validate_scope(&scope)?;
                if scope.object_type != object_type || scope.object_id != object_id {
                    return Err("object_scope_integrity_failed".to_string());
                }
                Ok::<ObjectScope, String>(scope)
            })
            .transpose()
    }

    fn list_visible(
        &self,
        owner: &ObjectOwner,
        object_type: &str,
    ) -> Result<Vec<ObjectScope>, String> {
        let mut visible = self
            .storage
            .list_json(SCOPES_NS)?
            .into_iter()
            .map(|(_, value)| {
                let scope: ObjectScope = serde_json::from_value(value)
                    .map_err(|_| "object_scope_integrity_failed".to_string())?;
                validate_scope(&scope)?;
                Ok::<ObjectScope, String>(scope)
            })
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .filter(|scope| scope.object_type == object_type && scope.owner == *owner)
            .collect::<Vec<_>>();
        visible.sort_by(|left, right| left.object_id.cmp(&right.object_id));
        Ok(visible)
    }
}

fn scope_key(object_type: &str, object_id: &str) -> String {
    format!("{object_type}:{object_id}")
}

fn validate_scope(scope: &ObjectScope) -> Result<(), String> {
    if [
        scope.object_type.as_str(),
        scope.object_id.as_str(),
        scope.owner.issuer.as_str(),
        scope.owner.subject.as_str(),
        scope.owner.tenant_ref.as_str(),
    ]
    .iter()
    .any(|value| value.trim().is_empty())
    {
        return Err("object_scope_invalid".to_string());
    }
    if scope.parent_type.is_some() != scope.parent_id.is_some() {
        return Err("object_scope_invalid".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::local::sqlite::LocalSqliteStorage;
    use crate::ports::{AuthorityContext, IdempotencyKey};

    fn context() -> SideEffectContext {
        SideEffectContext::new(
            AuthorityContext::local_cli(),
            IdempotencyKey::new("object-scope:test").expect("valid idempotency key"),
        )
    }

    fn owner(subject: &str) -> ObjectOwner {
        ObjectOwner {
            issuer: "https://issuer.example".to_string(),
            subject: subject.to_string(),
            tenant_ref: format!("personal:{subject}"),
        }
    }

    #[test]
    fn immutable_scope_is_owner_qualified() {
        let db = tempfile::Builder::new()
            .prefix("tradeassembly-object-auth-")
            .suffix(".db")
            .tempfile()
            .expect("temporary db");
        let storage: Arc<dyn StoragePort> =
            Arc::new(LocalSqliteStorage::new(db.path().to_string_lossy()));
        let adapter = StorageObjectAuthorization::new(storage);
        let scope = ObjectScope {
            object_type: "strategy".to_string(),
            object_id: "strat_a".to_string(),
            owner: owner("alice"),
            parent_type: None,
            parent_id: None,
        };

        assert_eq!(
            adapter.bind(&scope, &context()).expect("bind"),
            ImmutablePutOutcome::Created
        );
        assert_eq!(
            adapter.bind(&scope, &context()).expect("idempotent bind"),
            ImmutablePutOutcome::AlreadyPresent
        );
        assert_eq!(
            adapter
                .list_visible(&owner("alice"), "strategy")
                .expect("alice list"),
            vec![scope.clone()]
        );
        assert!(adapter
            .list_visible(&owner("bob"), "strategy")
            .expect("bob list")
            .is_empty());

        let mut conflicting = scope;
        conflicting.owner = owner("bob");
        assert_eq!(
            adapter.bind(&conflicting, &context()).unwrap_err(),
            "object_scope_conflict"
        );
    }
}
