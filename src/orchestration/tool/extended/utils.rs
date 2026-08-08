//! Utility tools: UUID generation, random tokens, encoding/decoding, file hashing.
//!
//! Provides simple utility operations that don't fit into other tool categories.

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use rand::RngExt;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::orchestration::tool::{Tool, ToolInput, ToolOutput};

// ── UUID Generator ──────────────────────────────────────────────────────────

/// Input parameters for [`UuidGenTool`].
#[derive(JsonSchema, Deserialize)]
struct UuidGenInput {}

pub struct UuidGenTool;

impl Tool for UuidGenTool {
    fn name(&self) -> &'static str {
        "uuid_gen"
    }

    fn description(&self) -> &str {
        "Generate a UUID v4 (random). Returns a universally unique identifier string."
    }

    fn input_schema(&self) -> Value {
        schemars::schema_for!(UuidGenInput).into()
    }

    fn run(&self, _input: &ToolInput) -> Result<ToolOutput> {
        Ok(ToolOutput {
            success: true,
            result: Some(json!({
                "uuid": uuid::Uuid::new_v4().to_string(),
            })),
            error: None,
            verification: None,
            audit_log: None,
            pua_report: None,
        })
    }
}

// ── Random Token Generator ──────────────────────────────────────────────────

/// Input parameters for [`RandomTokenTool`].
#[derive(JsonSchema, Deserialize)]
struct RandomTokenInput {
    /// Token length in characters (default: 32, range: 4–256).
    #[serde(default)]
    length: Option<u64>,
    /// Token format (default: hex). One of: hex, base64, alphanumeric.
    #[serde(default)]
    format: Option<String>,
}

pub struct RandomTokenTool;

impl Tool for RandomTokenTool {
    fn name(&self) -> &'static str {
        "random_token"
    }

    fn description(&self) -> &str {
        concat!(
            "Generate a cryptographically secure random token. ",
            "Supports: hex, base64, alphanumeric formats. Default: 32-char hex.",
        )
    }

    fn input_schema(&self) -> Value {
        schemars::schema_for!(RandomTokenInput).into()
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let params: RandomTokenInput = serde_json::from_value(input.payload.clone())
            .context("failed to deserialize random_token input")?;
        let length = params.length.unwrap_or(32).clamp(4, 256) as usize;
        let format = params.format.as_deref().unwrap_or("hex");

        let token = match format {
            "base64" => {
                let bytes: Vec<u8> = (0..length).map(|_| rand::rng().random::<u8>()).collect();
                use base64::Engine;
                base64::engine::general_purpose::STANDARD.encode(&bytes)
            }
            "alphanumeric" => {
                const CHARSET: &[u8] =
                    b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
                (0..length)
                    .map(|_| {
                        let idx = rand::rng().random_range(0..CHARSET.len());
                        CHARSET[idx] as char
                    })
                    .collect()
            }
            _ => {
                // hex
                (0..length)
                    .map(|_| format!("{:02x}", rand::rng().random::<u8>()))
                    .collect::<Vec<_>>()
                    .join("")
                    .chars()
                    .take(length)
                    .collect()
            }
        };

        Ok(ToolOutput {
            success: true,
            result: Some(json!({
                "token": token,
                "format": format,
                "length": token.len(),
            })),
            error: None,
            verification: None,
            audit_log: None,
            pua_report: None,
        })
    }
}

// ── Encode / Decode Tool ────────────────────────────────────────────────────

pub struct EncodeDecodeTool;

