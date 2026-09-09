# DSH SDK wire-parity surface

**Status:** Frozen (2026-09-08)
**Scope:** repo-level normative contract for the `deepseek-harness-sdk` wire surface — request methods, server notifications, content-block vocabulary, the `Config` / `RunResult` field set, defaults, and documented divergences from the reference clients.
**Upstream basis:** `deepseek-harness` @ `c389f96bf3a9b6807cb71ed6bdad5849be0df6d8` (`dsh-v0.1.3-alpha.2-133-gc389f96bf3`, 2026-09-08), re-verified line by line on 2026-09-08. Every contract statement below cites `path:line` at that ref.
**Alignment baseline:** the **Python SDK** is the parity baseline (`python/sdk`); the **TypeScript SDK** is the design twin (`packages/sdk/client`). Where they disagree, the crate follows Python unless a divergence is recorded here (§7).
**Normative language:** `MUST` / `MUST NOT` are contract; `SHOULD` is strong guidance that a deviation must justify.

This spec is cross-iteration authority. It states the target surface, not a stopgap. The launch/environment/home half of the contract lives in `dsh-runtime-launch-contract.md`; this file owns the wire and API-shape half.

---

## 1. Contract summary

The SDK runtime speaks **newline-delimited JSON-RPC 2.0 over stdio** with exactly **three request methods** and **four server-to-client notifications** (`packages/sdk/protocol/src/types.ts:106-119`). The crate MUST mirror that set exactly: no additional method strings, no additional notification names, and no client-directed request handling exposed as public API.

The crate MUST keep unknown-tolerant parsing: an unrecognized content-block `type` (or a known tag with a malformed body) MUST fall through to `ContentBlock::Unknown(Value)` rather than fail the parse (`src/protocol.rs:63-73`; the reference clients are untyped here).

---

## 2. Request methods (C→S)

| Method | Params | Result | Upstream evidence |
|---|---|---|---|
| `initialize` | `{cwd, provider, model, reasoningEffort?, maxTokens?}` | `{serverInfo: {name, version}}` | `packages/sdk/protocol/src/types.ts:16-33,115`; dispatch `packages/sdk/server/src/server.ts:135,249` |
| `session/prompt` | `{sessionId, contentBlocks}` | `{messageId}` | `packages/sdk/protocol/src/types.ts:36-59,116`; dispatch `packages/sdk/server/src/server.ts:176,250` |
| `shutdown` | none (`params` omitted by both references) | `{}` | `packages/sdk/protocol/src/types.ts:118`; dispatch `packages/sdk/server/src/server.ts:206,252` |

Normative rules:

1. The crate MUST send only these three method strings. The method strings are wire constants, not an extension point.
2. `initialize` MUST be the first request on the transport, and `start()` MUST await its result before returning. `session/prompt` before a successful `initialize` is rejected by the runtime (`packages/sdk/server/src/server.ts:176-177` — `SDK server is not initialized`).
3. `session/prompt.contentBlocks` MUST be sent verbatim as the user message (`packages/sdk/protocol/src/types.ts:39-41`); the runtime does not re-normalize non-image blocks (`packages/sdk/server/src/server.ts:39-41`).
4. `shutdown` MUST be sent only by the close ladder. The runtime returns `{}` and the plugin then disposes the root and exits 0 (`packages/sdk/server/src/server.ts:206-237,252-253`).
5. Outgoing request ids SHOULD stay string UUIDs; incoming ids MUST accept both string and number, matching the reference transports.
6. `serverInfo.name` MUST equal `deepseek-harness-sdk-runtime` exactly; `serverInfo.version` is hard-coded `0.0.1` with **no version negotiation** (`packages/sdk/server/src/server.ts:168`). Strict name equality is a deliberate, documented divergence (§7.4).

---

## 3. Notifications (S→C)

