use crate::AgentError;
use serde_json::Value;

/// JSON-compatible subset of Flutter's StandardMessageCodec, for semantics
/// action arguments. Byte order matches supported little-endian native hosts.
pub fn encode_standard(value: &Value) -> Result<Vec<u8>, AgentError> {
    fn size(out: &mut Vec<u8>, length: usize) -> Result<(), AgentError> {
        if length < 254 {
            out.push(length as u8);
        } else if length <= u16::MAX as usize {
            out.push(254);
            out.extend_from_slice(&(length as u16).to_le_bytes());
        } else {
            out.push(255);
            out.extend_from_slice(
                &u32::try_from(length)
                    .map_err(|_| AgentError::new("invalid_request", "codec value too large"))?
                    .to_le_bytes(),
            );
        }
        Ok(())
    }
    fn align(out: &mut Vec<u8>, alignment: usize) {
        while !out.len().is_multiple_of(alignment) {
            out.push(0);
        }
    }
    fn encode(value: &Value, out: &mut Vec<u8>) -> Result<(), AgentError> {
        match value {
            Value::Null => out.push(0),
            Value::Bool(true) => out.push(1),
            Value::Bool(false) => out.push(2),
            Value::Number(value) => {
                if let Some(value) = value.as_i64() {
                    if let Ok(small) = i32::try_from(value) {
                        out.push(3);
                        align(out, 4);
                        out.extend_from_slice(&small.to_le_bytes());
                    } else {
                        out.push(4);
                        align(out, 8);
                        out.extend_from_slice(&value.to_le_bytes());
                    }
                } else if value.as_u64().is_some() {
                    return Err(AgentError::new(
                        "invalid_request",
                        "integer exceeds signed 64-bit codec range",
                    ));
                } else {
                    let value = value.as_f64().filter(|v| v.is_finite()).ok_or_else(|| {
                        AgentError::new("invalid_request", "invalid codec number")
                    })?;
                    out.push(6);
                    align(out, 8);
                    out.extend_from_slice(&value.to_le_bytes());
                }
            }
            Value::String(value) => {
                out.push(7);
                size(out, value.len())?;
                out.extend_from_slice(value.as_bytes());
            }
            Value::Array(values) => {
                out.push(12);
                size(out, values.len())?;
                for value in values {
                    encode(value, out)?;
                }
            }
            Value::Object(values) => {
                out.push(13);
                size(out, values.len())?;
                for (key, value) in values {
                    encode(&Value::String(key.clone()), out)?;
                    encode(value, out)?;
                }
            }
        }
        Ok(())
    }
    let mut output = Vec::new();
    encode(value, &mut output)?;
    Ok(output)
}
