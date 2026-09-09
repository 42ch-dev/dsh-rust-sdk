# DSH runtime launch, home, and configuration contract

**Status:** Frozen (2026-09-08)
**Scope:** repo-level normative contract for how `deepseek-harness-sdk` resolves, launches, and configures a DSH runtime process.
**Upstream basis:** `deepseek-harness` @ `c389f96bf3a9b6807cb71ed6bdad5849be0df6d8` (`dsh-v0.1.3-alpha.2-133-gc389f96bf3`, 2026-09-08), re-verified line by line on 2026-09-08. Every contract statement below cites `path:line` at that ref.
**Normative language:** `MUST` / `MUST NOT` are contract; `SHOULD` is strong guidance that a deviation must justify. Prose without these words is rationale.

This spec is cross-iteration authority. It describes the target contract, not a stopgap. Per-plan process narrative, task lists, and iteration-local decisions do not belong here; they live in `.mstar/plans/` and `.mstar/iterations/`.

---

## 1. Contract summary

The DSH runtime is **the `dsh` CLI booted under a named profile** (`apps/cli/package.json:2,15-17`). There is no separate JSON-RPC agent program: the stdio JSON-RPC server is a plugin row inside the profile bundle (`packages/bundle/sdk-app/cordis.patch.yml:18-19`, `packages/bundle/sdk-minimal/cordis.patch.yml:11-12`).

The crate MUST therefore compose a **launch** (program + ordered argv + child environment) rather than resolve a bare executable. Four contract areas follow: launch grammar (§2), `DSH_HOME` resolution (§3), child environment (§4), and the `Config` / `RunResult` surface (§6). Error semantics are §7; the removal list is §5.

The crate MUST NOT bundle, download, ship, or vend a runtime. Runtime acquisition stays bring-your-own (§8).

---

## 2. Launch grammar

### 2.1 Upstream grammar (normative)

`dsh --profile <name> [--patch <path>]... [app args...]`

| Fact | Upstream evidence |
|---|---|
| The launcher's own flags are `--profile <name>`, `--patch <path>` (repeatable), `--dump-config`, `--dump-default-config` | `apps/cli/src/args.ts:137-140` |
| `--profile` is **mandatory**; a bare `dsh` prints `error: --profile <name> is required` and does not boot | `apps/cli/src/args.ts:144-146` |
| An empty profile name is rejected (`error: --profile needs a name`) | `apps/cli/src/args.ts:148-149` |
| The profile names a directory under `$DSH_HOME/profiles` | `apps/cli/src/args.ts:137` |
| `--patch` is an ordered overlay applied **after** the profile layer, in argv order | `apps/cli/src/args.ts:138,24-25`; collector `:58-61`; empty patch path rejected `:91` |
| Arguments after the launcher's own flags belong to the booted app | `apps/cli/src/args.ts:129-136` |
| Shipped profile names include `sdk` and `sdk-minimal` | `packages/boot/app-boot/src/profile.ts:110-133` |

Both reference clients compose exactly this argv:

- Python: `return (*base, "--profile", self.config.profile, *patches)` — `python/sdk/src/deepseek_harness/client.py:486`; patches flattened as `("--patch", <abs>)` pairs at `:481-485`.
- TypeScript: `args: [...dshLaunch.nodeArgs, '--profile', profile, ...patches.flatMap(path => ['--patch', path])]` — `packages/sdk/client/src/launch.ts:143`.

### 2.2 Crate obligations

1. The crate MUST spawn the resolved program with argv whose **first pair is** `--profile`, `<profile>`, followed by one `--patch`, `<path>` pair per configured patch, in the caller's order.
2. `Config::profile` MUST default to `"sdk"` (`python/sdk/src/deepseek_harness/api.py:30`; `packages/sdk/client/src/launch.ts:131`).
3. The crate MUST NOT emit `--dump-config` or `--dump-default-config`; those are diagnostic subcommands that print and exit (`apps/cli/src/args.ts:139-140`).
4. The crate MUST NOT pass application arguments: `DeepSeekHarness::start` drives the JSON-RPC handshake over stdio, and the app-args tail (`apps/cli/src/args.ts:136`) is not part of the SDK contract.
5. Patch paths MUST be resolved to absolute paths before spawn, matching both references (Python `Path(patch).expanduser().resolve()` — `python/sdk/src/deepseek_harness/client.py:483-484`; TS `resolve(callerCwd, path)` — `packages/sdk/client/src/launch.ts:137-140`).
6. An empty `profile` value MUST be rejected locally with a configuration error rather than launched; the runtime rejects it too (`apps/cli/src/args.ts:149`), and a local rejection keeps the failure attributable.

