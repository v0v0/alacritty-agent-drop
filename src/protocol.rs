use std::io::{BufRead, Write};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

pub const PROTOCOL_VERSION: u32 = 3;
pub const MAX_HEADER: u64 = 64 * 1024;
pub const MAX_FILE_SIZE: u64 = 256 * 1024 * 1024;

#[derive(Debug, Serialize, Deserialize)]
pub struct BridgeRequest {
    pub version: u32,
    pub token: String,
    #[serde(flatten)]
    pub operation: Operation,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Operation {
    UploadPath { path: String },
    ClipboardImage,
}

impl BridgeRequest {
    pub fn new(token: String, operation: Operation) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            token,
            operation,
        }
    }
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Outcome {
    File { name: String, size: u64 },
    NoClipboardImage,
    Error { message: String },
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BridgeResponse {
    pub version: u32,
    #[serde(flatten)]
    pub outcome: Outcome,
}

impl BridgeResponse {
    pub fn new(outcome: Outcome) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            outcome,
        }
    }
}

// Bound allocation before parsing, and retain BufReader for the following binary payload.
pub fn read_json<R: BufRead, T: DeserializeOwned>(reader: &mut R) -> Result<T> {
    let mut bytes = Vec::new();
    let count = std::io::Read::take(reader, MAX_HEADER + 1).read_until(b'\n', &mut bytes)?;
    if count == 0 || count as u64 > MAX_HEADER || bytes.last() != Some(&b'\n') {
        bail!("missing, oversized or incomplete bridge header");
    }
    serde_json::from_slice(&bytes).context("invalid bridge header")
}

pub fn write_json<W: Write, T: Serialize>(writer: &mut W, value: &T) -> Result<()> {
    let bytes = serde_json::to_vec(value)?;
    if bytes.len() as u64 + 1 > MAX_HEADER {
        bail!("bridge header is too large");
    }
    writer.write_all(&bytes)?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufReader, Cursor, Read};

    #[test]
    fn header_keeps_binary_payload_and_enforces_bounds() {
        let mut bytes = Vec::new();
        write_json(
            &mut bytes,
            &BridgeResponse::new(Outcome::File {
                name: "图片.png".into(),
                size: 4,
            }),
        )
        .unwrap();
        bytes.extend_from_slice(b"\0\n\xffx");
        let mut reader = BufReader::new(Cursor::new(bytes));
        let response: BridgeResponse = read_json(&mut reader).unwrap();
        assert!(matches!(response.outcome, Outcome::File { size: 4, .. }));
        let mut payload = Vec::new();
        reader.read_to_end(&mut payload).unwrap();
        assert_eq!(payload, b"\0\n\xffx");
        assert!(
            read_json::<_, BridgeRequest>(&mut Cursor::new(vec![b'a'; MAX_HEADER as usize + 2]))
                .is_err()
        );
        assert!(read_json::<_, BridgeRequest>(&mut Cursor::new(b"{}")).is_err());
    }
}
