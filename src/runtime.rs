//! Runtime launch composition and child environment (Plan A: the caller
//! brings their own runtime binary; this crate does not download or bundle
//! one).
//!
//! The DSH runtime is the `dsh` CLI booted under a named profile
//! (`apps/cli/package.json:2,15-17` upstream): there is no separate
//! JSON-RPC carrier program. [`resolve_runtime`] therefore composes a
//! **launch** — program + ordered argv — rather than resolving a bare
//! executable:
//!
//! 1. `Config::dsh_bin` (non-empty);
//! 2. `DSH_RUNTIME_BIN` from the parent environment (non-empty);
//! 3. otherwise [`Error::RuntimeNotFound`] with acquisition hints.
//!
//! The argv is exactly `--profile <name>` followed by one `--patch <abs
//! path>` pair per configured patch, in caller order (upstream launch
//! grammar, `apps/cli/src/args.ts:137-140,144-146`; Python
//! `python/sdk/src/deepseek_harness/client.py:481-486`). An empty `profile`
//! is rejected locally before spawn.
//!
//! [`compose_env`] builds the override set injected into the runtime
//! subprocess: the resolved `DSH_HOME` (`Config::dsh_home` → non-empty
//! `$DSH_HOME` from `Config::env`, then the parent environment, else
//! `~/.dsh` — upstream `resolveDshHome`,
//! `packages/util/home-paths/src/index.ts:87-91`), the
//! caller's `Config::env` entries (verbatim, except that `DSH_HOME` and
//! the three forbidden keys of spec §4.2 are filtered out under any
//! configuration), and `DEEPSEEK_BASE_URL` / `DEEPSEEK_API_KEY` when
//! configured. The caller's `DSH_HOME` is an **input to resolution**
//! (spec §3.2.1), not a post-resolution override: it is excluded from the
//! verbatim passthrough, so the child always receives the resolved
//! absolute, `~`-expanded home (spec §3.2.3). Every other caller key keeps
//! verbatim passthrough and wins over the crate-injected values on
//! collision (Python `env.update(self.config.env)` ordering,
//! `python/sdk/src/deepseek_harness/client.py:75-77`). The crate never
//! writes `DSH_CORDIS_CONFIG`, `DSH_SESSION_ROOT`, or `DSH_CWD` — none has
//! a reader upstream (spec §4.2).
//!
//! The official runtime and its sources live at
//! <https://github.com/deepseek-ai/deepseek-harness>.

use std::collections::HashMap;
use std::ffi::OsString;
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::client::ClientTimeouts;
use crate::error::Error;

/// Acquisition hints embedded in [`Error::RuntimeNotFound`] when no runtime
/// binary is configured anywhere. Names the bring-your-own route and the
/// build-from-source route; must never advertise a not-yet-published Rust
/// companion crate or a Python wheel as a v0.1 install path.
const RUNTIME_NOT_FOUND_HINT: &str = "no DeepSeek Harness runtime binary is configured. \
Bring your own: set DSH_RUNTIME_BIN (or Config::dsh_bin) to a runtime binary you already \
have, or build the official runtime from the deepseek-harness repository \
(https://github.com/deepseek-ai/deepseek-harness) with \
`scripts/build-exe-for-python-sdk.ts` and point DSH_RUNTIME_BIN at the built executable";