| Notification | Payload | Emission site |
|---|---|---|
| `session.event` | `{sessionId, event}` — the full session-log event envelope | `packages/sdk/server/src/server.ts:95-98` |
| `session.status` | `{sessionId, status: "idle" \| "running"}` | `packages/sdk/server/src/server.ts:99-101` |
| `subagent.started` | `{parentSessionId, childSessionId}` | `packages/sdk/server/src/server.ts:102-110` |
| `subagent.finished` | `{provider, agentId, parentSessionId, childSessionId, status: "ok" \| "error", stopReason, lastAssistantMessage?}` | `packages/sdk/server/src/server.ts:111-127` |

Type evidence: `packages/sdk/protocol/src/types.ts:64-104,106-112`; `session.status` vocabulary `packages/core/agent/src/runtime-types.ts:103-110`; `SubagentStopReason` five reasons `packages/subagent/subagent/src/types.ts:252-263`; `subagent.finished.status` is a deployment mapping, so `max-tokens` can map to `ok` (`packages/sdk/server/src/server.ts:65-68`).

Normative rules:

1. The crate MUST model exactly these four names and MUST NOT invent others.
2. `session.event.event` MUST stay an untyped `Value` in the crate: the event vocabulary is merge-extensible, and session format v2 changed it (§5.2). Parsing it into a closed Rust enum would break on the next upstream addition.
3. `session.status` and `subagent.finished.status` / `stopReason` SHOULD stay strings rather than closed enums, matching both reference clients' unknown-value strategy. Their runtime vocabularies are documented here, not enforced at parse time.
4. `subagent.started` MUST be treated as the only source of client-side parent→child edges; the tree filter is derived from it (§4.3).

---

## 4. Session-tree semantics

1. `Session::run` MUST subscribe to the session tree **before** writing the `session/prompt` request, so the receipt notification and everything after it are observed from the first broadcast. Python and TypeScript both subscribe before prompting (`python/sdk/src/deepseek_harness/client.py:268-299`; `packages/sdk/client/src/api.ts:186,191-196`).
2. The receipt MUST be matched on `agent/inbox/spliced` → `inserted[].id` equal to the returned `messageId`. Matching `.messageId` never fires and would hang every run. Producer evidence: `packages/core/agent/src/types.ts:87-94`; both clients match `.id` (`python/sdk/src/deepseek_harness/api.py:192-202`; `packages/sdk/client/src/api.ts:289-293`).
3. The activity interval MUST end at the **root** session's idle; descendant idle notifications MUST NOT terminate it.
4. The tree filter MUST accept a `subagent.*` edge when the parent is already in the tree or the child is the root, and MUST key every other notification on `sessionId` (both references implement this identically — `python/sdk/src/deepseek_harness/client.py:492-536`; `packages/sdk/client/src/client.ts:370-381,417-439`).
5. Malformed peer lines MUST be skipped, not rejected: blank lines, non-JSON, and invalid UTF-8 are dropped and reading continues (`python/sdk/src/deepseek_harness/client.py:352-364`; `packages/sdk/protocol/src/transport.ts:180-189,201-208`). A local framing guard (16 MiB line) MAY reject.
6. stderr MUST be captured as a bounded tail and embedded in close/transport diagnostics together with the exit status (`python/sdk/src/deepseek_harness/client.py:60,370-375,433-456`; `packages/sdk/client/src/client.ts:29,445-451,460-466`). The crate's tail is 400 lines; its *embedded* text is additionally capped (a documented local divergence, §7.3).

---

## 5. Content-block vocabulary

### 5.1 The six variants (normative)

`ContentBlockMap` has **six** members — `text`, `reasoning`, `image`, `file`, `tool-call`, `tool-result` (`packages/llm/llm/src/types.ts:112-118`). The crate MUST model all six as typed variants:

