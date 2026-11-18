//! Error type returned to the webview.
//!
//! The front end maps `code` (plus `params`) to a French message. Technical details
//! never contain user content (no secrets in errors or logs).

use serde::Serialize;
use serde_json::{json, Value};

#[derive(Debug, Clone, PartialEq, Serialize, thiserror::Error)]
#[error("{code}")]
pub struct AppError {
    pub code: String,
    pub params: Value,
}

impl AppError {
    pub fn new(code: &str) -> Self {
        Self {
            code: code.to_string(),
            params: json!({}),
        }
    }

    pub fn with(code: &str, params: Value) -> Self {
        Self {
            code: code.to_string(),
            params,
        }
    }
}

pub type AppResult<T> = Result<T, AppError>;

impl From<crate::core_api::CoreError> for AppError {
    fn from(e: crate::core_api::CoreError) -> Self {
        use crate::core_api::CoreError;
        match e {
            CoreError::NotFound(_) => AppError::new("error.not_found"),
            CoreError::Offline => AppError::new("error.offline"),
            // Refusing to sign a pin time beyond 2^53 - 1 ms: the clock of this PC is
            // wrong (`spec/pin-contract.md` §2).
            CoreError::Rejected(code) if code == "malformed_statement" => {
                AppError::new("error.clock")
            }
            // Device management (Appareils): each of these has its own sentence.
            CoreError::Rejected(code) if code == "name_invalid" => {
                AppError::new("error.device_name_invalid")
            }
            CoreError::Rejected(code) if code == "unknown_device" => {
                AppError::new("error.unknown_device")
            }
            CoreError::Rejected(code) if code == "revoke_self" => {
                AppError::new("error.revoke_self")
            }
            // The devices or the key of the space moved while the key was being renewed.
            CoreError::Rejected(code)
                if code == "device_set_mismatch" || code == "epoch_conflict" =>
            {
                AppError::new("error.devices_changed")
            }
            CoreError::Rejected(_) => AppError::new("error.rejected"),
            CoreError::NotPaired => AppError::new("error.not_paired"),
            CoreError::Blocked(_) => AppError::new("error.blocked"),
            CoreError::Storage(_) => AppError::new("error.storage"),
            CoreError::Pairing(code) => AppError::new(&format!("error.pairing_{code}")),
            // This core offers no departure (no engine): nothing was sent or erased.
            CoreError::DepartureUnavailable => AppError::new("error.departure_unavailable"),
            CoreError::Other(_) => AppError::new("error.core"),
        }
    }
}

impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        AppError::with("error.io", json!({ "kind": format!("{:?}", e.kind()) }))
    }
}
