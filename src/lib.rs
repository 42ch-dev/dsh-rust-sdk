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
//! that leak into the public API. [`RunResult`] mirrors Python's five fields
//! exactly (`session_id`, `final_response`, `finish_reason`, `events`,
//! `notifications`; upstream `python/sdk/src/deepseek_harness/api.py:40-46`);
//! the TypeScript SDK's `RunResult` lacks `finish_reason`. Rust
//! intentionally follows Python, not TypeScript:
//!
//! | Field | Python | TypeScript | Rust (this crate) |
//! |---|---|---|---|
//! | `session_id` / `sessionId` | yes | yes | [`RunResult::session_id`] |
//! | `final_response` / `finalResponse` | yes | yes | [`RunResult::final_response`] |
//! | `finish_reason` | yes | no | [`RunResult::finish_reason`] |
//! | `events` (root session only) | yes | yes | [`RunResult::events`] |
//! | `notifications` (root + descendants) | yes | yes | [`RunResult::notifications`] |
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
//! environment is otherwise inherited wholesale. The caller's `DSH_HOME`
//! is an input to resolution (spec §3.2.1), not a post-resolution
//! override: it is excluded from the verbatim passthrough, so the child
//! always receives the resolved absolute, `~`-expanded home (spec §3.2.3).
//! The crate never writes `DSH_CORDIS_CONFIG`, `DSH_SESSION_ROOT`, or
//! `DSH_CWD` — none has a reader upstream — and filters them out of
//! `Config::env` as well (spec §4.2).
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
    ContentBlock, Dimensions, FileAttachmentRef, ImageAttachmentRef, IncomingFrame,
    IncomingRequest, InitializeParams, InitializeResult, JsonRpcErrorBody, JsonRpcId,
    JsonRpcResponse, JsonRpcResponseOutcome, Notification, ServerInfo, SessionEventNotification,
    SessionPromptParams, SessionPromptResult, SessionStatusNotification,
    SubagentFinishedNotification, SubagentStartedNotification,
};
pub use runtime::Config;
pub use transport::JsonRpcLineTransport;