impl Tool for EncodeDecodeTool {
    fn name(&self) -> &'static str {
        "encode_decode"
    }

    fn description(&self) -> &str {
        concat!(
            "Encode or decode data using various formats. ",
            "Supports: base64, hex, url encoding/decoding. ",
            "Input text or binary data and choose the operation.",
        )
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "operation": {
                    "type": "string",
                    "enum": ["base64_encode", "base64_decode", "hex_encode", "hex_decode", "url_encode", "url_decode"],
                    "description": "Encoding/decoding operation"
                },
                "input": {
                    "type": "string",
                    "description": "Input text to encode or decode"
                }
            },
            "required": ["operation", "input"]
        })
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let operation = input.payload["operation"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("encode_decode requires arguments.operation"))?;
        let input_str = input.payload["input"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("encode_decode requires arguments.input"))?;

        let result = match operation {
            "base64_encode" => {
                use base64::Engine;
                base64::engine::general_purpose::STANDARD.encode(input_str.as_bytes())
            }
            "base64_decode" => {
                use base64::Engine;
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(input_str)
                    .context("Failed to decode base64 input")?;
                String::from_utf8(bytes).context("Base64 decoded data is not valid UTF-8")?
            }
            "hex_encode" => input_str
                .as_bytes()
                .iter()
                .map(|b| format!("{:02x}", b))
                .collect::<Vec<_>>()
                .join(""),
            "hex_decode" => {
                let cleaned: String = input_str
                    .chars()
                    .filter(|c| c.is_ascii_hexdigit())
                    .collect();
                if !cleaned.len().is_multiple_of(2) {
                    anyhow::bail!("Hex string must have an even number of hex digits");
                }
                let bytes: Vec<u8> = (0..cleaned.len())
                    .step_by(2)
                    .map(|i| u8::from_str_radix(&cleaned[i..i + 2], 16))
                    .collect::<Result<Vec<_>, _>>()
                    .context("Failed to decode hex string")?;
                String::from_utf8(bytes).context("Hex decoded data is not valid UTF-8")?
            }
            "url_encode" => url::form_urlencoded::byte_serialize(input_str.as_bytes()).collect(),
            "url_decode" => percent_decode_url(input_str).context(
                "Failed to URL-decode input: invalid UTF-8 or malformed percent encoding",
            )?,
            other => anyhow::bail!("Unsupported encode/decode operation '{}'", other),
        };

        Ok(ToolOutput {
            success: true,
            result: Some(json!({
                "operation": operation,
                "input": input_str,
                "output": result,
            })),
            error: None,
            verification: None,
            audit_log: None,
            pua_report: None,
        })
    }
}

/// Simple URL percent-decoding without external dependencies.
fn percent_decode_url(s: &str) -> Result<String> {
    let mut result = Vec::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        match c {
            '+' => result.push(b' '),
            '%' => {
                let hi = chars.next().and_then(|c| c.to_digit(16));
                let lo = chars.next().and_then(|c| c.to_digit(16));
                match (hi, lo) {
                    (Some(h), Some(l)) => result.push((h * 16 + l) as u8),
                    _ => anyhow::bail!("Invalid percent encoding at position {}", result.len()),
                }
            }
            c => {
                let mut buf = [0u8; 4];
                let encoded = c.encode_utf8(&mut buf);
                result.extend_from_slice(encoded.as_bytes());
            }
        }
    }
    String::from_utf8(result).context("URL-decoded result is not valid UTF-8")
}

// ── File Hasher Tool ────────────────────────────────────────────────────────

pub struct HashFileTool;