/// High-level launch configuration, mirroring Python
/// `DeepSeekHarnessConfig` (`python/sdk/src/deepseek_harness/api.py`).
///
/// `provider` and `model` mirror the Python defaults; they may drift
/// upstream, so treat them as Python-parity defaults.
///
/// Construction is via the public fields plus [`Config::default`]; a
/// builder-style API is deferred (not part of the v0.1 surface).
///
/// `Debug` redacts the credential fields: `api_key` and any
/// `DEEPSEEK_API_KEY` entry in [`Config::env`] print as `<redacted>`, so a
/// `{:?}` of the config never leaks the live API key.
#[derive(Clone)]
pub struct Config {
    /// Provider name sent to the runtime on `initialize` (Python default
    /// `"deepseek-official"`, `python/sdk/src/deepseek_harness/api.py:21`).
    pub provider: String,
    /// Model name sent to the runtime on `initialize` (Python default
    /// `"deepseek-v4-flash"`, `python/sdk/src/deepseek_harness/api.py:22`).
    pub model: String,
    /// Optional `reasoningEffort` for `initialize` (Python
    /// `python/sdk/src/deepseek_harness/api.py:24`). Surface only in this
    /// plan; the wire rule lands with plan 06.
    pub reasoning_effort: Option<String>,
    /// Optional `maxTokens` for `initialize` (rejected when `0`).
    pub max_tokens: Option<u32>,
    /// Working directory for the agent; defaults to the current directory,
    /// resolved absolute. Feeds `initialize.cwd`.
    pub cwd: Option<PathBuf>,
    /// Subprocess working directory for the runtime; defaults to `cwd`
    /// (Python parity).
    pub runtime_cwd: Option<PathBuf>,
    /// Path to (or name of) a runtime binary the caller already has
    /// (Python `dsh_bin`, `python/sdk/src/deepseek_harness/api.py:29`).
    pub dsh_bin: Option<String>,
    /// The DSH profile to boot (`--profile <name>`); defaults to `"sdk"`
    /// (Python `python/sdk/src/deepseek_harness/api.py:30`). An empty value
    /// is rejected locally before spawn.
    pub profile: String,
    /// Ordered overlay patches, emitted as one `--patch <abs path>` pair per
    /// entry in caller order and resolved absolute before spawn (Python
    /// `python/sdk/src/deepseek_harness/api.py:31`).
    pub patches: Vec<PathBuf>,
    /// Explicit harness home; resolution precedence in
    /// [`Config::resolve_dsh_home`] (Python `dsh_home`,
    /// `python/sdk/src/deepseek_harness/api.py:32`; spec §3).
    pub dsh_home: Option<PathBuf>,
    /// Extra environment entries layered over the parent environment.
    /// `DSH_HOME` here is an input to resolution (spec §3.2.1) and is
    /// excluded from the verbatim passthrough — the child always receives
    /// the resolved absolute home. Every other caller entry keeps verbatim
    /// passthrough and wins over the crate-injected values on collision
    /// (Python `env.update(self.config.env)` ordering).
    pub env: Option<HashMap<String, String>>,
    /// `initialize` handshake deadline (Python
    /// `initialize_timeout_seconds: float = 30.0`,
    /// `python/sdk/src/deepseek_harness/api.py:33`). Surface only in this
    /// plan; the handshake bound lands with plan 06.
    pub initialize_timeout: Option<Duration>,
    /// Per-request response deadline; `None` waits indefinitely (Python
    /// default).
    pub request_timeout: Option<Duration>,
    /// Close-ladder timeouts via plan 01 [`ClientTimeouts`].
    pub timeouts: ClientTimeouts,
    /// Overrides the inherited `DEEPSEEK_BASE_URL`.
    pub base_url: Option<String>,
    /// Overrides the inherited `DEEPSEEK_API_KEY`.
    pub api_key: Option<String>,
}

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("provider", &self.provider)
            .field("model", &self.model)
            .field("reasoning_effort", &self.reasoning_effort)
            .field("max_tokens", &self.max_tokens)
            .field("cwd", &self.cwd)
            .field("runtime_cwd", &self.runtime_cwd)
            .field("dsh_bin", &self.dsh_bin)
            .field("profile", &self.profile)
            .field("patches", &self.patches)
            .field("dsh_home", &self.dsh_home)
            .field(
                "env",
                &self.env.as_ref().map(|env| {
                    // A credential carried through Config::env is redacted
                    // exactly like api_key (the env key is injected into the
                    // runtime subprocess verbatim).
                    env.iter()
                        .map(|(key, value)| {
                            let value = if key == "DEEPSEEK_API_KEY" {
                                "<redacted>"
                            } else {
                                value.as_str()
                            };
                            (key.as_str(), value)
                        })
                        .collect::<Vec<(&str, &str)>>()
                }),
            )
            .field("initialize_timeout", &self.initialize_timeout)
            .field("request_timeout", &self.request_timeout)
            .field("timeouts", &self.timeouts)
            .field("base_url", &self.base_url)
            .field("api_key", &self.api_key.as_deref().map(|_| "<redacted>"))
            .finish()
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            provider: "deepseek-official".into(),
            model: "deepseek-v4-flash".into(),
            reasoning_effort: None,
            max_tokens: None,
            cwd: None,
            runtime_cwd: None,
            dsh_bin: None,
            profile: "sdk".into(),
            patches: Vec::new(),
            dsh_home: None,
            env: None,
            initialize_timeout: Some(Duration::from_secs(30)),
            request_timeout: None,
            timeouts: ClientTimeouts::default(),
            base_url: None,
            api_key: None,
        }
    }
}

/// The resolved runtime launch command, feeding plan 01
/// [`LaunchSpec`](crate::client::LaunchSpec) (`program` + `args`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeLaunch {
    /// Path to (or name of) the runtime executable.
    pub program: PathBuf,
    /// Command-line arguments passed to the runtime: exactly
    /// `--profile <name>` followed by one `--patch <abs path>` pair per
    /// configured patch, in caller order.
    pub args: Vec<OsString>,
}

/// Resolve the runtime launch command for a [`Config`].
///
/// Program precedence (Python `HarnessClient` parity, plus the Rust-only
/// `DSH_RUNTIME_BIN` route):
///
/// 1. `Config::dsh_bin` (non-empty);
/// 2. `DSH_RUNTIME_BIN` from the parent environment;
/// 3. [`Error::RuntimeNotFound`] whose message names both acquisition routes
///    (bring-your-own, and building the official runtime via
///    `scripts/build-exe-for-python-sdk.ts`) and cites
///    <https://github.com/deepseek-ai/deepseek-harness>.
///
/// The argv is composed from the launch grammar `dsh --profile <name>
/// [--patch <path>]...` (upstream `apps/cli/src/args.ts:137-140`): the first
/// pair is `--profile <profile>`, then one `--patch <abs path>` pair per
/// configured patch in caller order, each resolved absolute before spawn
/// (Python `Path(patch).expanduser().resolve()` —
/// `python/sdk/src/deepseek_harness/client.py:483-484`). An empty `profile`
/// is rejected locally with [`Error::Config`] before spawn (spec §2.2.6).
///
/// An empty `dsh_bin` and an empty `DSH_RUNTIME_BIN` both count as absent
/// (Python truthiness), so resolution never produces an unlaunchable empty
/// program.
pub fn resolve_runtime(config: &Config) -> Result<RuntimeLaunch, Error> {
    resolve_runtime_with(config, env_var_non_empty)
}