| Variant | Shape | Upstream evidence |
|---|---|---|
| `text` | `{type:"text", text}` | `packages/llm/llm/src/types.ts:55-58` |
| `reasoning` | `{type:"reasoning", text}` | `packages/llm/llm/src/types.ts:61-64` |
| `image` | `{type:"image", attachment: ImageAttachmentRef}` | `packages/llm/llm/src/types.ts:72-76` |
| `file` | `{type:"file", attachment: FileAttachmentRef}` | `packages/llm/llm/src/types.ts:84-87` |
| `tool-call` | `{type:"tool-call", id, name, arguments}` — `arguments` is a **raw JSON string** | `packages/llm/llm/src/types.ts:92-98` |
| `tool-result` | `{type:"tool-result", toolCallId, content: ContentBlock[], isError?}` — `content` is recursive | `packages/llm/llm/src/types.ts:102-107` |

`FileAttachmentRef` MUST be `{attachmentId, name, bytes}` (`packages/attachment/attachment/src/types.ts:39-46`).

`ImageAttachmentRef` MUST be `{attachmentId, mediaType, bytes, width, height, name?, originalDimensions?}` where `originalDimensions?: {width, height}` (`packages/attachment/attachment/src/types.ts:11-34`). `originalDimensions` is present only when normalization reduced the image (`packages/attachment/attachment/src/types.ts:24-31`); the crate MUST model it so that parse → serialize round-trips without loss (Track B F-4: re-serializing a parsed image block currently drops it).

### 5.2 `file` is an addition, not a replacement

`ContentBlock::Unknown` MUST remain the fallback for unrecognized tags and malformed bodies. Adding `File` narrows what falls through; it MUST NOT remove the escape hatch, and the crate MUST NOT `deny_unknown_fields` anywhere on the wire path.

The rustdoc "known variants" count MUST say six, not five.

### 5.3 Session format v2 (event vocabulary, not a wire change)

`SESSION_FORMAT_VERSION` is **2** (`packages/core/session/src/types.ts:86`). v2 changed the `session.event` vocabulary:

- `assistant/message` now carries an embedded `stream: AssistantStreamRecord[]` (`packages/core/session/src/types.ts:305-313`).
- `assistant/attempt` was added (`packages/core/session/src/types.ts:319`).
- `assistant/chunk` was removed.

Because the crate keeps `session.event.event` untyped (§3.2), the run path is unaffected: `final_response` reads the last root `assistant/message`'s `data.message.content`, and `finish_reason` reads the last root `turn/end`'s `data.reason.kind`. The crate MUST NOT claim `assistant/chunk` support and MUST NOT parse the embedded v2 stream (non-goal). Documentation MUST state the v2 vocabulary.

---

## 6. Field-level parity table (Python baseline)

### 6.1 `Config`

Python `DeepSeekHarnessConfig` (`python/sdk/src/deepseek_harness/api.py:14-37`) is the baseline; the TypeScript mirror is `packages/sdk/client/src/types.ts:23-53` plus `packages/sdk/client/src/types.ts:54-79` for the high-level wrapper. The authoritative field-by-field contract (types, defaults, rationale) is `dsh-runtime-launch-contract.md` §6.1; this section records only the **parity verdicts**.

| Python field | Crate field | Verdict |
|---|---|---|
| `provider`, `model`, `max_tokens` | same names | parity |
| `reasoning_effort` | `reasoning_effort: Option<String>` | parity — omit-when-unset rule §6.3 |
| `cwd`, `runtime_cwd` | same names | parity |
| `dsh_bin`, `profile`, `patches`, `dsh_home` | same names | parity (launch half in the launch spec) |
| `env` | `env` | parity; caller entries win on collision |
| `initialize_timeout_seconds` | `initialize_timeout: Option<Duration>` | parity on default (30 s); Rust models `None` as unbounded (§6.4) |
| `request_timeout_seconds` | `request_timeout` | parity (`None` = unbounded) |
| `shutdown_timeout_seconds` | `timeouts` (close-ladder struct) | parity in behaviour; Rust splits the ladder into typed tiers |
| `base_url`, `api_key` | same names | parity (injected as `DEEPSEEK_BASE_URL` / `DEEPSEEK_API_KEY`) |

