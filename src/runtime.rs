//! Runtime binary resolution and environment injection (Plan A: the caller
//! brings their own runtime binary; this crate does not download or bundle
//! one).
//!
//! [`resolve_runtime`] picks the launch command with Python `HarnessClient`
//! parity:
//!
//! 1. `Config::launch_args_override` (non-empty) — the whole argv, verbatim;
//! 2. `Config::runtime_bin`;
//! 3. `DSH_RUNTIME_BIN` from the parent environment;
//! 4. otherwise [`Error::RuntimeNotFound`] with acquisition hints.
//!
//! [`compose_env`] builds the override set injected into the runtime
//! subprocess: the SDK keys win over the caller's extra `Config::env` entries
//! (Python `dict.update` semantics) and the parent environment is inherited
//! wholesale for every key not in the set. When no effective
//! `DSH_CORDIS_CONFIG` exists, the SDK-bundled default config
//! ([`bundled_default_config_path`], a byte-identical copy of the official
//! runtime's default `cordis.yml`) is injected.
//!
//! The official runtime and its sources live at
//! <https://github.com/deepseek-ai/deepseek-harness>.

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::LazyLock;
use std::time::Duration;

use crate::client::ClientTimeouts;
use crate::error::Error;

/// Bundled default runtime configuration. Byte-identical to the official
/// DeepSeek Harness default `cordis.yml`; extracted to a temp location on
/// first use via [`bundled_default_config_path`].
const DEFAULT_CORDIS_YML: &[u8] = include_bytes!("../assets/cordis.yml");

/// Acquisition hints embedded in [`Error::RuntimeNotFound`] when no runtime
/// binary is configured anywhere. Names the bring-your-own route and the
/// build-from-source route; must never advertise a not-yet-published Rust
/// companion crate or a Python wheel as a v0.1 install path.
const RUNTIME_NOT_FOUND_HINT: &str = "no DeepSeek Harness runtime binary is configured. \
Bring your own: set DSH_RUNTIME_BIN (or Config::runtime_bin / launch_args_override) to a \
runtime binary you already have, or build the official runtime from the deepseek-harness \
repository (https://github.com/deepseek-ai/deepseek-harness) with \
`scripts/build-exe-for-python-sdk.ts` and point DSH_RUNTIME_BIN at the built executable";

/// Cached temp path of the extracted default config. The path's initializer
/// is known at declaration time, so the cell and initializer live together.
static DEFAULT_CONFIG_PATH: LazyLock<PathBuf> = LazyLock::new(|| {
    std::env::temp_dir().join(format!(
        "deepseek-harness-sdk-{}-cordis.yml",
        env!("CARGO_PKG_VERSION")
    ))
});