**Rationale.** The old bare-program spawn (`args: []`) is not a valid `dsh` invocation: the child exits 1 before any JSON-RPC frame exists (`apps/cli/src/args.ts:144-146`; PM live probe in `references/rust-sdk-analysis/consolidated-verdict.md` §"PM live verification"). A launch that omits `--profile` cannot produce a session.

---

## 3. `DSH_HOME` resolution

### 3.1 Upstream precedence (normative)

`resolveDshHome(configured?, env)` resolves, highest precedence first:

1. an explicit **configured** path, else
2. a **non-empty** `$DSH_HOME` (empty or whitespace-only counts as **unset**), else
3. `~/.dsh`.

The result is normalized to an absolute path with `~` expanded.

Evidence: `packages/util/home-paths/src/index.ts:87-91` (function body: `configured ?? (fromEnv !== undefined && fromEnv.trim().length > 0 ? fromEnv : defaultDshHome())`, then `resolve(expandHomePath(selected))`); constants `DSH_HOME_ENV = 'DSH_HOME'` `:18`, `DSH_HOME_DIR_NAME = '.dsh'` `:12`, default `join(homedir(), '.dsh')` `:62`, expansion `:70-74`, documented precedence `:79-82`.

### 3.2 Crate obligations

1. The crate MUST resolve the harness home as: `Config::dsh_home` → non-empty `DSH_HOME` (first from `Config::env`, then inherited from the parent environment) → `~/.dsh`. It MUST NOT add a fourth rule and MUST NOT invert the order.
2. Blank or whitespace-only `DSH_HOME` MUST be treated as unset, exactly as upstream does (`packages/util/home-paths/src/index.ts:88-89`).
3. The resolved home MUST be an absolute path with `~` expanded before it is written to the child environment.
4. The resolved home MUST be **observable by the caller** through public API. The crate MUST NOT resolve a home silently and write into it without the caller being able to read the value it selected. (This is the AC4 observability requirement; the product rationale is that `~/.dsh` is a real user directory, not a sandbox.)
5. The crate SHOULD create the resolved home directory when it does not exist, so a fresh home boots (`dsh` self-initializes `$DSH_HOME/profiles/<name>` on first use — `packages/boot/app-boot/src/profile.ts:100,109-133,187`).

### 3.3 Deliberate divergence from the Python SDK

Python **refuses** to fall back:

- `dsh_home` set but blank → `ValueError("HarnessConfig requires a non-empty dsh_home")` — `python/sdk/src/deepseek_harness/client.py:471-473`.
- `dsh_home` unset and `DSH_HOME` blank → `ValueError("HarnessConfig requires an explicit dsh_home or non-empty DSH_HOME; the Python SDK never uses ~/.dsh implicitly")` — `python/sdk/src/deepseek_harness/client.py:475-479`.

TypeScript does **not** refuse: `dshHome` is optional and `undefined` injects no `DSH_HOME` at all, leaving the runtime to apply its own `~/.dsh` default (`packages/sdk/client/src/launch.ts:140,148`; documented as optional at `packages/sdk/client/src/types.ts:31-32`).

**Decision (locked).** The crate follows the **runtime's** precedence and therefore behaves like TypeScript, not Python: it resolves `~/.dsh` instead of raising. Rationale: `~/.dsh` is the runtime's own documented default (`packages/util/home-paths/src/index.ts:79-82`), and requiring an explicit home would be a divergence *from the runtime contract* that the crate exists to mirror. The real defect the Python refusal guards against is **silence**, which obligation §3.2.4 (observability) addresses instead.

**This divergence is normative and MUST be documented**, never implied: in this spec, in both READMEs, and in the changelog fragment (AC5/AC7). A future contributor MUST NOT "fix" the crate back to Python's refusal without a superseding spec decision.

---

## 4. Child environment

### 4.1 Keys the crate writes

The crate MUST write exactly these keys into the runtime child environment:

| Key | When | Value |
|---|---|---|
| `DSH_HOME` | always (a home always resolves, §3) | the resolved absolute home |
| every entry of `Config::env` | when the caller supplies them | verbatim |
| `DEEPSEEK_BASE_URL` | when `Config::base_url` is set | the configured URL |
| `DEEPSEEK_API_KEY` | when `Config::api_key` is set | the configured key |

`DEEPSEEK_API_KEY` / `DEEPSEEK_BASE_URL` remain supported because the runtime still reads them; they are unchanged by this contract (`python/sdk/src/deepseek_harness/api.py:70-73` injects them from `base_url` / `api_key` the same way).

