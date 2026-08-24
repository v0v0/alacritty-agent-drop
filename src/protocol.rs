use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u32 = 1;

#[derive(Debug, Serialize, Deserialize)]
pub struct UploadRequest {
    pub version: u32,
    pub path: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UploadResponse {
    pub version: u32,
    pub remote_relative_path: Option<String>,
    pub error: Option<String>,
}

impl UploadResponse {
    pub fn success(remote_relative_path: String) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            remote_relative_path: Some(remote_relative_path),
            error: None,
        }
    }

    pub fn failure(error: impl Into<String>) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            remote_relative_path: None,
            error: Some(error.into()),
        }
    }
}
