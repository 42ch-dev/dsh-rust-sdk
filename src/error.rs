use serde_json::Value;

/// Typed error taxonomy for the DeepSeek Harness SDK.
///
/// Maps to the reference clients' error surfaces: Python's
/// `HarnessError` / `TransportClosedError` / `SdkProtocolError` /
/// `JsonRpcError` and the TS client's transport and protocol errors.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The runtime process is not running, or its stdio transport closed
    /// unexpectedly. The payload carries diagnostics (exit status and the
    /// captured stderr tail where available).
    #[error("DeepSeek Harness runtime is not running: {0}")]
    TransportClosed(String),

    /// A request did not get a response within the configured timeout.
    #[error("{method} timed out waiting for DeepSeek Harness runtime: {source}")]
    RequestTimeout {
        /// The JSON-RPC method that timed out.
        method: String,
        /// The underlying timeout error.
        #[source]
        source: tokio::time::error::Elapsed,
    },

    /// A protocol-level violation: the runtime's behavior contradicts the
    /// documented wire protocol. Used for missing server identity, missing
    /// `messageId`, `finish_reason` extraction failures, and similar cases —
    /// kept as a structured variant instead of ad-hoc strings.
    #[error("SDK protocol error: {message}")]
    SdkProtocol { message: String },

    /// A JSON-RPC error response, preserving `code` and optional `data`.
    #[error("JSON-RPC error {code:?}: {message}")]
    JsonRpc {
        /// The JSON-RPC error code, when present.
        code: Option<i64>,
        /// The error message.
        message: String,
        /// Optional structured error payload from the server.
        data: Option<Value>,
    },

    /// The runtime binary is missing or not launchable.
    #[error("runtime is missing or not launchable: {0}")]
    RuntimeNotFound(String),

    /// An I/O error (spawn, stdio, transport).
    #[error(transparent)]
    Io(#[from] std::io::Error),

    /// A JSON serialization or deserialization error.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

impl Error {
    /// Returns `true` when the error is a protocol violation
    /// ([`Error::SdkProtocol`]).
    pub fn is_protocol(&self) -> bool {
        matches!(self, Error::SdkProtocol { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::Error;
    use serde_json::{json, Value};
    use std::time::Duration;

    /// Obtains a real `tokio::time::error::Elapsed` — the type has no public
    /// constructor, so the only way to build it is to let a timeout expire.
    async fn elapsed() -> tokio::time::error::Elapsed {
        tokio::time::timeout(Duration::from_millis(5), std::future::pending::<()>())
            .await
            .unwrap_err()
    }

    #[test]
    fn display_transport_closed() {
        let err = Error::TransportClosed("child exited with status 1".into());
        assert_eq!(
            err.to_string(),
            "DeepSeek Harness runtime is not running: child exited with status 1"
        );
    }

    #[tokio::test]
    async fn display_request_timeout() {
        let err = Error::RequestTimeout {
            method: "initialize".into(),
            source: elapsed().await,
        };
        assert_eq!(
            err.to_string(),
            "initialize timed out waiting for DeepSeek Harness runtime: deadline has elapsed"
        );
    }

    #[test]
    fn display_sdk_protocol() {
        let err = Error::SdkProtocol {
            message: "turn/end event requires a string data.reason.kind".into(),
        };
        assert_eq!(
            err.to_string(),
            "SDK protocol error: turn/end event requires a string data.reason.kind"
        );
    }

    #[test]
    fn display_json_rpc() {
        let err = Error::JsonRpc {
            code: Some(-32601),
            message: "method not found".into(),
            data: None,
        };
        assert_eq!(
            err.to_string(),
            "JSON-RPC error Some(-32601): method not found"
        );
    }

    #[test]
    fn display_runtime_not_found() {
        let err = Error::RuntimeNotFound("no dsh runtime on PATH".into());
        assert_eq!(
            err.to_string(),
            "runtime is missing or not launchable: no dsh runtime on PATH"
        );
    }

    #[test]
    fn display_io_is_transparent() {
        let source = std::io::Error::new(std::io::ErrorKind::NotFound, "boom");
        let err = Error::Io(source);
        assert_eq!(err.to_string(), "boom");
    }

    #[test]
    fn display_json_is_transparent() {
        let source: serde_json::Error = serde_json::from_str::<Value>("not json").unwrap_err();
        let expected = source.to_string();
        let err = Error::Json(source);
        assert_eq!(err.to_string(), expected);
    }

    #[tokio::test]
    async fn is_protocol_true_only_for_sdk_protocol() {
        assert!(Error::SdkProtocol {
            message: "x".into()
        }
        .is_protocol());
        assert!(!Error::TransportClosed("x".into()).is_protocol());
        assert!(!Error::RequestTimeout {
            method: "m".into(),
            source: elapsed().await,
        }
        .is_protocol());
        assert!(!Error::JsonRpc {
            code: None,
            message: "m".into(),
            data: None,
        }
        .is_protocol());
        assert!(!Error::RuntimeNotFound("x".into()).is_protocol());
        assert!(!Error::Io(std::io::Error::other("io")).is_protocol());
        assert!(!Error::Json(serde_json::from_str::<Value>("x").unwrap_err()).is_protocol());
    }

    #[test]
    fn from_io_error() {
        let io = std::io::Error::new(std::io::ErrorKind::NotFound, "missing file");
        let err = Error::from(io);
        assert!(matches!(err, Error::Io(_)));
    }

    #[test]
    fn from_json_error() {
        let source: serde_json::Error = serde_json::from_str::<Value>("{").unwrap_err();
        let err = Error::from(source);
        assert!(matches!(err, Error::Json(_)));
    }

    #[test]
    fn json_rpc_round_trips_code_and_data() {
        let err = Error::JsonRpc {
            code: Some(-32000),
            message: "Server error".into(),
            data: Some(json!({ "reason": "engine unavailable" })),
        };
        match err {
            Error::JsonRpc {
                code,
                message,
                data,
            } => {
                assert_eq!(code, Some(-32000));
                assert_eq!(message, "Server error");
                assert_eq!(data, Some(json!({ "reason": "engine unavailable" })));
            }
            other => panic!("expected JsonRpc variant, got {other:?}"),
        }
    }
}