/// `resolve_runtime` with an injectable parent-environment lookup (a test
/// seam: unit tests supply a fixed map instead of the process env, so no
/// test mutates global state).
fn resolve_runtime_with(
    config: &Config,
    lookup: impl Fn(&str) -> Option<String>,
) -> Result<RuntimeLaunch, Error> {
    // The profile is validated before the runtime lookup so an empty/blank
    // profile is rejected locally with a configuration error (spec §2.2.6,
    // §7) even when no runtime binary is configured anywhere — the
    // missing-runtime error must never mask the profile violation.
    if config.profile.trim().is_empty() {
        return Err(Error::Config(
            "profile must not be empty: dsh --profile <name> is required".to_string(),
        ));
    }
    // 1. explicit Config dsh_bin (Python `_default_launch_args`).
    // 2. Rust-only route: DSH_RUNTIME_BIN from the parent environment.
    // 3. Nothing anywhere → RuntimeNotFound with both acquisition routes.
    let program = if let Some(bin) = config.dsh_bin.as_deref().filter(|bin| !bin.is_empty()) {
        PathBuf::from(bin)
    } else if let Some(bin) = lookup("DSH_RUNTIME_BIN").filter(|bin| !bin.is_empty()) {
        PathBuf::from(bin)
    } else {
        return Err(Error::RuntimeNotFound(RUNTIME_NOT_FOUND_HINT.to_string()));
    };
    // The launch grammar is `dsh --profile <name> [--patch <path>]...`
    // (upstream `apps/cli/src/args.ts:137-140`); remaining argv composition:
    let mut args = vec![OsString::from("--profile"), OsString::from(&config.profile)];
    for patch in &config.patches {
        // Patch paths are resolved absolute before spawn, matching both
        // references (Python `Path(patch).expanduser().resolve()` —
        // `python/sdk/src/deepseek_harness/client.py:483-484`).
        let abs = patch.canonicalize().map_err(Error::Io)?;
        args.push(OsString::from("--patch"));
        args.push(abs.into_os_string());
    }
    Ok(RuntimeLaunch { program, args })
}

/// The environment keys the crate MUST never write into the child
/// environment, under any configuration (spec §4.2). None has a reader
/// upstream: the bundled `cordis.yml` consuming `DSH_CORDIS_CONFIG` was
/// deleted, sessions live under `$DSH_HOME/sessions`, and the workspace cwd
/// reaches the runtime through `initialize.cwd` (spec §4.2 evidence).
const FORBIDDEN_ENV_KEYS: [&str; 3] = ["DSH_CORDIS_CONFIG", "DSH_SESSION_ROOT", "DSH_CWD"];

/// Compose the environment override set injected into the runtime subprocess.
///
/// Returns exactly the applicable override keys, in a stable order: the
/// resolved `DSH_HOME` first, then the caller's `Config::env` entries
/// (verbatim, except that `DSH_HOME` and the three forbidden keys of spec
/// §4.2 are filtered out under any configuration), then
/// `DEEPSEEK_BASE_URL` / `DEEPSEEK_API_KEY` when configured. The caller's
/// `DSH_HOME` is an **input to resolution** (spec §3.2.1), not a
/// post-resolution override: excluding it from the passthrough guarantees
/// the child always receives the resolved absolute, `~`-expanded home
/// (spec §3.2.3). When any other key appears twice, the later entry wins at
/// spawn, so the caller's `Config::env` overrides the crate-injected
/// values on collision (Python `env.update(self.config.env)` ordering —
/// `python/sdk/src/deepseek_harness/client.py:75-77`). Every other variable
/// is inherited wholesale from the parent environment by the spawn layer.
///
/// The crate never writes `DSH_CORDIS_CONFIG`, `DSH_SESSION_ROOT`, or
/// `DSH_CWD` — none has a reader upstream (spec §4.2).
pub fn compose_env(config: &Config) -> Result<Vec<(String, String)>, Error> {
    compose_env_with(config, env_var_non_empty)
}

