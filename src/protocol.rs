use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u32 = 2;

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum BridgeRequest {
    UploadPath {
        version: u32,
        path: String,
    },
    ClipboardImage {
        version: u32,
    },
}

impl BridgeRequest {
    pub fn upload_path(path: impl Into<String>) -> Self {
        Self::UploadPath {
            version: PROTOCOL_VERSION,
            path: path.into(),
        }
    }

    pub fn clipboard_image() -> Self {
        Self::ClipboardImage {
            version: PROTOCOL_VERSION,
        }
    }

    pub fn version(&self) -> u32 {
        match self {
            Self::UploadPath { version, .. } | Self::ClipboardImage { version } => *version,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BridgeStatus {
    Success,
    NoClipboardImage,
    Error,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BridgeResponse {
    pub version: u32,
    pub status: BridgeStatus,
    pub remote_relative_path: Option<String>,
    pub error: Option<String>,
}

impl BridgeResponse {
    pub fn success(remote_relative_path: String) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            status: BridgeStatus::Success,
            remote_relative_path: Some(remote_relative_path),
            error: None,
        }
    }

    pub fn no_clipboard_image() -> Self {
        Self {
            version: PROTOCOL_VERSION,
            status: BridgeStatus::NoClipboardImage,
            remote_relative_path: None,
            error: None,
        }
    }

    pub fn failure(error: impl Into<String>) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            status: BridgeStatus::Error,
            remote_relative_path: None,
            error: Some(error.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_clipboard_request_with_explicit_operation() {
        let json = serde_json::to_string(&BridgeRequest::clipboard_image()).expect("serialize request");
        assert!(json.contains("\"op\":\"clipboard_image\""));
        assert!(json.contains("\"version\":2"));
    }

    #[test]
    fn no_image_response_is_distinct_from_failure() {
        let response = BridgeResponse::no_clipboard_image();
        assert_eq!(response.status, BridgeStatus::NoClipboardImage);
        assert!(response.error.is_none());
        assert!(response.remote_relative_path.is_none());
    }
}
