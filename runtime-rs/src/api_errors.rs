// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ErrorCode {
    StaleMarketData,
    ProviderUnavailable,
    StrategyNotFound,
    NotFound,
    ValidationFailed,
}

impl ErrorCode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::StaleMarketData => "stale_market_data",
            Self::ProviderUnavailable => "provider_unavailable",
            Self::StrategyNotFound => "strategy_not_found",
            Self::NotFound => "not_found",
            Self::ValidationFailed => "validation_failed",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ErrorObject {
    pub code: String,
    pub message: String,
    pub safe_to_continue: bool,
    pub retryable: bool,
    pub details: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ApiErrorResponse {
    pub status_code: u16,
    pub detail: Value,
    pub error: ErrorObject,
}

impl ApiErrorResponse {
    pub fn tradeassembly_error(code: ErrorCode, message: &str, details: Value) -> Self {
        let status_code = match code {
            ErrorCode::StaleMarketData => 409,
            ErrorCode::ProviderUnavailable => 502,
            ErrorCode::StrategyNotFound => 404,
            ErrorCode::NotFound => 404,
            ErrorCode::ValidationFailed => 400,
        };
        let retryable = matches!(code, ErrorCode::ProviderUnavailable);
        let safe_to_continue = !matches!(
            code,
            ErrorCode::StaleMarketData | ErrorCode::ProviderUnavailable
        );
        let error = ErrorObject {
            code: code.as_str().to_string(),
            message: message.to_string(),
            safe_to_continue,
            retryable,
            details,
        };
        Self {
            status_code,
            detail: serde_json::to_value(&error).expect("error serializes"),
            error,
        }
    }

    pub fn broker_unavailable(message: &str, details: Value) -> Self {
        Self::tradeassembly_error(ErrorCode::ProviderUnavailable, message, details)
    }

    pub fn http_exception(status_code: u16, detail: &str) -> Self {
        let code = match (status_code, detail) {
            (404, "Not Found") => ErrorCode::NotFound,
            (_, "strategy_not_found") => ErrorCode::StrategyNotFound,
            _ => ErrorCode::ValidationFailed,
        };
        let error = ErrorObject {
            code: code.as_str().to_string(),
            message: detail.to_string(),
            safe_to_continue: true,
            retryable: false,
            details: json!({}),
        };
        Self {
            status_code,
            detail: json!(detail),
            error,
        }
    }

    pub fn validation_failed(detail: Value) -> Self {
        let error = ErrorObject {
            code: ErrorCode::ValidationFailed.as_str().to_string(),
            message: "validation_failed".to_string(),
            safe_to_continue: true,
            retryable: false,
            details: json!({}),
        };
        Self {
            status_code: 400,
            detail,
            error,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_runtime_errors_are_machine_readable_and_fail_closed() {
        let response = ApiErrorResponse::tradeassembly_error(
            ErrorCode::StaleMarketData,
            "snapshot is stale",
            json!({"symbol": "BTC/USD"}),
        );

        assert_eq!(response.status_code, 409);
        assert_eq!(
            response.detail,
            serde_json::to_value(&response.error).expect("error serializes")
        );
        assert_eq!(response.error.code, "stale_market_data");
        assert!(!response.error.safe_to_continue);
        assert_eq!(response.error.details, json!({"symbol": "BTC/USD"}));
    }

    #[test]
    fn api_broker_errors_keep_retry_and_provider_context() {
        let response = ApiErrorResponse::broker_unavailable(
            "alpaca unavailable",
            json!({"provider_ref": "alpaca-paper"}),
        );

        assert_eq!(response.status_code, 502);
        assert_eq!(response.error.code, "provider_unavailable");
        assert!(response.error.retryable);
        assert_eq!(
            response.error.details,
            json!({"provider_ref": "alpaca-paper"})
        );
    }

    #[test]
    fn api_http_exception_handler_preserves_legacy_detail_and_adds_error() {
        let response = ApiErrorResponse::http_exception(404, "strategy_not_found");

        assert_eq!(response.status_code, 404);
        assert_eq!(response.detail, json!("strategy_not_found"));
        assert_eq!(response.error.code, "strategy_not_found");
        assert!(response.error.safe_to_continue);
    }

    #[test]
    fn api_not_found_errors_are_structured() {
        let response = ApiErrorResponse::http_exception(404, "Not Found");

        assert_eq!(response.status_code, 404);
        assert_eq!(response.detail, json!("Not Found"));
        assert_eq!(response.error.code, "not_found");
        assert!(response.error.safe_to_continue);
    }

    #[test]
    fn api_request_validation_errors_add_stable_error_object() {
        let detail = json!([{"loc": ["body", "symbols"], "msg": "expected list"}]);
        let response = ApiErrorResponse::validation_failed(detail.clone());

        assert_eq!(response.status_code, 400);
        assert_eq!(response.detail, detail);
        assert_eq!(response.error.code, "validation_failed");
        assert!(response.error.safe_to_continue);
    }
}