/// `compose_env` with an injectable parent-environment lookup (a test seam;
/// see [`resolve_runtime_with`]).
fn compose_env_with(
    config: &Config,
    lookup: impl Fn(&str) -> Option<String>,
) -> Result<Vec<(String, String)>, Error> {
    // The resolved home is injected first; `DSH_HOME` is excluded from the
    // caller-env passthrough below (PM resolution 2026-09-08), so no later
    // entry can replace it — the child always receives the resolved
    // absolute, `~`-expanded home.
    let mut envs = vec![(
        "DSH_HOME".to_string(),
        resolve_dsh_home_with(config, &lookup)
            .to_string_lossy()
            .into_owned(),
    )];
    if let Some(extra) = &config.env {
        envs.extend(
            extra
                .iter()
                // Spec §4.2 forbids the three keys under any configuration,
                // which overrides the §4.1 verbatim rule for those names:
                // they are filtered out even when the caller supplies them.
                // `DSH_HOME` gets the same treatment (PM resolution
                // 2026-09-08): the caller's `DSH_HOME` is an input to
                // resolution (spec §3.2.1), not a post-resolution override,
                // so the child always receives the resolved absolute,
                // `~`-expanded home (spec §3.2.3).
                .filter(|(key, _)| {
                    !FORBIDDEN_ENV_KEYS.contains(&key.as_str()) && key.as_str() != "DSH_HOME"
                })
                .map(|(key, value)| (key.clone(), value.clone())),
        );
    }
    if let Some(url) = &config.base_url {
        envs.push(("DEEPSEEK_BASE_URL".to_string(), url.clone()));
    }
    if let Some(key) = &config.api_key {
        envs.push(("DEEPSEEK_API_KEY".to_string(), key.clone()));
    }
    Ok(envs)
}

impl Config {
    /// Resolve the harness home (spec §3.1/§3.2), highest precedence first:
    ///
    /// 1. [`Config::dsh_home`] (an explicit configured path);
    /// 2. a non-empty `DSH_HOME` — first from [`Config::env`], then from the
    ///    injected `parent_env` (the parent process environment);
    /// 3. `~/.dsh`.
    ///
    /// Blank/whitespace-only `DSH_HOME` counts as **unset**, exactly as
    /// upstream does (`packages/util/home-paths/src/index.ts:88-89`). The
    /// result is normalized to an absolute path with `~` expanded (upstream
    /// `resolve(expandHomePath(selected))`,
    /// `packages/util/home-paths/src/index.ts:87-91`).
    ///
    /// Pure: no filesystem side effects, so the resolution matrix is
    /// testable with an injected `parent_env` map without touching a real
    /// home (spec §9.2). The caller observes the selected home through this
    /// accessor (spec §3.2.4); directory creation happens at boot (spec
    /// §3.2.5).
    pub fn resolve_dsh_home(&self, parent_env: &HashMap<String, String>) -> PathBuf {
        resolve_dsh_home_with(self, &|name| parent_env.get(name).cloned())
    }
}

/// `Config::resolve_dsh_home` with an injectable parent-environment lookup
/// (a test seam; see [`resolve_runtime_with`]).
fn resolve_dsh_home_with(config: &Config, lookup: &impl Fn(&str) -> Option<String>) -> PathBuf {
    // 1. an explicit configured path (upstream `configured ?? ...`).
    if let Some(home) = &config.dsh_home {
        return normalize_home(home);
    }
    // 2. a non-empty $DSH_HOME — first from Config::env, then the parent
    //    environment; blank/whitespace-only counts as unset (upstream
    //    `fromEnv !== undefined && fromEnv.trim().length > 0 ? fromEnv :
    //    defaultDshHome()`, `packages/util/home-paths/src/index.ts:88-89`).
    let from_env = config
        .env
        .as_ref()
        .and_then(|extra| extra.get("DSH_HOME"))
        .filter(|value| !value.trim().is_empty())
        .cloned()
        .or_else(|| lookup("DSH_HOME").filter(|value| !value.trim().is_empty()));
    if let Some(home) = from_env {
        return normalize_home(Path::new(&home));
    }
    // 3. ~/.dsh (upstream `defaultDshHome`,
    //    `packages/util/home-paths/src/index.ts:62`).
    let default = std::env::home_dir()
        .map(|home| home.join(".dsh"))
        .unwrap_or_else(|| PathBuf::from(".dsh"));
    normalize_home(&default)
}

/// Normalize a selected home to an absolute path with `~` expanded
/// (upstream `resolve(expandHomePath(selected))`).
fn normalize_home(path: &Path) -> PathBuf {
    let expanded = expand_home(path);
    if expanded.is_absolute() {
        expanded
    } else {
        std::path::absolute(&expanded).unwrap_or(expanded)
    }
}

/// Expand a leading `~` (or `~/...`) to the user's home directory (upstream
/// `expandHomePath`, `packages/util/home-paths/src/index.ts:70-74`).
fn expand_home(path: &Path) -> PathBuf {
    let mut components = path.components();
    match components.next() {
        Some(std::path::Component::Normal(segment)) if segment == "~" => {
            let rest: PathBuf = components.as_path().to_path_buf();
            match std::env::home_dir() {
                Some(home) => home.join(rest),
                None => path.to_path_buf(),
            }
        }
        _ => path.to_path_buf(),
    }
}