Precedence on collision: the caller's `Config::env` wins over the crate-injected `DSH_HOME`, matching today's documented override rule and Python's `env.update(self.config.env)` ordering (`python/sdk/src/deepseek_harness/client.py:75-77`).

### 4.2 The three keys the crate MUST NEVER write

`DSH_CORDIS_CONFIG`, `DSH_SESSION_ROOT`, and `DSH_CWD` MUST NOT be written into the child environment under any configuration.

Evidence that no reader exists upstream at `c389f96bf3`:

- The bundled default `cordis.yml` that consumed `DSH_CORDIS_CONFIG` was deleted (`python/sdk-runtime/README.md:15`; `be7b064504` deleted the file), and `DSH_CORDIS_CONFIG` survives at HEAD only inside Python's own removal assertions (`python/sdk/tests/test_client.py:876`).
- Sessions now live under `$DSH_HOME/sessions` (`packages/bundle/base/cordis.patch.yml:110-113` — `root: !!js dshHomePath('sessions')`), not under `DSH_SESSION_ROOT`.
- The workspace cwd reaches the runtime through `initialize.cwd` (`packages/sdk/protocol/src/types.ts:17-18`) and the profile's `{{cwd}}` persona (`packages/bundle/sdk-app/cordis.patch.yml:5`), not through `DSH_CWD`.
- Python removed all three writes in `56e038b2e3`; `python/sdk/tests/test_client.py:116-118` pins their absence.

Writing them is not merely useless — it is misleading: the old code made `Config::session_root` *appear* to control where sessions land while they in fact landed under `~/.dsh`.

---

## 5. Removal list (each item with its replacement)

Every item below MUST be removed with **no alias, no `#[deprecated]` shim, no `Option` fallback** (repo rule: remove obsolete paths). Each MUST appear in the spec, both READMEs, and the changelog fragment with the replacement named on the same line (AC5). The list is fixed here and MUST NOT drift between plans.

| Removed item | Location at v0.1 | Replacement | Why |
|---|---|---|---|
| `Config::session_root` | `src/runtime.rs:103-104` | `Config::dsh_home` | sessions live under `$DSH_HOME/sessions` (`packages/bundle/base/cordis.patch.yml:110-113`) |
| `Config::cordis_config` | `src/runtime.rs:96-98` | the profile tree (`Config::profile` + `Config::patches`) | no config file is passed to the runtime; the profile composes the tree (`packages/boot/app-boot/src/profile.ts:110-133`) |
| the `DSH_CORDIS_CONFIG` injection | `src/runtime.rs:309-338` | the profile tree | no reader upstream (§4.2) |
| the `DSH_SESSION_ROOT` injection | `src/runtime.rs:305-308` | `Config::dsh_home` | no reader upstream (§4.2) |
| the `DSH_CWD` injection | `src/runtime.rs:301-304` | `Config::cwd` (sent as `initialize.cwd`) | no reader upstream (§4.2) |
| `Config::launch_args_override` | `src/runtime.rs:93-95` | `Config::dsh_bin` + `Config::profile` + `Config::patches` | the launch is now composed from typed fields, not an opaque argv |
| `Config::runtime_bin` | `src/runtime.rs:91-92` | `Config::dsh_bin` (rename) | Python/TS name the field `dsh_bin` (`python/sdk/src/deepseek_harness/api.py:29`; `packages/sdk/client/src/launch.ts:131`) |
| `RunResult::session_root` | `src/api.rs:376-378` | dropped — no replacement | upstream removed it and asserts its absence (`python/sdk/src/deepseek_harness/api.py:40-46`; `python/sdk/tests/test_client.py:880`) |
| `assets/cordis.yml` | `assets/cordis.yml` | the profile bundle | it mounts `@deepseek-ai/dsh-agent-spine-demo`, deleted upstream (`244de7c18a`) |
| `DEFAULT_CORDIS_YML` | `src/runtime.rs:36-37` | the profile bundle | it embeds the file above |
| `bundled_default_config_path` | `src/runtime.rs:362-384` | the profile bundle | its whole purpose was the deleted injection channel |
| the `assets/cordis.yml` entry in `Cargo.toml [package] include` | `Cargo.toml:22` | — | the file no longer exists |

**Not on this list — explicitly preserved:** `DSH_RUNTIME_BIN`. It remains the crate's runtime-path override after the rename, in today's resolution order `Config::dsh_bin` → non-empty `DSH_RUNTIME_BIN` → `Error::RuntimeNotFound`, with empty counting as absent (AC10). It is a supported product surface, **not** a compatibility shim for a removed field, and MUST NOT be deleted as dead code.

---

## 6. Public surface

### 6.1 `Config` field set