/// Monotonic counter making extraction temp names unique per attempt, so
/// concurrent first-use extractions (separate threads or processes) never
/// collide on the temp sibling.
static EXTRACT_TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

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
    /// Provider name sent to the runtime on `initialize`
    /// (Python default `"deepseek-official"`).
    pub provider: String,
    /// Model name sent to the runtime on `initialize`
    /// (Python default `"deepseek-v4-flash"`).
    pub model: String,
    /// Optional `maxTokens` for `initialize` (rejected when `0`).
    pub max_tokens: Option<u32>,
    /// Working directory for the agent; defaults to the current directory,
    /// resolved absolute. Feeds `DSH_CWD` and `initialize.cwd`.
    pub cwd: Option<PathBuf>,
    /// Subprocess working directory for the runtime; defaults to `cwd`
    /// (Python parity).
    pub runtime_cwd: Option<PathBuf>,
    /// Path to (or name of) a runtime binary the caller already has.
    pub runtime_bin: Option<String>,
    /// Complete launch argv (program + arguments) replacing the resolved
    /// binary verbatim; an empty list counts as unset (Python truthiness).
    pub launch_args_override: Option<Vec<String>>,
    /// Explicit path for `DSH_CORDIS_CONFIG`; an empty string counts as
    /// absent (Python truthiness) and falls back to the bundled default.
    pub cordis_config: Option<String>,
    /// Overrides the inherited `DEEPSEEK_BASE_URL`.
    pub base_url: Option<String>,
    /// Overrides the inherited `DEEPSEEK_API_KEY`.
    pub api_key: Option<String>,
    /// Session root directory, injected as `DSH_SESSION_ROOT`.
    pub session_root: Option<String>,
    /// Extra environment entries layered over the parent environment; SDK
    /// keys (`DSH_CWD`, `DSH_SESSION_ROOT`, `DSH_CORDIS_CONFIG`,
    /// `DEEPSEEK_BASE_URL`, `DEEPSEEK_API_KEY`) win on collision.
    pub env: Option<HashMap<String, String>>,
    /// Per-request response deadline; `None` waits indefinitely (Python
    /// default).
    pub request_timeout: Option<Duration>,
    /// Close-ladder timeouts via plan 01 [`ClientTimeouts`].
    pub timeouts: ClientTimeouts,
}

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("provider", &self.provider)
            .field("model", &self.model)
            .field("max_tokens", &self.max_tokens)
            .field("cwd", &self.cwd)
            .field("runtime_cwd", &self.runtime_cwd)
            .field("runtime_bin", &self.runtime_bin)
            .field("launch_args_override", &self.launch_args_override)
            .field("cordis_config", &self.cordis_config)
            .field("base_url", &self.base_url)
            .field("api_key", &self.api_key.as_deref().map(|_| "<redacted>"))
            .field("session_root", &self.session_root)
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
            .field("request_timeout", &self.request_timeout)
            .field("timeouts", &self.timeouts)
            .finish()
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            provider: "deepseek-official".into(),
            model: "deepseek-v4-flash".into(),
            max_tokens: None,
            cwd: None,
            runtime_cwd: None,
            runtime_bin: None,
            launch_args_override: None,
            cordis_config: None,
            base_url: None,
            api_key: None,
            session_root: None,
            env: None,
            request_timeout: None,
            timeouts: ClientTimeouts::default(),
        }
    }
}

/// The resolved runtime launch command, feeding plan 01
/// [`LaunchSpec`](crate::client::LaunchSpec) (`program` + `args`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeLaunch {
    /// Path to (or name of) the runtime executable.
    pub program: String,
    /// Extra command-line arguments passed to the runtime.
    pub args: Vec<String>,
}

/// Resolve the runtime launch command for a [`Config`].
///
/// Precedence (Python `HarnessClient` parity, plus the Rust-only
/// `DSH_RUNTIME_BIN` route):
///
/// 1. `Config::launch_args_override` (non-empty) — the whole argv, verbatim;
/// 2. `Config::runtime_bin`;
/// 3. `DSH_RUNTIME_BIN` from the parent environment;
/// 4. [`Error::RuntimeNotFound`] whose message names both acquisition routes
///    (bring-your-own, and building the official runtime via
///    `scripts/build-exe-for-python-sdk.ts`) and cites
///    <https://github.com/deepseek-ai/deepseek-harness>.
///
/// An empty `launch_args_override` list and an empty `DSH_RUNTIME_BIN` both
/// count as absent (Python truthiness), so resolution never produces an
/// unlaunchable empty program.
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
    // 1. launch_args_override is the whole argv, verbatim (Python
    //    `launch_args_override or _default_launch_args()`; an empty list is
    //    falsy → unset).
    if let Some(argv) = config
        .launch_args_override
        .as_deref()
        .filter(|argv| !argv.is_empty())
    {
        return Ok(RuntimeLaunch {
            program: argv[0].clone(),
            args: argv[1..].to_vec(),
        });
    }
    // 2. explicit Config runtime_bin (Python `_default_launch_args`).
    if let Some(bin) = config.runtime_bin.as_deref().filter(|bin| !bin.is_empty()) {
        return Ok(RuntimeLaunch {
            program: bin.to_string(),
            args: Vec::new(),
        });
    }
    // 3. Rust-only route: DSH_RUNTIME_BIN from the parent environment.
    if let Some(bin) = lookup("DSH_RUNTIME_BIN").filter(|bin| !bin.is_empty()) {
        return Ok(RuntimeLaunch {
            program: bin,
            args: Vec::new(),
        });
    }
    // 4. Nothing anywhere → RuntimeNotFound with both acquisition routes.
    Err(Error::RuntimeNotFound(RUNTIME_NOT_FOUND_HINT.to_string()))
}

