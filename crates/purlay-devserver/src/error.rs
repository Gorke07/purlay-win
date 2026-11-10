//! The one error envelope: `{ "error": { "code", "message", "details"? } }`, its status
//! taken from the contract's `ERROR_STATUS` table.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use purlay_contract::constants::error_status;
use purlay_contract::models::{AppErrorBody, ErrorCode, ErrorResponse};
use serde_json::{Value, json};

#[derive(Debug, Clone)]
pub struct AppError {
    pub code: &'static str,
    pub message: String,
    pub details: Option<Value>,
}

pub type AppResult<T> = Result<T, AppError>;

impl AppError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        AppError { code, message: message.into(), details: None }
    }

    /// `400 VALIDATION_FAILED` naming one field.
    pub fn field(field: &str, why: &str) -> Self {
        AppError {
            code: "VALIDATION_FAILED",
            message: "The request is not valid.".into(),
            details: Some(json!({ "fields": { field: why } })),
        }
    }

    pub fn validation(message: impl Into<String>) -> Self {
        AppError::new("VALIDATION_FAILED", message)
    }

    pub fn unauthenticated() -> Self {
        AppError::new("UNAUTHENTICATED", "Sign in to continue.")
    }

    pub fn forbidden() -> Self {
        AppError::new("FORBIDDEN", "You are not allowed to do that.")
    }

    pub fn route_not_found() -> Self {
        AppError { code: "ROUTE_NOT_FOUND", message: "No such route.".into(), details: Some(json!({})) }
    }

    pub fn workspace_not_found() -> Self {
        AppError::new("WORKSPACE_NOT_FOUND", "No such workspace.")
    }

    pub fn channel_not_found() -> Self {
        AppError::new("CHANNEL_NOT_FOUND", "No such channel.")
    }

    pub fn message_not_found() -> Self {
        AppError::new("MESSAGE_NOT_FOUND", "No such message.")
    }

    pub fn not_a_member() -> Self {
        AppError::new("NOT_A_CHANNEL_MEMBER", "Join the channel first.")
    }

    pub fn status(&self) -> StatusCode {
        StatusCode::from_u16(error_status(self.code).unwrap_or(500)).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR)
    }

    pub fn body(&self) -> AppErrorBody {
        AppErrorBody { code: ErrorCode::parse(self.code), message: self.message.clone(), details: self.details.clone() }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = self.status();
        let body = serde_json::to_vec(&ErrorResponse { error: self.body() }).unwrap_or_default();
        (status, [(axum::http::header::CONTENT_TYPE, "application/json")], body).into_response()
    }
}