The crate MUST expose exactly these `Config` fields (Python-parity naming, `python/sdk/src/deepseek_harness/api.py:14-37`; TS mirror `packages/sdk/client/src/types.ts:23-53`):

| Field | Type | Default | Source / note |
|---|---|---|---|
| `provider` | `String` | `"deepseek-official"` | `python/sdk/src/deepseek_harness/api.py:21` |
| `model` | `String` | `"deepseek-v4-flash"` | `python/sdk/src/deepseek_harness/api.py:22` |
| `reasoning_effort` | `Option<String>` | `None` | `python/sdk/src/deepseek_harness/api.py:24`; wire rule in `dsh-sdk-wire-parity-surface.md` §6 |
| `max_tokens` | `Option<u32>` | `None` | `python/sdk/src/deepseek_harness/api.py:25` |
| `cwd` | `Option<PathBuf>` | `None` → process cwd | sent as `initialize.cwd` (`packages/sdk/protocol/src/types.ts:17-18`) |
| `runtime_cwd` | `Option<PathBuf>` | `None` → `cwd` | subprocess cwd (`python/sdk/src/deepseek_harness/api.py:28,68`) |
| `dsh_bin` | `Option<String>` | `None` | `python/sdk/src/deepseek_harness/api.py:29` |
| `profile` | `String` | `"sdk"` | `python/sdk/src/deepseek_harness/api.py:30` |
| `patches` | `Vec<PathBuf>` | empty | `python/sdk/src/deepseek_harness/api.py:31` |
| `dsh_home` | `Option<PathBuf>` | `None` | `python/sdk/src/deepseek_harness/api.py:32`; resolution §3 |
| `env` | `Option<HashMap<String, String>>` | `None` | `python/sdk/src/deepseek_harness/api.py:33` |
| `initialize_timeout` | `Option<Duration>` | `Some(30 s)` | `python/sdk/src/deepseek_harness/api.py:33` (`initialize_timeout_seconds: float = 30.0`) |
| `request_timeout` | `Option<Duration>` | `None` (unbounded) | `python/sdk/src/deepseek_harness/api.py:34` |
| `timeouts` | `ClientTimeouts` | close-ladder defaults (1 s / 6 s / 3 s) | `python/sdk/src/deepseek_harness/api.py:35` (`shutdown_timeout_seconds: float or None = 1.0`) |
| `base_url` | `Option<String>` | `None` | `python/sdk/src/deepseek_harness/api.py:36` |
| `api_key` | `Option<String>` | `None` | `python/sdk/src/deepseek_harness/api.py:37` |

Removed relative to v0.1: `session_root`, `cordis_config`, `launch_args_override`, `runtime_bin` (renamed). See §5.

### 6.2 `RunResult` field set

`RunResult` MUST have exactly the five Python fields (`python/sdk/src/deepseek_harness/api.py:40-46`):

| Field | Type | Meaning |
|---|---|---|
| `session_id` | `String` | the SDK session the turn ran on |
| `final_response` | `String` | last root `assistant/message` text (Python algorithm, `python/sdk/src/deepseek_harness/api.py:211-228`) |
| `finish_reason` | `Option<String>` | last root `turn/end` `data.reason.kind` (Python algorithm, `python/sdk/src/deepseek_harness/api.py:231-248`) |
| `events` | `Vec<Value>` | root-session `session.event` payloads, transport order |
| `notifications` | `Vec<Notification>` | every tree notification, transport order |

`session_root` MUST NOT be present. `finish_reason` MUST be present: Python has it and TypeScript does not (`packages/sdk/client/src/types.ts:69-79`), and the crate's alignment baseline is Python (root `AGENTS.md`-level direction; roadmap § Direction).

---

## 7. Error semantics

The crate MUST keep its existing typed taxonomy (`src/error.rs:8-55`) and map launch/config failures as follows:

| Condition | Error |
|---|---|
| no `dsh_bin`, no non-empty `DSH_RUNTIME_BIN` | `Error::RuntimeNotFound` |
| `profile` empty | configuration error before spawn (reject locally) |
| program spawn / stdio failure | `Error::Io` |
| child exits or closes stdio before/while a request is outstanding | `Error::TransportClosed` with exit status and stderr tail diagnostics |
| `initialize` exceeds `initialize_timeout` | `Error::RequestTimeout { method: "initialize", .. }` |
| `serverInfo.name` is not exactly `deepseek-harness-sdk-runtime` | `Error::SdkProtocol` (strict equality is a deliberate, documented divergence — see `dsh-sdk-wire-parity-surface.md` §7) |
| runtime returns a JSON-RPC error | `Error::JsonRpc { code, message, data }` |