/// Compose the environment override set injected into the runtime subprocess.
///
/// Returns exactly the applicable override keys, in a stable order:
/// caller `Config::env` entries first, then the SDK keys — `DSH_CWD`
/// (always, from `resolved_cwd`), then `DSH_SESSION_ROOT`, `DSH_CORDIS_CONFIG`,
/// `DEEPSEEK_BASE_URL`, `DEEPSEEK_API_KEY`, each only when configured. When a
/// key appears twice, the later entry wins (Python `dict.update` semantics),
/// so SDK keys override caller-provided env entries. Every other variable is
/// inherited wholesale from the parent environment by the spawn layer.
///
/// `DSH_CORDIS_CONFIG` resolution, Python-parity (`env.get` truthiness):
/// `Config::cordis_config` (non-empty) wins; otherwise a non-empty
/// `DSH_CORDIS_CONFIG` in `Config::env`, then in the parent environment, is
/// inherited as-is (not part of the override set); empty strings count as
/// absent — an empty-string `Config::env` entry is skipped on copy so it can
/// never clobber a non-empty parent value. When nothing effective exists,
/// the bundled default config ([`bundled_default_config_path`]) is injected.
///
/// Deliberate divergence from the Python SDK (documented, do not "fix" to
/// match Python): Python injects the bundled default only when the bundled
/// runtime carrier is used. This crate is Plan A (bring-your-own runtime) —
/// there is no bundled carrier and the runtime requires an explicit config —
/// so the default is injected whenever no effective `DSH_CORDIS_CONFIG`
/// exists, regardless of how the runtime was resolved.
///
/// `resolved_cwd` must be the absolute, resolved working directory (Python:
/// `str(Path(cwd).resolve())`).
///
/// Returns [`Error::Io`] when the bundled default config cannot be extracted
/// or verified — the required default-config injection never degrades
/// silently to a config-less launch; [`crate::DeepSeekHarness::start`]
/// propagates the failure.
pub fn compose_env(config: &Config, resolved_cwd: &Path) -> Result<Vec<(String, String)>, Error> {
    compose_env_with(config, resolved_cwd, env_var_non_empty)
}