impl Tool for HashFileTool {
    fn name(&self) -> &'static str {
        "hash_file"
    }

    fn description(&self) -> &str {
        concat!(
            "Compute a cryptographic hash of a file. ",
            "Supports SHA-256 (default) and SHA-512. Returns the hash as a hex string.",
        )
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "File path to hash"},
                "algorithm": {
                    "type": "string",
                    "enum": ["sha256", "sha512"],
                    "description": "Hash algorithm (default: sha256)",
                    "default": "sha256"
                }
            },
            "required": ["path"]
        })
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let path = input.payload["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("hash_file requires arguments.path"))?;
        let path = PathBuf::from(path);
        let algorithm = input.payload["algorithm"].as_str().unwrap_or("sha256");

        let data =
            fs::read(&path).with_context(|| format!("Failed to read file '{}'", path.display()))?;

        let hash = match algorithm {
            "sha512" => {
                use sha2::{Digest, Sha512};
                let mut hasher = Sha512::new();
                hasher.update(&data);
                hasher
                    .finalize()
                    .iter()
                    .map(|b| format!("{:02x}", b))
                    .collect::<Vec<_>>()
                    .join("")
            }
            "sha256" => crate::shared::sha256_hex(&data),
            // Unknown algorithms are rejected instead of silently returning a
            // sha256 digest labelled with the requested algorithm name.
            other => {
                anyhow::bail!(
                    "hash_file: unsupported algorithm '{}' (expected sha256 or sha512)",
                    other
                );
            }
        };

        let file_size = data.len();

        Ok(ToolOutput {
            success: true,
            result: Some(json!({
                "path": path.to_string_lossy(),
                "algorithm": algorithm,
                "hash": hash,
                "file_size": file_size,
            })),
            error: None,
            verification: None,
            audit_log: None,
            pua_report: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestration::tool::ToolInput;

    fn tool_input(payload: serde_json::Value) -> ToolInput {
        ToolInput {
            task_id: "test-utils".to_string(),
            phase: "act".to_string(),
            agent_role: "coder".to_string(),
            objective: "test".to_string(),
            constraints: None,
            evidence: None,
            payload,
            allowed_base_dir: None,
        }
    }

    #[test]
    fn uuid_gen_returns_valid_uuid() {
        let tool = UuidGenTool;
        let input = tool_input(json!({}));
        let output = tool.run(&input).expect("uuid_gen should succeed");
        assert!(output.success);
        let result = output.result.unwrap();
        let uuid_str = result["uuid"].as_str().unwrap();
        // UUID v4 format: 8-4-4-4-12 hex digits with version nibble 4
        assert_eq!(uuid_str.len(), 36);
        assert_eq!(&uuid_str[14..15], "4");
    }

    #[test]
    fn random_token_default_is_hex() {
        let tool = RandomTokenTool;
        let input = tool_input(json!({}));
        let output = tool.run(&input).expect("random_token should succeed");
        assert!(output.success);
        let result = output.result.unwrap();
        assert_eq!(result["format"].as_str().unwrap(), "hex");
        assert_eq!(result["length"].as_u64().unwrap(), 32);
        let token = result["token"].as_str().unwrap();
        assert_eq!(token.len(), 32);
        assert!(token.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn random_token_alphanumeric() {
        let tool = RandomTokenTool;
        let input = tool_input(json!({ "format": "alphanumeric", "length": 16 }));
        let output = tool.run(&input).expect("random_token should succeed");
        assert!(output.success);
        let result = output.result.unwrap();
        let token = result["token"].as_str().unwrap();
        assert_eq!(token.len(), 16);
        assert!(token.chars().all(|c| c.is_alphanumeric()));
    }

    #[test]
    fn encode_decode_base64_roundtrip() {
        let tool = EncodeDecodeTool;
        let original = "hello world";

        let input_enc = tool_input(json!({
            "operation": "base64_encode",
            "input": original,
        }));
        let enc_output = tool.run(&input_enc).expect("encode should succeed");
        let encoded = enc_output.result.unwrap()["output"]
            .as_str()
            .unwrap()
            .to_string();

        let input_dec = tool_input(json!({
            "operation": "base64_decode",
            "input": encoded,
        }));
        let dec_output = tool.run(&input_dec).expect("decode should succeed");
        let decoded = dec_output.result.unwrap()["output"]
            .as_str()
            .unwrap()
            .to_string();

        assert_eq!(decoded, original);
    }

    #[test]
    fn encode_decode_hex_roundtrip() {
        let tool = EncodeDecodeTool;
        let original = "test data";

        let input_enc = tool_input(json!({
            "operation": "hex_encode",
            "input": original,
        }));
        let enc_output = tool.run(&input_enc).expect("encode should succeed");
        let encoded = enc_output.result.unwrap()["output"]
            .as_str()
            .unwrap()
            .to_string();

        let input_dec = tool_input(json!({
            "operation": "hex_decode",
            "input": encoded,
        }));
        let dec_output = tool.run(&input_dec).expect("decode should succeed");
        let decoded = dec_output.result.unwrap()["output"]
            .as_str()
            .unwrap()
            .to_string();

        assert_eq!(decoded, original);
    }

    #[test]
    fn encode_decode_url_roundtrip() {
        let tool = EncodeDecodeTool;
        let original = "hello world & more!";

        let input_enc = tool_input(json!({
            "operation": "url_encode",
            "input": original,
        }));
        let enc_output = tool.run(&input_enc).expect("encode should succeed");
        let encoded = enc_output.result.unwrap()["output"]
            .as_str()
            .unwrap()
            .to_string();

        let input_dec = tool_input(json!({
            "operation": "url_decode",
            "input": encoded,
        }));
        let dec_output = tool.run(&input_dec).expect("decode should succeed");
        let decoded = dec_output.result.unwrap()["output"]
            .as_str()
            .unwrap()
            .to_string();

        assert_eq!(decoded, original);
    }

    #[test]
    fn hash_file_requires_path() {
        let tool = HashFileTool;
        let input = tool_input(json!({}));
        let result = tool.run(&input);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("requires arguments.path"));
    }
}