`initialize` MUST be bounded by `initialize_timeout` and only the handshake; the activity interval keeps using `request_timeout` (Python parity: `initialize_timeout_seconds` vs `request_timeout_seconds`, `python/sdk/src/deepseek_harness/api.py:33-34`, applied at `python/sdk/src/deepseek_harness/client.py:152-157`). The `RequestTimeout` payload SHOULD name the selected profile so a wedged handshake is diagnosable (Python appends `selected dsh profile {profile!r}` — `python/sdk/src/deepseek_harness/client.py:158-160`).

---

## 8. Durable roadmap interaction (deferred scope)

This section records the long-term contract interaction with the **deferred** roadmap item `runtime-bin-delivery`. It is normative for what this spec permits later; it is **not** in scope for this spec's current obligations.

- The launch model locked here (program + `--profile` + ordered `--patch` + resolved `DSH_HOME`) is the model schemes **B** (companion crate embedding a prebuilt runtime) and **C** (first-run download from GitHub Releases) MUST build on. Neither scheme may introduce a second launch grammar; both MUST populate `Config::dsh_bin` (or resolve the program internally) and leave argv composition unchanged.
- `DSH_RUNTIME_BIN` (AC10) remains the env override for the acquisition scheme **A** in use today. Scheme B/C MUST keep it working and MUST NOT repurpose it as a cache path.
- `Error::RuntimeNotFound` stays reserved for "no runtime could be resolved": under scheme B/C it is emitted only for an unsupported OS/arch or a failed artifact fetch, never for a normal launch failure.
- The scheme choice (B **or** C, never both), the platform matrix, and the done definition live in the `_default` project roadmap (`{PROJECT_DIR}/_default/roadmap.md` § Deferred, row `runtime-bin-delivery`) and must be locked before implementation.
- Until that trigger fires, the crate MUST NOT bundle, download, or ship a runtime (§1), and MUST NOT document acquisition as delivered.

---

## 9. Verification obligations

A change to this contract MUST be verifiable by:

1. **Unit tests** over argv composition (default `["--profile","sdk"]`, ordered `--patch` pairs) and over the child env (contains `DSH_HOME`, contains none of the three forbidden keys).
2. **Unit tests** over the home-resolution matrix: explicit wins → inherited non-empty → blank falls through → nothing resolves `~/.dsh`; resolved path absolute with `~` expanded; resolution driven by an injected env map so tests never touch a real home.
3. **A public accessor** returning the resolved home, asserted absolute (§3.2.4).
4. **A keyless handshake** against a real `dsh`: `start()` completes `initialize` and `close()` reaps the child, with no API key.
5. **A removal grep**: for every identifier in §5, `grep -n "<identifier>" .mstar/specs/*.md README.md README.zh.md .changes/unreleased/*.md` returns at least one hit on a line that names the replacement.

---

## 10. Citation index (upstream @ `c389f96bf3`)

| Path | Lines used |
|---|---|
| `apps/cli/src/args.ts` | 129-136, 137-140, 144-146, 148-149, 24-25, 58-61, 91 |
| `apps/cli/package.json` | 2, 15-17 |
| `packages/util/home-paths/src/index.ts` | 12, 18, 62, 70-74, 79-82, 87-91 |
| `packages/boot/app-boot/src/profile.ts` | 100, 109-133, 187 |
| `packages/bundle/base/cordis.patch.yml` | 110-113 |
| `packages/bundle/sdk-app/cordis.patch.yml` | 5, 18-19 |
| `packages/bundle/sdk-minimal/cordis.patch.yml` | 11-12 |
| `packages/sdk/protocol/src/types.ts` | 16-33 |
| `packages/sdk/client/src/launch.ts` | 131, 137-140, 143, 148 |
| `packages/sdk/client/src/types.ts` | 23-53, 69-79 |
| `python/sdk/src/deepseek_harness/api.py` | 14-37, 40-46, 70-73, 211-228, 231-248 |
| `python/sdk/src/deepseek_harness/client.py` | 75-77, 152-160, 471-479, 481-486 |
| `python/sdk/tests/test_client.py` | 116-118, 876, 880 |
| `python/sdk-runtime/README.md` | 15 |

Rust-side paths (`src/runtime.rs`, `src/api.rs`, `src/error.rs`, `Cargo.toml`, `assets/cordis.yml`) are cited at the v0.1 tree and are the *removal* targets, not the contract source.

Re-verification rule: if upstream advances past `c389f96bf3`, re-run the greps in §2.1, §3.1, and §4.2 against the new ref before restating any line here. A moved line number is not a contract change; a changed rule is.