Crate-only fields MUST NOT exist beyond `timeouts` (the close-ladder struct, a local composition of Python's single `shutdown_timeout_seconds`). Python-only fields MUST NOT be dropped silently: `next_request` / `respond` / `notify` are a **Python-only** low-level surface and stay a non-goal (the runtime sends no client-directed requests — §7.2).

### 6.2 `RunResult`

| Field | Python | TypeScript | Crate |
|---|---|---|---|
| `session_id` | yes | yes | yes |
| `final_response` | yes | yes | yes |
| `finish_reason` | yes | **no** | yes — follows Python |
| `events` | yes | yes | yes |
| `notifications` | yes | yes | yes |
| `session_root` | **no** (removed) | no | **MUST NOT** exist |

Evidence: `python/sdk/src/deepseek_harness/api.py:40-46`; `packages/sdk/client/src/types.ts:69-79`; upstream asserts the removal at `python/sdk/tests/test_client.py:880`.

Derivation algorithms MUST match Python exactly:

- `final_response` — last root `assistant/message`, pointer walk `data.message.content` else `data.content`; a non-string `text` contributes `""`; no fallback to an earlier event (`python/sdk/src/deepseek_harness/api.py:211-228`).
- `finish_reason` — last root `turn/end`'s `data.reason.kind` inside the activity interval; no `turn/end` → `None`; a malformed last `turn/end` → protocol error with the exact message `turn/end event requires a string data.reason.kind`; malformedness is checked only on the last one (reversed scan) (`python/sdk/src/deepseek_harness/api.py:231-248`).
- The runtime's `turn/end` reason vocabulary is six kinds — `completed`, `aborted`, `blocked`, `error`, `max-tokens`, `interrupted` (`packages/core/session/src/types.ts:198-222`) — and MUST stay a string, not a closed enum.

### 6.3 `reasoningEffort` — omit-when-unset (normative)

1. `Config::reasoning_effort: Option<String>`; `InitializeParams.reasoning_effort: Option<String>` serialized as the wire key `reasoningEffort`.
2. When unset (`None`) the key MUST be **omitted** from `params` entirely. When set, it MUST be sent as a non-empty string.
3. An empty or whitespace-only value MUST be treated as unset and dropped, never sent: the server rejects a non-string or empty `reasoningEffort` with `TypeError('initialize reasoningEffort must be a non-empty string')` (`packages/sdk/server/src/server.ts:136-138`). Python applies the same rule by only adding the key when the value is not `None` (`python/sdk/src/deepseek_harness/client.py:147-148`).
4. A set value is validated and applied by the server (`packages/sdk/server/src/server.ts:147-161`) and forwarded to every SDK-created agent (`packages/sdk/server/src/server.ts:285`).

### 6.4 `initialize` bound (normative)

1. `Config::initialize_timeout` defaults to **30 s** (`python/sdk/src/deepseek_harness/api.py:33`).
2. The bound MUST apply to the `initialize` handshake request only, not to the activity interval or to `session/prompt`.
3. `None` MUST mean **unbounded** and MUST remain expressible, so a caller can opt out deliberately. It is an explicit opt-out, not a legacy default and not a compatibility shim.
4. On expiry the crate MUST return its timeout error and MUST NOT leave the child running (`python/sdk/src/deepseek_harness/client.py:152-160` closes the client and appends the selected profile before raising).

TypeScript's default is 10 000 ms (`packages/sdk/client/src/launch.ts:12,151`; `packages/sdk/client/src/types.ts:44`). The crate follows **Python's 30 s**, consistent with the alignment baseline.

### 6.5 Notification callback contract (normative)

`Session::run` MUST accept a per-notification observer. The locked Rust shape is:

```rust
pub async fn run(
    &self,
    input: Input,
    on_notification: Option<&(dyn Fn(&Notification) + Send + Sync)>,
) -> Result<RunResult, Error>
```

Rules:

1. The callback MUST observe **every notification delivered to the run's session-tree subscription, in wire order** — including `session.event`, `session.status`, and `subagent.*`. This matches the TypeScript observer's documented scope: "every notification for this session tree, in wire order" (`packages/sdk/client/src/api.ts:152-156,186-200`).
2. The callback MUST be invoked **as notifications arrive**, not only at the end of the run. It MUST NOT be deferred until idle.
3. The callback MUST NOT change the returned `RunResult`: with and without a callback, `events` and `notifications` MUST contain the same sets in the same order. The callback observes; it does not filter or consume.
4. The callback MUST be a layer over the existing notification path, not a second subscription: the run already owns one tree subscription, and duplicating collection would double-count or race.
5. The callback MUST NOT be able to break the run's invariants. A `panic` in the callback is a caller bug and MAY propagate; the crate MUST NOT swallow it. (Neither reference client guards against a throwing callback.)
6. `Session::run` MUST keep taking `&self`; the callback MUST NOT require the caller to give up ownership of the session. A caller needing shared mutable state uses interior mutability (`Arc<Mutex<..>>`) captured by the closure — that is why the bound is `Send + Sync` rather than `FnMut`.
7. The callback is **optional and additive**: passing `None` MUST behave exactly like today's run path.

Python's analogue is `on_notification: Callable[[Notification], None] | None` (`python/sdk/src/deepseek_harness/api.py:124-131,139-144`), delivered by draining a subscription while waiting (`python/sdk/src/deepseek_harness/client.py:268-315`). TypeScript's is `onNotification?: (notification) => void` (`packages/sdk/client/src/api.ts:150-156`), invoked inline in the collect path (`packages/sdk/client/src/api.ts:186-200`). The crate follows the **behaviour** both share (arrival-order observation of the tree) and the **shape** of neither verbatim, because Rust cannot take an optional owning closure without changing the receiver; the signature above is the architect's locked decision and is normative.

---

## 7. Documented divergences

Each divergence below is deliberate and MUST be stated in rustdoc and, where user-visible, in the README pair. A contributor MUST NOT "fix" one back to reference behaviour without a superseding spec decision.

### 7.1 `DSH_HOME` fallback

The crate resolves `~/.dsh` when no home is configured, where Python raises `ValueError`. Rationale and citations: `dsh-runtime-launch-contract.md` §3.3.

### 7.2 No client-directed request API

Python exposes `next_request` / `respond` / `respond_error` / `notify` (`python/sdk/src/deepseek_harness/client.py:214-260`); TypeScript exposes none (`packages/sdk/client/src/client.ts:265-268`). The crate follows TypeScript: client-directed requests are auto-answered `-32601` and are not public API (`src/client/read_loop.rs:134-149`). The runtime emits no client-directed requests (`packages/sdk/server/src/server.ts:246-257`), so this costs nothing. Non-goal, not a gap.

### 7.3 Malformed-notification policy is stricter than both references

The crate fails the run with a protocol error when a `session.event` or `session.status` payload fails its shape check, in receipt-wait, event-collection, and idle-detection paths (`src/api.rs:220-240,261-280,285-303`). Python silently skips a malformed `event` / `status` and only raises on a malformed last `turn/end` (`python/sdk/src/deepseek_harness/api.py:149-181,245-246`); TypeScript raises for a malformed `session.event` envelope but ignores a malformed `session.status` (`packages/sdk/client/src/api.ts:189,210-212,264-286`).

Verdict: **keep the strictness** (it converts a silent hang into a typed failure) but **do not claim it is Python parity**. The rustdoc MUST say the policy is intentionally stricter. This is the one place where "follows Python" does not hold, and the docs must say so.

Related: the crate's embedded stderr text is capped (8 KiB, newest lines first) while both references embed the whole 400-line tail; and the crate's broadcast buffer is bounded with a fail-fast on observed lag where Python's queue is unbounded. Both are documented local robustness choices, not parity claims.

### 7.4 Strict `serverInfo.name` equality

The crate requires exact equality with `deepseek-harness-sdk-runtime` (`packages/sdk/server/src/server.ts:168`); Python treats the fields as optional and TypeScript checks presence only. Deliberate: an upstream rename should fail loudly rather than be silently accepted.

### 7.5 Rust-only ergonomics

No `DeepSeekHarness::run` convenience and no lazy start: the crate requires an explicit `DeepSeekHarness::start`. Python has both a `run` convenience and lazy start (`python/sdk/src/deepseek_harness/api.py:121,124-131`); TypeScript lazily starts inside `run` (`packages/sdk/client/src/api.ts:177`). The divergence is documented in the crate's README and stays a non-goal.

---

## 8. Deferred and non-goal surface

These are **out of scope** and MUST NOT be documented as delivered:

- Inline encoded-image prompt blocks (`SdkEncodedImageBlock`, `packages/sdk/protocol/src/types.ts:43-53`): needs a prompt-side type distinct from `ContentBlock::Image`; own plan.
- Python-only `next_request` / `respond` / `notify` (§7.2).
- A Rust `expandAssistantStream` equivalent for v2 embedded streams (§5.3).
- TypeScript-parity helpers (the crate has no TS parity contract).
- Protocol version negotiation (`serverInfo.version` is hard-coded `0.0.1`).

---

## 9. Verification obligations

A change to this contract MUST be verifiable by:

1. **Protocol unit tests** asserting: `reasoningEffort` is absent when `None` and present when `Some`; a blank value is dropped; a `{"type":"file",...}` block parses to the typed variant; an image block with `originalDimensions` round-trips parse → serialize; an unknown tag still reaches `Unknown`.
2. **A callback test** driven by the fake runtime: the callback fires for every notification the client received, in arrival order, and the returned `RunResult` is byte-identical to the no-callback path.
3. **A bounded-handshake test**: a fake runtime that accepts the frame and never replies makes `start()` fail within the configured bound instead of hanging.
4. **A parity table check**: the crate's `Config` / `RunResult` field sets match §6.1 / §6.2, asserted by exhaustive destructuring so an added or removed field breaks the build.
5. **A removal grep** for `session_root` across spec, READMEs, and changelog fragment (AC5).

---

## 10. Citation index (upstream @ `c389f96bf3`)

| Path | Lines used |
|---|---|
| `packages/sdk/protocol/src/types.ts` | 16-33, 36-59, 43-53, 64-104, 106-119 |
| `packages/sdk/server/src/server.ts` | 39-41, 65-68, 95-127, 135-161, 168, 176-177, 206-237, 246-257, 285 |
| `packages/sdk/client/src/api.ts` | 150-156, 177, 186-200, 210-212, 264-293 |
| `packages/sdk/client/src/client.ts` | 29, 263-268, 370-381, 417-439, 445-455, 460-466 |
| `packages/sdk/client/src/launch.ts` | 12, 151 |
| `packages/sdk/client/src/types.ts` | 23-53, 69-79 |
| `packages/sdk/protocol/src/transport.ts` | 180-190, 201-208 |
| `packages/llm/llm/src/types.ts` | 55-58, 61-64, 72-76, 84-87, 92-98, 102-107, 112-118 |
| `packages/attachment/attachment/src/types.ts` | 11-34, 39-46 |
| `packages/core/session/src/types.ts` | 86, 198-222, 305-313, 319 |
| `packages/core/agent/src/types.ts` | 87-94 |
| `packages/core/agent/src/runtime-types.ts` | 103-110 |
| `packages/subagent/subagent/src/types.ts` | 252-263 |
| `python/sdk/src/deepseek_harness/api.py` | 14-37, 40-46, 121, 124-144, 149-181, 192-202, 211-228, 231-248 |
| `python/sdk/src/deepseek_harness/client.py` | 60, 147-148, 152-160, 214-260, 269-315, 352-364, 370-375, 433-456, 493-536 |
| `python/sdk/tests/test_client.py` | 880 |

Rust-side paths (`src/protocol.rs`, `src/api.rs`, `src/client/`) are cited at the v0.1 tree as the current state this contract amends.

Re-verification rule: if upstream advances past `c389f96bf3`, re-run the greps behind §2, §3, §5.1, and §6.3 against the new ref before restating any line here. A moved line number is not a contract change; a changed rule is.
