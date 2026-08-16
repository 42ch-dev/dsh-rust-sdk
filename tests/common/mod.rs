// Shared integration-test support. Each test crate under `tests/` declares
// `mod common;`; nothing here is a test target itself.
//
// The harness is compiled into every test crate, but each crate uses only a
// subset of the helpers — dead-code analysis is per test binary, so the
// helpers another crate owns are legitimately unused here.
#![allow(dead_code)]

pub mod fake_runtime;
