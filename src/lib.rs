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
//! The **Python** SDK surface is the alignment baseline for types and errors
//! that leak into the public API. The TypeScript SDK's `RunResult` lacks
//! `finish_reason` and `session_root`; Rust intentionally follows Python, not
//! TypeScript:
//!
//! | Field | Python | TypeScript | Rust (this crate) |
//! |---|---|---|---|
//! | `session_id` / `sessionId` | yes | yes | [`RunResult::session_id`] |
//! | `final_response` / `finalResponse` | yes | yes | [`RunResult::final_response`] |
//! | `finish_reason` | yes (Python extension) | no | [`RunResult::finish_reason`] |
//! | `events` (root session only) | yes | yes | [`RunResult::events`] |
//! | `notifications` (root + descendants) | yes | yes | [`RunResult::notifications`] |
//! | `session_root` | yes (Python extension) | no | [`RunResult::session_root`] |
//!
//! (The table is mirrored in the crate README, `## RunResult alignment`;
//! keep the two copies in sync.)
//!
//! # Environment injection
//!
//! [`DeepSeekHarness::start`] boots the runtime under the configured
//! `profile` (`dsh --profile <name> [--patch <path>]...`) and injects the
//! resolved `DSH_HOME`, the caller's `Config::env` entries, and
//! `DEEPSEEK_BASE_URL` / `DEEPSEEK_API_KEY` when configured; the parent
//! environment is otherwise inherited wholesale. The crate never writes
//! `DSH_CORDIS_CONFIG`, `DSH_SESSION_ROOT`, or `DSH_CWD` — none has a
//! reader upstream.
//!
//! The runtime binary is bring-your-own (Plan A): [`DeepSeekHarness::start`]
//! resolves it from `Config::dsh_bin` or the `DSH_RUNTIME_BIN` environment
//! variable. This crate never downloads or bundles a runtime — see
//! <https://github.com/deepseek-ai/deepseek-harness> for the official runtime
//! and its sources.
//!
//! # Non-goals
//!
//! - **No cancellation**: there is no session-close / cancel RPC.
//!   [`Session::run`] waits for root `idle`; closing the harness mid-turn
//!   abandons the turn.
//! - **No Windows support** (consumed platforms: linux-x64, linux-arm64,
//!   macos-arm64).
//! - (The README lists the remaining non-goals: no runtime delivery /
//!   bundling, no crates.io publish, no TypeScript-parity helper.)

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
