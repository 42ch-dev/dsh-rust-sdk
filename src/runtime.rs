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
//! subprocess: the resolved `DSH_HOME` (non-empty `$DSH_HOME` from
//! `Config::env`, then the parent environment, else `~/.dsh` — upstream
//! `resolveDshHome`, `packages/util/home-paths/src/index.ts:87-91`), the
//! caller's `Config::env` entries verbatim, and `DEEPSEEK_BASE_URL` /
//! `DEEPSEEK_API_KEY` when configured. The caller's `Config::env` wins over
//! the injected `DSH_HOME` on collision (Python `env.update(self.config.env)`
//! ordering, `python/sdk/src/deepseek_harness/client.py:75-77`). The crate
//! never writes `DSH_CORDIS_CONFIG`, `DSH_SESSION_ROOT`, or `DSH_CWD` —
//! none has a reader upstream (spec §4.2).
//!
//! The official runtime and its sources live at
//! <https://github.com/deepseek-ai/deepseek-harness>.

use std::collections::HashMap;
use std::ffi::OsString;
use std::fmt;
use std::path::PathBuf;
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
    /// Provider name sent to the runtime on `initialize`
    /// (Python default `"deepseek-official"`).
    pub provider: String,
    /// Model name sent to the runtime on `initialize`
    /// (Python default `"deepseek-v4-flash"`).
    pub model: String,
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
    /// Extra environment entries layered over the parent environment; the
    /// caller's entries win over the crate-injected `DSH_HOME` on collision
    /// (Python `env.update(self.config.env)` ordering).
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
            .field("dsh_bin", &self.dsh_bin)
            .field("profile", &self.profile)
            .field("patches", &self.patches)
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
            dsh_bin: None,
            profile: "sdk".into(),
            patches: Vec::new(),
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
    // (upstream `apps/cli/src/args.ts:137-140`); an empty profile is
    // rejected locally (spec §2.2.6) so the failure stays attributable.
    if config.profile.trim().is_empty() {
        return Err(Error::Config(
            "profile must not be empty: dsh --profile <name> is required".to_string(),
        ));
    }
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

/// Compose the environment override set injected into the runtime subprocess.
///
/// Returns exactly the applicable override keys, in a stable order: the
/// resolved `DSH_HOME` first, then the caller's `Config::env` entries
/// verbatim, then `DEEPSEEK_BASE_URL` / `DEEPSEEK_API_KEY` when configured.
/// When a key appears twice, the later entry wins at spawn, so the caller's
/// `Config::env` overrides the injected `DSH_HOME` on collision (Python
/// `env.update(self.config.env)` ordering —
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
    // The resolved home is injected first so the caller's Config::env wins
    // on collision (later entry wins at spawn).
    let mut envs = vec![("DSH_HOME".to_string(), resolve_dsh_home(config, &lookup))];
    if let Some(extra) = &config.env {
        envs.extend(
            extra
                .iter()
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

/// Resolve the harness home: a non-empty `$DSH_HOME` (first from
/// `Config::env`, then the parent environment), else `~/.dsh` (upstream
/// `resolveDshHome`, `packages/util/home-paths/src/index.ts:87-91`).
/// Blank/whitespace-only counts as unset, exactly as upstream does
/// (`packages/util/home-paths/src/index.ts:88-89`).
///
/// The `Config::dsh_home` first rule, absolute/`~` normalization, directory
/// creation, and the public accessor land with the resolution helper in the
/// Config surface task (spec §3.2).
fn resolve_dsh_home(config: &Config, lookup: &impl Fn(&str) -> Option<String>) -> String {
    config
        .env
        .as_ref()
        .and_then(|extra| extra.get("DSH_HOME"))
        .filter(|value| !value.trim().is_empty())
        .cloned()
        .or_else(|| lookup("DSH_HOME").filter(|value| !value.trim().is_empty()))
        .unwrap_or_else(|| {
            std::env::home_dir()
                .map(|home| home.join(".dsh"))
                .unwrap_or_else(|| PathBuf::from(".dsh"))
                .to_string_lossy()
                .into_owned()
        })
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
    fn compose_env_caller_env_wins_over_injected_dsh_home() {
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
            "the caller's Config::env must win over the injected DSH_HOME"
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
