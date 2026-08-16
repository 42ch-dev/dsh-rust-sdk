//! # deepseek-harness-sdk
//!
//! Low-level Rust client for the official
//! [DeepSeek Harness](https://github.com/deepseek-ai/deepseek-harness)
//! runtime: typed errors, wire-protocol types, a line transport, and a
//! `HarnessClient` that spawns the runtime process and speaks its stdio
//! JSON-RPC 2.0 protocol.
//!
//! This crate contains no agent, LLM, or persistence logic — it is the
//! transport foundation that higher-level APIs (the Python-like
//! `DeepSeekHarness` / `Session` surface) build on.
//!
//! # Compatibility
//!
//! The Python SDK surface is the alignment baseline for types and errors
//! that leak into the public API. Divergences from the TypeScript client are
//! documented where they occur (planned in plan 02, task 4).

pub mod error;
pub mod protocol;
pub mod transport;

pub use error::Error;
pub use protocol::{
    ContentBlock, ImageAttachmentRef, IncomingFrame, IncomingRequest, InitializeParams,
    InitializeResult, JsonRpcErrorBody, JsonRpcId, JsonRpcResponse, JsonRpcResponseOutcome,
    Notification, ServerInfo, SessionEventNotification, SessionPromptParams, SessionPromptResult,
    SessionStatusNotification, SubagentFinishedNotification, SubagentStartedNotification,
};
pub use transport::JsonRpcLineTransport;