/// `compose_env` with an injectable parent-environment lookup (a test seam;
/// see [`resolve_runtime_with`]).
fn compose_env_with(
    config: &Config,
    resolved_cwd: &Path,
    lookup: impl Fn(&str) -> Option<String>,
) -> Result<Vec<(String, String)>, Error> {
    // Python `env = dict(self.config.env)`: caller extra env first. An
    // empty-string `DSH_CORDIS_CONFIG` is skipped on copy — the documented
    // truthiness rule treats empty as absent, so the empty pair must not
    // clobber a non-empty parent value at spawn (the resolution below
    // already treats it as absent for the injection decision).
    let mut envs: Vec<(String, String)> = Vec::new();
    if let Some(extra) = &config.env {
        envs.extend(
            extra
                .iter()
                .filter(|(key, value)| !(*key == "DSH_CORDIS_CONFIG" && value.is_empty()))
                .map(|(key, value)| (key.clone(), value.clone())),
        );
    }
    // ...then the SDK keys, so they win on collision (later entry wins).
    // DSH_CWD is always injected from the resolved working directory.
    envs.push((
        "DSH_CWD".to_string(),
        resolved_cwd.to_string_lossy().into_owned(),
    ));
    if let Some(root) = &config.session_root {
        envs.push(("DSH_SESSION_ROOT".to_string(), root.clone()));
    }
    // DSH_CORDIS_CONFIG, Python-parity truthiness: a non-empty
    // Config::cordis_config wins; otherwise a non-empty value already in the
    // set (Config::env) or in the parent environment is inherited as-is —
    // empty strings count as absent, in which case the bundled default is
    // injected (deliberate Plan A divergence, documented on `compose_env`).
    match config
        .cordis_config
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        Some(path) => envs.push(("DSH_CORDIS_CONFIG".to_string(), path.to_string())),
        None => {
            let inherited = config
                .env
                .as_ref()
                .and_then(|extra| extra.get("DSH_CORDIS_CONFIG"))
                .filter(|value| !value.is_empty())
                .cloned()
                .or_else(|| lookup("DSH_CORDIS_CONFIG").filter(|value| !value.is_empty()));
            if inherited.is_none() {
                // The bundled default is required when no effective config
                // exists; an extraction/verification failure propagates
                // (fail-visible — no silent config-less launch).
                let default = bundled_default_config_path()?;
                envs.push((
                    "DSH_CORDIS_CONFIG".to_string(),
                    default.to_string_lossy().into_owned(),
                ));
            }
        }
    }
    if let Some(url) = &config.base_url {
        envs.push(("DEEPSEEK_BASE_URL".to_string(), url.clone()));
    }
    if let Some(key) = &config.api_key {
        envs.push(("DEEPSEEK_API_KEY".to_string(), key.clone()));
    }
    Ok(envs)
}

