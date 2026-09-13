use super::{plugin_lifecycle, TradeAssemblyService};
use serde_json::{json, Value};

pub(crate) fn store(service: &TradeAssemblyService, provider_ref: &str, body: Value) -> Value {
    if let Err(response) = plugin_lifecycle::require_instance(service, provider_ref) {
        return failure(provider_ref, "credential_store_failed", response);
    }
    let fields = body.as_object().cloned().unwrap_or_default();
    let fields =
        match plugin_lifecycle::partition_compatibility_fields(service, provider_ref, &fields) {
            Ok(fields) => fields,
            Err(message) => {
                return json!({
                    "ok": false,
                    "providerRef": provider_ref,
                    "error": {
                        "code": "credential_store_failed",
                        "message": message,
                    },
                });
            }
        };
    if !fields.configuration.is_empty() {
        let configured = plugin_lifecycle::configure_instance(
            service,
            provider_ref,
            json!({"configuration": fields.configuration}),
        );
        if configured.status != 200 {
            return json!({
                "ok": false,
                "providerRef": provider_ref,
                "error": configured.body.get("error").cloned().unwrap_or_else(|| json!({
                    "code": "credential_store_failed",
                    "message": "plugin configuration could not be updated"
                })),
            });
        }
    }
    let stored = plugin_lifecycle::store_credentials(
        service,
        provider_ref,
        json!({"credentials": Value::Object(fields.credentials)}),
    );
    if stored.status == 200 {
        let status = &stored.body["credentialStatus"];
        json!({
            "provider_ref": provider_ref,
            "providerRef": provider_ref,
            "stored": true,
            "configured": status["configured"],
            "custody": status["custody"],
            "redactedDisplay": status["redactedDisplay"],
            "revisionRef": status["revisionRef"],
        })
    } else {
        json!({
            "ok": false,
            "providerRef": provider_ref,
            "error": stored.body.get("error").cloned().unwrap_or_else(|| json!({
                "code": "credential_store_failed",
                "message": "plugin credentials could not be stored"
            })),
        })
    }
}

pub(crate) fn status(service: &TradeAssemblyService, provider_ref: &str) -> Value {
    match plugin_lifecycle::credential_status_for(service, provider_ref) {
        response if response.status == 200 => {
            let status = &response.body["credentialStatus"];
            json!({
                "provider_ref": provider_ref,
                "providerRef": provider_ref,
                "configured": status["configured"],
                "custody": status["custody"],
                "redactedDisplay": status["redactedDisplay"],
                "revisionRef": status["revisionRef"],
            })
        }
        _ => json!({
            "ok": false,
            "error": {"code": "object_not_available", "message": "object is not available"},
        }),
    }
}

pub(crate) fn test(service: &TradeAssemblyService, provider_ref: &str, body: Value) -> Value {
    let _ = body;
    let status = status(service, provider_ref);
    if status["ok"] == false {
        if status["error"]["code"] == "object_not_available" && service.invocation_owner().is_none()
        {
            return json!({
                "provider_ref": provider_ref,
                "providerRef": provider_ref,
                "ok": false,
                "configured": false,
                "connectivityChecked": false,
                "message": "credentials not configured",
            });
        }
        return json!({"ok": false, "error": status["error"]});
    }
    let configured = status["configured"].as_bool().unwrap_or(false);
    json!({
        "provider_ref": provider_ref,
        "providerRef": provider_ref,
        "ok": configured,
        "configured": status["configured"],
        "connectivityChecked": false,
        "message": if configured { "credential prerequisites are available" } else { "credentials not configured" },
    })
}

pub(crate) fn revoke(service: &TradeAssemblyService, provider_ref: &str) -> Value {
    let revoked = plugin_lifecycle::revoke_credentials(service, provider_ref);
    if revoked.status == 200 {
        json!({
            "provider_ref": provider_ref,
            "providerRef": provider_ref,
            "revoked": revoked.body["revoked"],
        })
    } else {
        json!({
            "ok": false,
            "providerRef": provider_ref,
            "error": revoked.body.get("error").cloned().unwrap_or_else(|| json!({
                "code": "credential_revoke_failed",
                "message": "plugin credentials could not be revoked"
            })),
        })
    }
}

fn failure(provider_ref: &str, code: &str, response: super::ServiceResponse) -> Value {
    let error = if response.status == 404 {
        json!({"code": "object_not_available", "message": "object is not available"})
    } else {
        response.body.get("error").cloned().unwrap_or_else(|| {
            json!({
                "code": code,
                "message": "plugin credential access failed"
            })
        })
    };
    json!({
        "ok": false,
        "providerRef": provider_ref,
        "error": error,
    })
}

pub(crate) fn oauth_metadata(provider_ref: &str) -> Value {
    json!({
        "provider_ref": provider_ref,
        "providerRef": provider_ref,
        "configured": false,
        "hostedCredentials": false,
        "custody": "local/customer-managed",
    })
}
