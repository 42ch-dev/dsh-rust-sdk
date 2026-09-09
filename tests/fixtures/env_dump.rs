//! Fixture binary: dumps the process environment to a file, then sleeps
//! forever without reading stdin.
//!
//! The child-env contract tests (spec §4.2) need to observe what the
//! spawned child actually inherited, so this fixture writes the requested
//! environment keys to a file synchronously at startup and then sleeps —
//! the test polls for the file, reads the dump, and closes the client
//! through the normal ladder (which must escalate to SIGTERM, exactly like
//! the `sleep-forever` fixture).
//!
//! Usage: `env-dump <output-file> [key...]`
//!
//! With keys: writes one `key=value` line per requested key (absent keys
//! are omitted). Without keys: writes the whole environment. Lines are
//! sorted so the dump is deterministic. Registered as the `env-dump` bin
//! target; integration tests reach it via
//! `env!("CARGO_BIN_EXE_env-dump")`.

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(output) = args.next() else {
        eprintln!("usage: env-dump <output-file> [key...]");
        std::process::exit(2);
    };
    let keys: Vec<String> = args.collect();
    let mut lines: Vec<String> = if keys.is_empty() {
        std::env::vars()
            .map(|(key, value)| format!("{key}={value}"))
            .collect()
    } else {
        keys.iter()
            .filter_map(|key| {
                std::env::var(key)
                    .ok()
                    .map(|value| format!("{key}={value}"))
            })
            .collect()
    };
    lines.sort();
    std::fs::write(&output, lines.join("\n")).expect("write the env dump");
    loop {
        std::thread::sleep(std::time::Duration::from_secs(3600));
    }
}