/// Path of the SDK-bundled default runtime configuration
/// (`assets/cordis.yml`, a byte-identical copy of the official DSH default),
/// extracted into the system temp directory on first use.
///
/// The path is cached for the process lifetime, but the file is **byte
/// verified against the `include_bytes!` source on every use**: a missing,
/// unreadable, or mismatched file (e.g. a locally poisoned file at the
/// deterministic temp path on a shared machine, or a temp cleaner removing
/// the file between launches) triggers an atomic re-extraction. The write is
/// atomic (unique temp sibling + rename), so the destination only ever
/// appears via rename and is always complete. On a sticky temp dir a foreign
/// file cannot be renamed over — the [`Error::Io`] propagates (fail-visible,
/// never a config-less launch).
pub fn bundled_default_config_path() -> Result<PathBuf, Error> {
    let path: &Path = &DEFAULT_CONFIG_PATH;
    // Byte-verify any existing file against the include_bytes! source on
    // every use: the deterministic temp path is world-writable on shared
    // machines, so an unverified file could be a locally poisoned config
    // injected into a process holding DEEPSEEK_API_KEY. An unreadable file
    // counts as unverified too.
    let verified = std::fs::read(path).is_ok_and(|bytes| bytes == DEFAULT_CORDIS_YML);
    if !verified {
        // Write to a unique temp sibling and rename (atomic) so a crashed
        // writer can never leave a truncated config behind, and concurrent
        // first-use extractions never collide on the temp path. The
        // destination only ever appears via rename, so it is always complete.
        let tmp = path.with_extension(format!(
            "tmp-{}-{}",
            std::process::id(),
            EXTRACT_TMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::write(&tmp, DEFAULT_CORDIS_YML).map_err(Error::Io)?;
        std::fs::rename(&tmp, path).map_err(Error::Io)?;
    }
    Ok(path.to_path_buf())
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
    use std::path::Path;

    /// Test lookup from a fixed map; keeps unit tests free of process-global
    /// env mutation (and thus race-free under parallel `cargo test`).
    fn lookup<'a>(
        env: &'a HashMap<&'static str, &'static str>,
    ) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| env.get(name).map(|value| value.to_string())
    }

    // --- resolve_runtime: precedence matrix -------------------------------

    #[test]
    fn resolve_launch_args_override_wins_verbatim_over_bin_and_env() {
        let env: HashMap<&'static str, &'static str> =
            HashMap::from([("DSH_RUNTIME_BIN", "env-bin")]);
        let config = Config {
            runtime_bin: Some("config-bin".into()),
            launch_args_override: Some(vec![
                "/usr/local/bin/dsh-run".into(),
                "--debug".into(),
                "serve".into(),
            ]),
            ..Config::default()
        };
        assert_eq!(
            resolve_runtime_with(&config, lookup(&env)).unwrap(),
            RuntimeLaunch {
                program: "/usr/local/bin/dsh-run".into(),
                args: vec!["--debug".into(), "serve".into()],
            }
        );
    }

    #[test]
    fn resolve_runtime_bin_wins_over_env() {
        let env: HashMap<&'static str, &'static str> =
            HashMap::from([("DSH_RUNTIME_BIN", "env-bin")]);
        let config = Config {
            runtime_bin: Some("config-bin".into()),
            ..Config::default()
        };
        assert_eq!(
            resolve_runtime_with(&config, lookup(&env)).unwrap(),
            RuntimeLaunch {
                program: "config-bin".into(),
                args: vec![]
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
                program: "env-bin".into(),
                args: vec![]
            }
        );
    }

    #[test]
    fn resolve_empty_override_and_env_bin_count_as_absent() {
        // empty launch_args_override falls through to runtime_bin (Python
        // truthiness)...
        let empty_env = HashMap::new();
        let config = Config {
            runtime_bin: Some("config-bin".into()),
            launch_args_override: Some(vec![]),
            ..Config::default()
        };
        assert_eq!(
            resolve_runtime_with(&config, lookup(&empty_env)).unwrap(),
            RuntimeLaunch {
                program: "config-bin".into(),
                args: vec![]
            }
        );
        // ...and an empty DSH_RUNTIME_BIN counts as absent → RuntimeNotFound
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
    fn compose_env_exact_set_when_all_configured() {
        let empty_env = HashMap::new();
        let config = Config {
            session_root: Some("/sessions".into()),
            cordis_config: Some("/cfg/cordis.yml".into()),
            base_url: Some("https://api.example".into()),
            api_key: Some("sk-test".into()),
            ..Config::default()
        };
        let map: HashMap<_, _> =
            compose_env_with(&config, Path::new("/work/dir"), lookup(&empty_env))
                .unwrap()
                .into_iter()
                .collect();
        assert_eq!(
            map.len(),
            5,
            "exactly the applicable override keys: {map:?}"
        );
        assert_eq!(map.get("DSH_CWD").map(String::as_str), Some("/work/dir"));
        assert_eq!(
            map.get("DSH_SESSION_ROOT").map(String::as_str),
            Some("/sessions")
        );
        assert_eq!(
            map.get("DSH_CORDIS_CONFIG").map(String::as_str),
            Some("/cfg/cordis.yml")
        );
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
    fn compose_env_omits_unconfigured_and_injects_default_when_absent() {
        let empty_env = HashMap::new();
        let map: HashMap<_, _> = compose_env_with(
            &Config::default(),
            Path::new("/work/dir"),
            lookup(&empty_env),
        )
        .unwrap()
        .into_iter()
        .collect();
        assert_eq!(
            map.len(),
            2,
            "only DSH_CWD + injected default config: {map:?}"
        );
        assert_eq!(map.get("DSH_CWD").map(String::as_str), Some("/work/dir"));
        let default_path = bundled_default_config_path().unwrap();
        assert_eq!(
            map.get("DSH_CORDIS_CONFIG").map(String::as_str),
            Some(default_path.to_str().unwrap())
        );
    }

    #[test]
    fn compose_env_inherits_parent_cordis_config_without_override_or_default() {
        let env: HashMap<&'static str, &'static str> =
            HashMap::from([("DSH_CORDIS_CONFIG", "/inherited/cordis.yml")]);
        let map: HashMap<_, _> =
            compose_env_with(&Config::default(), Path::new("/work/dir"), lookup(&env))
                .unwrap()
                .into_iter()
                .collect();
        assert_eq!(
            map.len(),
            1,
            "inherited config is not part of the override set: {map:?}"
        );
        assert_eq!(map.get("DSH_CWD").map(String::as_str), Some("/work/dir"));
    }

    #[test]
    fn compose_env_empty_cordis_counts_absent_in_parent_env() {
        let env: HashMap<&'static str, &'static str> = HashMap::from([("DSH_CORDIS_CONFIG", "")]);
        let map: HashMap<_, _> =
            compose_env_with(&Config::default(), Path::new("/work/dir"), lookup(&env))
                .unwrap()
                .into_iter()
                .collect();
        let default_path = bundled_default_config_path().unwrap();
        assert_eq!(
            map.get("DSH_CORDIS_CONFIG").map(String::as_str),
            Some(default_path.to_str().unwrap()),
            "empty inherited DSH_CORDIS_CONFIG counts as absent: {map:?}"
        );
    }

    #[test]
    fn compose_env_empty_config_cordis_counts_absent() {
        let empty_env = HashMap::new();
        let config = Config {
            cordis_config: Some("".into()),
            ..Config::default()
        };
        let map: HashMap<_, _> =
            compose_env_with(&config, Path::new("/work/dir"), lookup(&empty_env))
                .unwrap()
                .into_iter()
                .collect();
        let default_path = bundled_default_config_path().unwrap();
        assert_eq!(
            map.get("DSH_CORDIS_CONFIG").map(String::as_str),
            Some(default_path.to_str().unwrap()),
            "empty Config::cordis_config counts as absent: {map:?}"
        );
    }

    #[test]
    fn compose_env_empty_user_env_cordis_counts_absent() {
        let empty_env = HashMap::new();
        let config = Config {
            env: Some(HashMap::from([("DSH_CORDIS_CONFIG".into(), "".into())])),
            ..Config::default()
        };
        let map: HashMap<_, _> =
            compose_env_with(&config, Path::new("/work/dir"), lookup(&empty_env))
                .unwrap()
                .into_iter()
                .collect();
        let default_path = bundled_default_config_path().unwrap();
        assert_eq!(
            map.get("DSH_CORDIS_CONFIG").map(String::as_str),
            Some(default_path.to_str().unwrap()),
            "empty user-env DSH_CORDIS_CONFIG counts as absent: {map:?}"
        );
    }

    #[test]
    fn compose_env_empty_user_env_cordis_does_not_clobber_parent_value() {
        // An empty-string DSH_CORDIS_CONFIG in Config::env counts as absent
        // (truthiness): it must be skipped on copy so the parent's non-empty
        // value survives the override set instead of being clobbered by an
        // empty pair at spawn. The injection decision (no effective config)
        // is unchanged — the parent value is inherited, so no default is
        // injected either.
        let env: HashMap<&'static str, &'static str> =
            HashMap::from([("DSH_CORDIS_CONFIG", "/parent/cordis.yml")]);
        let config = Config {
            env: Some(HashMap::from([("DSH_CORDIS_CONFIG".into(), "".into())])),
            ..Config::default()
        };
        let map: HashMap<_, _> = compose_env_with(&config, Path::new("/work/dir"), lookup(&env))
            .unwrap()
            .into_iter()
            .collect();
        assert!(
            !map.contains_key("DSH_CORDIS_CONFIG"),
            "no empty DSH_CORDIS_CONFIG pair may be emitted; the parent value must be inherited instead: {map:?}"
        );
    }

    #[test]
    fn compose_env_user_env_flows_through_and_sdk_keys_win() {
        let empty_env = HashMap::new();
        let config = Config {
            api_key: Some("sdk-key".into()),
            cordis_config: Some("/cfg.yml".into()),
            env: Some(HashMap::from([
                ("FOO".into(), "bar".into()),
                ("DSH_CWD".into(), "user-cwd".into()),
                ("DEEPSEEK_API_KEY".into(), "user-key".into()),
                ("DSH_CORDIS_CONFIG".into(), "/user.yml".into()),
            ])),
            ..Config::default()
        };
        let map: HashMap<_, _> =
            compose_env_with(&config, Path::new("/work/dir"), lookup(&empty_env))
                .unwrap()
                .into_iter()
                .collect();
        assert_eq!(map.get("FOO").map(String::as_str), Some("bar"));
        assert_eq!(map.get("DSH_CWD").map(String::as_str), Some("/work/dir"));
        assert_eq!(
            map.get("DEEPSEEK_API_KEY").map(String::as_str),
            Some("sdk-key")
        );
        assert_eq!(
            map.get("DSH_CORDIS_CONFIG").map(String::as_str),
            Some("/cfg.yml")
        );
    }

    // --- bundled_default_config_path -------------------------------------

    #[test]
    fn bundled_default_config_is_extracted_byte_verified_and_recreated() {
        // First use extracts to a readable temp file byte-identical to the
        // include_bytes! source. (Sequenced in one test: the temp file is
        // shared process state, so a parallel test mutating it would race.)
        let first = bundled_default_config_path().unwrap();
        assert!(
            first.is_file(),
            "extracted default config must be a readable file"
        );
        assert_eq!(
            std::fs::read(&first).unwrap(),
            DEFAULT_CORDIS_YML,
            "must be byte-identical to assets/cordis.yml"
        );
        // Subsequent calls reuse the same cached path.
        let second = bundled_default_config_path().unwrap();
        assert_eq!(first, second, "subsequent calls reuse the same cached path");
        // A temp cleaner removing the file between launches re-extracts.
        std::fs::remove_file(&first).unwrap();
        let third = bundled_default_config_path().unwrap();
        assert_eq!(first, third);
        assert!(
            third.is_file(),
            "re-extracts when a temp cleaner removed the file"
        );
        assert_eq!(std::fs::read(&third).unwrap(), DEFAULT_CORDIS_YML);
        // A locally poisoned file at the deterministic temp path (the qc3
        // W-2 threat model: world-writable temp dir on shared machines) is
        // detected by the byte verification on the next use and atomically
        // re-extracted, so the injected config is always the SDK's own
        // asset.
        std::fs::write(&first, b"attacker-controlled cordis.yml").unwrap();
        let fourth = bundled_default_config_path().unwrap();
        assert_eq!(first, fourth);
        assert_eq!(
            std::fs::read(&fourth).unwrap(),
            DEFAULT_CORDIS_YML,
            "a mismatched cached file must be re-extracted (byte-verified)"
        );
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

    // --- resolve_runtime: empty runtime_bin fallthrough -------------------

    #[test]
    fn resolve_empty_runtime_bin_counts_as_absent_and_falls_through_to_env() {
        // runtime_bin: Some("") is falsy (Python truthiness) → the parent
        // DSH_RUNTIME_BIN route applies...
        let env: HashMap<&'static str, &'static str> =
            HashMap::from([("DSH_RUNTIME_BIN", "env-bin")]);
        let config = Config {
            runtime_bin: Some("".into()),
            ..Config::default()
        };
        assert_eq!(
            resolve_runtime_with(&config, lookup(&env)).unwrap(),
            RuntimeLaunch {
                program: "env-bin".into(),
                args: vec![]
            }
        );
        // ...and with no env either, the empty bin still counts as absent →
        // RuntimeNotFound (never an unlaunchable empty program).
        let empty_env = HashMap::new();
        assert!(matches!(
            resolve_runtime_with(&config, lookup(&empty_env)),
            Err(Error::RuntimeNotFound(_))
        ));
    }
}
