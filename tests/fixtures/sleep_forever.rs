//! Fixture binary: sleeps forever without ever reading stdin.
//!
//! The client's close ladder must therefore escalate past the cooperative
//! `shutdown` request (never answered) and the stdin-EOF tier (never read)
//! to SIGTERM, whose default disposition terminates the process. Registered
//! as the `sleep-forever` bin target; integration tests reach it via
//! `env!("CARGO_BIN_EXE_sleep-forever")`.

fn main() {
    loop {
        std::thread::sleep(std::time::Duration::from_secs(3600));
    }
}
