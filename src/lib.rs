//! # deepseek-harness-sdk
//!
//! Low-level Rust client for the official
//! [DeepSeek Harness](https://github.com/deepseek-ai/deepseek-harness)
//! runtime: typed errors, wire-protocol types, a line transport, and a
//! `HarnessClient` that spawns the runtime process and speaks its stdio
//! JSON-RPC 2.0 protocol.
//!
//! This crate contains no agent, LLM, or persistence logic — the runtime
//! process does all of that. It ships the low-level transport client
//! ([`HarnessClient`]) and, on top of it, the Python-parity high-level
//! surface ([`DeepSeekHarness`] / [`Session::run`] / [`RunResult`]).
//!
//! # Compatibility
//!
//! The Python SDK surface is the alignment baseline for types and errors
//! that leak into the public API. Divergences from the TypeScript client are
//! documented where they occur (planned in plan 02, task 4).

pub mod api;
pub mod client;
pub mod error;
pub mod protocol;
pub mod runtime;
pub mod transport;

pub use api::{extract_finish_reason, DeepSeekHarness, Input, RunResult, Session};
pub use client::{ClientTimeouts, HarnessClient, LaunchSpec, NotificationSubscription};
pub use error::Error;
pub use protocol::{
    ContentBlock, ImageAttachmentRef, IncomingFrame, IncomingRequest, InitializeParams,
    InitializeResult, JsonRpcErrorBody, JsonRpcId, JsonRpcResponse, JsonRpcResponseOutcome,
    Notification, ServerInfo, SessionEventNotification, SessionPromptParams, SessionPromptResult,
    SessionStatusNotification, SubagentFinishedNotification, SubagentStartedNotification,
};
pub use runtime::Config;
pub use transport::JsonRpcLineTransport;