/// Read a non-empty parent-environment variable, or `None` when unset or
/// empty (empty string counts as absent — Python truthiness).
fn env_var_non_empty(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::ffi::OsString;
    use std::path::PathBuf;

    /// Test lookup from a fixed map; keeps unit tests free of process-global
    /// env mutation (and thus race-free under parallel `cargo test`).
    fn lookup<'a>(
        env: &'a HashMap<&'static str, &'static str>,
    ) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| env.get(name).map(|value| value.to_string())
    }

    /// A unique temp directory for patch-resolution tests (real files:
    /// patch paths are canonicalized absolute before spawn).
    fn temp_patch_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "dsh-sdk-patch-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    // --- resolve_runtime: launch grammar + AC10 ---------------------------

    #[test]
    fn resolve_default_launch_argv_is_exactly_profile_sdk() {
        let env: HashMap<&'static str, &'static str> =
            HashMap::from([("DSH_RUNTIME_BIN", "/usr/local/bin/dsh")]);
        let launch = resolve_runtime_with(&Config::default(), lookup(&env)).unwrap();
        assert_eq!(launch.program, PathBuf::from("/usr/local/bin/dsh"));
        assert_eq!(
            launch.args,
            vec![OsString::from("--profile"), OsString::from("sdk")],
            "the default launch argv must be exactly --profile sdk"
        );
    }

    #[test]
    fn resolve_patches_produce_ordered_abs_patch_pairs() {
        let dir = temp_patch_dir();
        let first = dir.join("first.yml");
        let second = dir.join("second.yml");
        std::fs::write(&first, "").unwrap();
        std::fs::write(&second, "").unwrap();
        let env: HashMap<&'static str, &'static str> = HashMap::from([("DSH_RUNTIME_BIN", "dsh")]);
        let config = Config {
            patches: vec![first.clone(), second.clone()],
            ..Config::default()
        };
        let launch = resolve_runtime_with(&config, lookup(&env)).unwrap();
        assert_eq!(
            launch.args,
            vec![
                OsString::from("--profile"),
                OsString::from("sdk"),
                OsString::from("--patch"),
                first.canonicalize().unwrap().into_os_string(),
                OsString::from("--patch"),
                second.canonicalize().unwrap().into_os_string(),
            ],
            "patches must follow the profile pair as ordered --patch <abs> pairs"
        );
        // A relative patch is resolved absolute before spawn too.
        let config = Config {
            patches: vec![PathBuf::from("Cargo.toml")],
            ..Config::default()
        };
        let launch = resolve_runtime_with(&config, lookup(&env)).unwrap();
        let expected = PathBuf::from("Cargo.toml").canonicalize().unwrap();
        assert!(expected.is_absolute());
        assert_eq!(launch.args[3], expected.into_os_string());
    }

    #[test]
    fn resolve_empty_profile_is_rejected_locally() {
        let env: HashMap<&'static str, &'static str> = HashMap::from([("DSH_RUNTIME_BIN", "dsh")]);
        for profile in ["", "   "] {
            let config = Config {
                profile: profile.to_string(),
                ..Config::default()
            };
            assert!(
                matches!(
                    resolve_runtime_with(&config, lookup(&env)),
                    Err(Error::Config(_))
                ),
                "an empty/blank profile must be rejected locally before spawn"
            );
        }
    }

    #[test]
    fn resolve_combined_invalid_profile_and_missing_runtime_is_config_error() {
        // An empty profile with no runtime binary anywhere must fail as a
        // local configuration error (spec §2.2.6, §7): the missing-runtime
        // lookup must not mask the profile violation.
        let empty_env = HashMap::new();
        let config = Config {
            profile: "".to_string(),
            ..Config::default()
        };
        assert!(matches!(
            resolve_runtime_with(&config, lookup(&empty_env)),
            Err(Error::Config(_))
        ));
    }

    #[test]
    fn resolve_dsh_bin_wins_over_env() {
        let env: HashMap<&'static str, &'static str> =
            HashMap::from([("DSH_RUNTIME_BIN", "env-bin")]);
        let config = Config {
            dsh_bin: Some("config-bin".into()),
            ..Config::default()
        };
        assert_eq!(
            resolve_runtime_with(&config, lookup(&env)).unwrap(),
            RuntimeLaunch {
                program: PathBuf::from("config-bin"),
                args: vec![OsString::from("--profile"), OsString::from("sdk")],
            }
        );
    }

    #[test]
    fn resolve_env_bin_used_when_nothing_configured() {
        let env: HashMap<&'static str, &'static str> =
            HashMap::from([("DSH_RUNTIME_BIN", "env-bin")]);
        assert_eq!(
            resolve_runtime_with(&Config::default(), lookup(&env)).unwrap(),
            RuntimeLaunch {
                program: PathBuf::from("env-bin"),
                args: vec![OsString::from("--profile"), OsString::from("sdk")],
            }
        );
    }

    #[test]
    fn resolve_empty_dsh_bin_and_env_bin_count_as_absent() {
        // An empty Config::dsh_bin is falsy (Python truthiness) → the parent
        // DSH_RUNTIME_BIN route applies...
        let env: HashMap<&'static str, &'static str> =
            HashMap::from([("DSH_RUNTIME_BIN", "env-bin")]);
        let config = Config {
            dsh_bin: Some("".into()),
            ..Config::default()
        };
        assert_eq!(
            resolve_runtime_with(&config, lookup(&env)).unwrap(),
            RuntimeLaunch {
                program: PathBuf::from("env-bin"),
                args: vec![OsString::from("--profile"), OsString::from("sdk")],
            }
        );
        // ...and an empty DSH_RUNTIME_BIN counts as absent → RuntimeNotFound
        // (never an unlaunchable empty program).
        let env: HashMap<&'static str, &'static str> = HashMap::from([("DSH_RUNTIME_BIN", "")]);
        assert!(matches!(
            resolve_runtime_with(&Config::default(), lookup(&env)),
            Err(Error::RuntimeNotFound(_))
        ));
    }

    #[test]
    fn resolve_missing_everywhere_has_both_routes_and_github_url() {
        let empty_env = HashMap::new();
        let err = resolve_runtime_with(&Config::default(), lookup(&empty_env)).unwrap_err();
        let message = err.to_string();
        assert!(
            message.contains("DSH_RUNTIME_BIN"),
            "bring-your-own route hint missing: {message}"
        );
        assert!(
            message.contains("scripts/build-exe-for-python-sdk.ts"),
            "build route hint missing: {message}"
        );
        assert!(
            message.contains("https://github.com/deepseek-ai/deepseek-harness"),
            "GitHub URL missing: {message}"
        );
        assert!(
            !message.contains("crate"),
            "must not advertise a (not-yet-published) companion crate: {message}"
        );
        assert!(
            !message.contains("wheel"),
            "must not advertise a Python wheel as an install path: {message}"
        );
    }

    // --- compose_env ------------------------------------------------------

    #[test]
    fn compose_env_contains_dsh_home_and_never_forbidden_keys() {
        let env: HashMap<&'static str, &'static str> = HashMap::from([("DSH_HOME", "/home/sdk")]);
        let map: HashMap<_, _> = compose_env_with(&Config::default(), lookup(&env))
            .unwrap()
            .into_iter()
            .collect();
        assert_eq!(map.get("DSH_HOME").map(String::as_str), Some("/home/sdk"));
        for forbidden in ["DSH_CORDIS_CONFIG", "DSH_SESSION_ROOT", "DSH_CWD"] {
            assert!(
                !map.contains_key(forbidden),
                "{forbidden} must never be written into the child env: {map:?}"
            );
        }
    }

    #[test]
    fn compose_env_filters_forbidden_keys_from_caller_env() {
        // Spec §4.2: the three forbidden keys MUST NOT be written under any
        // configuration — even when the caller supplies them in
        // `Config::env`, they are filtered before the override list is
        // built. Every other caller entry still flows through verbatim.
        let empty_env = HashMap::new();
        let config = Config {
            env: Some(HashMap::from([
                ("DSH_CWD".into(), "/tmp/caller-cwd".into()),
                ("DSH_CORDIS_CONFIG".into(), "/tmp/caller-cordis.yml".into()),
                ("DSH_SESSION_ROOT".into(), "/tmp/caller-sessions".into()),
                ("FOO".into(), "bar".into()),
            ])),
            ..Config::default()
        };
        let map: HashMap<_, _> = compose_env_with(&config, lookup(&empty_env))
            .unwrap()
            .into_iter()
            .collect();
        for forbidden in ["DSH_CORDIS_CONFIG", "DSH_SESSION_ROOT", "DSH_CWD"] {
            assert!(
                !map.contains_key(forbidden),
                "{forbidden} must not pass through even when caller-supplied: {map:?}"
            );
        }
        assert_eq!(
            map.get("FOO").map(String::as_str),
            Some("bar"),
            "allowed caller env entries still flow through verbatim"
        );
    }

    #[test]
    fn compose_env_falls_back_to_default_home_when_nothing_set() {
        let empty_env = HashMap::new();
        let map: HashMap<_, _> = compose_env_with(&Config::default(), lookup(&empty_env))
            .unwrap()
            .into_iter()
            .collect();
        let home = map.get("DSH_HOME").expect("a home always resolves");
        assert!(
            !home.trim().is_empty(),
            "the resolved home must be non-empty: {map:?}"
        );
    }

    #[test]
    fn compose_env_caller_dsh_home_selects_home_via_resolution() {
        // PM resolution (2026-09-08): the caller's DSH_HOME is an input to
        // resolution (spec §3.2.1), not a post-resolution override. It is
        // excluded from the verbatim passthrough, so the child env carries
        // the resolved value — here the caller's Config::env DSH_HOME
        // selects the home (it is checked before the parent env),
        // normalized absolute.
        let env: HashMap<&'static str, &'static str> =
            HashMap::from([("DSH_HOME", "/inherited/home")]);
        let config = Config {
            env: Some(HashMap::from([("DSH_HOME".into(), "/caller/home".into())])),
            ..Config::default()
        };
        let map: HashMap<_, _> = compose_env_with(&config, lookup(&env))
            .unwrap()
            .into_iter()
            .collect();
        assert_eq!(
            map.get("DSH_HOME").map(String::as_str),
            Some("/caller/home"),
            "the caller's Config::env DSH_HOME selects the home via resolution; \
             the child env carries the resolved value"
        );
    }

    #[test]
    fn compose_env_blank_caller_dsh_home_never_reaches_child() {
        // A blank/whitespace caller DSH_HOME counts as unset in resolution
        // (spec §3.2.2) and is excluded from the passthrough, so the child
        // env carries the resolved home — never the blank value.
        let env: HashMap<&'static str, &'static str> =
            HashMap::from([("DSH_HOME", "/inherited/home")]);
        for blank in ["", "   ", "\t"] {
            let config = Config {
                env: Some(HashMap::from([("DSH_HOME".into(), blank.to_string())])),
                ..Config::default()
            };
            let map: HashMap<_, _> = compose_env_with(&config, lookup(&env))
                .unwrap()
                .into_iter()
                .collect();
            assert_eq!(
                map.get("DSH_HOME").map(String::as_str),
                Some("/inherited/home"),
                "a blank caller DSH_HOME {blank:?} must fall through to the \
                 parent env in the composed child env"
            );
        }
    }

    #[test]
    fn compose_env_tilde_relative_caller_dsh_home_is_expanded_in_child() {
        // A `~`-relative caller DSH_HOME selects the home via resolution
        // and is excluded from the passthrough: the child env carries the
        // resolved absolute, `~`-expanded value (spec §3.2.3).
        let empty_env = HashMap::new();
        let config = Config {
            env: Some(HashMap::from([(
                "DSH_HOME".into(),
                "~/dsh-test-home".into(),
            )])),
            ..Config::default()
        };
        let map: HashMap<_, _> = compose_env_with(&config, lookup(&empty_env))
            .unwrap()
            .into_iter()
            .collect();
        let home = map.get("DSH_HOME").expect("a home always resolves");
        assert!(
            Path::new(home).is_absolute(),
            "the child DSH_HOME must be absolute: {home:?}"
        );
        assert!(
            !home.contains('~'),
            "the child DSH_HOME must have ~ expanded: {home:?}"
        );
        assert_eq!(
            home,
            &default_home()
                .parent()
                .unwrap()
                .join("dsh-test-home")
                .to_string_lossy()
                .into_owned(),
            "the child DSH_HOME must be the resolved ~-expanded home"
        );
    }

    #[test]
    fn compose_env_relative_caller_dsh_home_is_resolved_absolute_in_child() {
        // A relative caller DSH_HOME selects the home via resolution and is
        // excluded from the passthrough: the child env carries the resolved
        // absolute value (spec §3.2.3), never the raw relative string.
        let empty_env = HashMap::new();
        let config = Config {
            env: Some(HashMap::from([("DSH_HOME".into(), "relative/home".into())])),
            ..Config::default()
        };
        let map: HashMap<_, _> = compose_env_with(&config, lookup(&empty_env))
            .unwrap()
            .into_iter()
            .collect();
        let home = map.get("DSH_HOME").expect("a home always resolves");
        assert!(
            Path::new(home).is_absolute(),
            "the child DSH_HOME must be absolute: {home:?}"
        );
        assert_ne!(
            home, "relative/home",
            "the raw relative caller value must never reach the child"
        );
    }

    #[test]
    fn compose_env_explicit_dsh_home_wins_over_conflicting_caller_env() {
        // Explicit Config::dsh_home is the highest-precedence selection rule
        // (spec §3.1); a conflicting caller DSH_HOME is excluded from the
        // passthrough, so the composed child env carries the explicit home.
        let empty_env = HashMap::new();
        let config = Config {
            dsh_home: Some(PathBuf::from("/configured/home")),
            env: Some(HashMap::from([("DSH_HOME".into(), "/caller/home".into())])),
            ..Config::default()
        };
        let map: HashMap<_, _> = compose_env_with(&config, lookup(&empty_env))
            .unwrap()
            .into_iter()
            .collect();
        assert_eq!(
            map.get("DSH_HOME").map(String::as_str),
            Some("/configured/home"),
            "explicit Config::dsh_home must win in the composed child env"
        );
    }

    #[test]
    fn compose_env_injects_deepseek_keys_when_configured() {
        let empty_env = HashMap::new();
        let config = Config {
            base_url: Some("https://api.example".into()),
            api_key: Some("sk-test".into()),
            ..Config::default()
        };
        let map: HashMap<_, _> = compose_env_with(&config, lookup(&empty_env))
            .unwrap()
            .into_iter()
            .collect();
        assert_eq!(
            map.get("DEEPSEEK_BASE_URL").map(String::as_str),
            Some("https://api.example")
        );
        assert_eq!(
            map.get("DEEPSEEK_API_KEY").map(String::as_str),
            Some("sk-test")
        );
    }

    #[test]
    fn compose_env_user_env_flows_through() {
        let empty_env = HashMap::new();
        let config = Config {
            env: Some(HashMap::from([
                ("FOO".into(), "bar".into()),
                ("DEEPSEEK_API_KEY".into(), "user-key".into()),
            ])),
            ..Config::default()
        };
        let map: HashMap<_, _> = compose_env_with(&config, lookup(&empty_env))
            .unwrap()
            .into_iter()
            .collect();
        assert_eq!(map.get("FOO").map(String::as_str), Some("bar"));
        assert_eq!(
            map.get("DEEPSEEK_API_KEY").map(String::as_str),
            Some("user-key"),
            "caller env entries flow through verbatim"
        );
    }

    // --- DSH_HOME resolution matrix (spec §3) ---------------------------

    /// The default `~/.dsh` fallback for the current user.
    fn default_home() -> PathBuf {
        std::env::home_dir()
            .map(|home| home.join(".dsh"))
            .unwrap_or_else(|| PathBuf::from(".dsh"))
    }

    #[test]
    fn resolve_home_explicit_dsh_home_wins_over_inherited() {
        // (a) an explicit Config::dsh_home wins over an inherited DSH_HOME
        // (spec §3.1, rule 1 over rules 2-3).
        let config = Config {
            dsh_home: Some(PathBuf::from("/configured/home")),
            env: Some(HashMap::from([("DSH_HOME".into(), "/env/home".into())])),
            ..Config::default()
        };
        let parent = HashMap::from([("DSH_HOME".to_string(), "/inherited/home".to_string())]);
        assert_eq!(
            config.resolve_dsh_home(&parent),
            PathBuf::from("/configured/home")
        );
    }

    #[test]
    fn resolve_home_inherited_dsh_home_used_when_unconfigured() {
        // (b) a non-empty inherited DSH_HOME is used when dsh_home is None
        // (spec §3.1, rule 2).
        let config = Config::default();
        let parent = HashMap::from([("DSH_HOME".to_string(), "/inherited/home".to_string())]);
        assert_eq!(
            config.resolve_dsh_home(&parent),
            PathBuf::from("/inherited/home")
        );
    }

    #[test]
    fn resolve_home_blank_dsh_home_falls_through_to_default() {
        // (c) blank/whitespace-only DSH_HOME counts as unset and falls
        // through to ~/.dsh (spec §3.1, §3.2.2; upstream
        // `packages/util/home-paths/src/index.ts:88-89`).
        for blank in ["", "   ", "\t"] {
            let parent = HashMap::from([("DSH_HOME".to_string(), blank.to_string())]);
            assert_eq!(
                Config::default().resolve_dsh_home(&parent),
                default_home(),
                "blank DSH_HOME {blank:?} must count as unset"
            );
        }
        // The same blank rule applies to a DSH_HOME inside Config::env: it
        // falls through to the parent environment.
        let config = Config {
            env: Some(HashMap::from([("DSH_HOME".into(), "   ".into())])),
            ..Config::default()
        };
        let parent = HashMap::from([("DSH_HOME".to_string(), "/inherited/home".to_string())]);
        assert_eq!(
            config.resolve_dsh_home(&parent),
            PathBuf::from("/inherited/home"),
            "a blank Config::env DSH_HOME must fall through to the parent env"
        );
    }

    #[test]
    fn resolve_home_nothing_set_resolves_default() {
        // (d) nothing configured anywhere → ~/.dsh (spec §3.1, rule 3; the
        // deliberate divergence from Python's refusal, spec §3.3).
        let parent = HashMap::new();
        assert_eq!(Config::default().resolve_dsh_home(&parent), default_home());
    }

    #[test]
    fn resolve_home_is_absolute_with_tilde_expanded() {
        // (e) the resolved home is absolute with `~` expanded (spec §3.2.3;
        // upstream `resolve(expandHomePath(selected))`).
        let parent = HashMap::from([("DSH_HOME".to_string(), "~/dsh-test-home".to_string())]);
        let home = Config::default().resolve_dsh_home(&parent);
        assert!(
            home.is_absolute(),
            "resolved home must be absolute: {home:?}"
        );
        assert!(
            !home.to_string_lossy().contains('~'),
            "resolved home must have ~ expanded: {home:?}"
        );
        assert_eq!(home, default_home().parent().unwrap().join("dsh-test-home"));
        // The same normalization applies to an explicit Config::dsh_home.
        let config = Config {
            dsh_home: Some(PathBuf::from("~/dsh-configured")),
            ..Config::default()
        };
        let home = config.resolve_dsh_home(&HashMap::new());
        assert!(
            home.is_absolute(),
            "resolved home must be absolute: {home:?}"
        );
        assert!(
            !home.to_string_lossy().contains('~'),
            "resolved home must have ~ expanded: {home:?}"
        );
        assert_eq!(
            home,
            default_home().parent().unwrap().join("dsh-configured")
        );
    }

    #[test]
    fn config_default_profile_is_sdk() {
        // (f) Config::default().profile == "sdk" (spec §6.1; Python
        // `python/sdk/src/deepseek_harness/api.py:30`).
        assert_eq!(Config::default().profile, "sdk");
    }

    // --- Config Debug redaction ------------------------------------------

    #[test]
    fn config_debug_redacts_api_key_and_env_credential() {
        let config = Config {
            api_key: Some("sk-super-secret".into()),
            env: Some(HashMap::from([(
                "DEEPSEEK_API_KEY".into(),
                "env-super-secret".into(),
            )])),
            ..Config::default()
        };
        let rendered = format!("{config:?}");
        assert!(
            !rendered.contains("sk-super-secret"),
            "Config Debug must redact api_key: {rendered}"
        );
        assert!(
            !rendered.contains("env-super-secret"),
            "Config Debug must redact a DEEPSEEK_API_KEY env entry: {rendered}"
        );
        assert!(
            rendered.contains("<redacted>"),
            "the redaction marker must be visible: {rendered}"
        );
    }
}
