//! Versioned private verification protocol and owned semantics observations.
mod artifact;
mod codec;
mod semantics;
pub use artifact::write_png;
pub use codec::encode_standard;
pub use semantics::*;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

pub const PROTOCOL_VERSION: u32 = 1;
pub const MAX_MESSAGE_BYTES: usize = 1024 * 1024;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub id: Value,
    pub method: String,
    #[serde(default)]
    pub params: Map<String, Value>,
}
impl Request {
    pub fn parse(bytes: &[u8]) -> Result<Self, AgentError> {
        if bytes.len() > MAX_MESSAGE_BYTES {
            return Err(AgentError::new("invalid_request", "message exceeds 1 MiB"));
        }
        let request: Self = serde_json::from_slice(bytes)
            .map_err(|e| AgentError::new("invalid_request", e.to_string()))?;
        if !(request.id.as_u64().is_some()
            || request
                .id
                .as_str()
                .is_some_and(|s| !s.is_empty() && s.len() <= 128))
            || request.method.is_empty()
            || request.method.len() > 128
        {
            return Err(AgentError::new(
                "invalid_request",
                "invalid request id or method",
            ));
        }
        Ok(request)
    }
    pub fn check_params(&self, allowed: &[&str]) -> Result<(), AgentError> {
        if let Some(key) = self
            .params
            .keys()
            .find(|key| key.as_str() != "timeoutMs" && !allowed.contains(&key.as_str()))
        {
            return Err(AgentError::new(
                "invalid_request",
                format!("unknown parameter {key}"),
            ));
        }
        Ok(())
    }
    pub fn view_id(&self) -> Result<i64, AgentError> {
        self.params
            .get("viewId")
            .and_then(Value::as_i64)
            .filter(|id| *id >= 0)
            .ok_or_else(|| {
                AgentError::new("invalid_request", "viewId must be a nonnegative integer")
            })
    }
    pub fn timeout_ms(&self) -> Result<u64, AgentError> {
        match self.params.get("timeoutMs") {
            None => Ok(5000),
            Some(value) => value
                .as_u64()
                .filter(|v| *v > 0 && *v <= 120_000)
                .ok_or_else(|| {
                    AgentError::new("invalid_request", "timeoutMs must be in 1..=120000")
                }),
        }
    }
    pub fn number(&self, key: &str) -> Result<f64, AgentError> {
        self.params
            .get(key)
            .and_then(Value::as_f64)
            .filter(|n| n.is_finite())
            .ok_or_else(|| AgentError::new("invalid_request", format!("{key} must be finite")))
    }
}
#[derive(Debug, Clone, Serialize)]
pub struct AgentError {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}
impl AgentError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            data: None,
        }
    }
    pub fn with_data(mut self, data: Value) -> Self {
        self.data = Some(data);
        self
    }
}
impl std::fmt::Display for AgentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for AgentError {}
pub fn response(id: Value, result: Result<Value, AgentError>) -> Value {
    match result {
        Ok(result) => json!({"id":id,"result":result}),
        Err(error) => json!({"id":id,"error":error}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn malformed_and_unknown_envelopes_are_rejected() {
        for input in [
            r#"{"id":-1,"method":"session.health"}"#,
            r#"{"id":1,"method":"x","extra":1}"#,
            r#"{"id":1,"method":"x","params":{"x":NaN}}"#,
        ] {
            assert!(Request::parse(input.as_bytes()).is_err());
        }
        let request = Request::parse(
            br#"{"id":7,"method":"input.tap","params":{"viewId":2,"x":4,"bad":true}}"#,
        )
        .unwrap();
        assert_eq!(request.view_id().unwrap(), 2);
        assert!(request.check_params(&["viewId", "x"]).is_err());
        assert!(Request::parse(&vec![b' '; MAX_MESSAGE_BYTES + 1]).is_err());
    }
}
