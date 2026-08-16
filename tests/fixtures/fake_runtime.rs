//! Fixture binary: a scripted stdio JSON-RPC peer standing in for the DSH
//! runtime in the integration suite.
//!
//! The scenario script — a JSON array of [`directive::Directive`] — is passed
//! as the sole argv argument. The peer then serves the client: requests
//! arrive as JSON lines on stdin, frames (responses, notifications, garbage
//! lines, blank lines) are written to stdout, one per line. Stdout is
//! flushed after every frame because it is a pipe, not a terminal.
//!
//! Registered as the `fake-runtime` bin target; integration tests reach it
//! via `env!("CARGO_BIN_EXE_fake-runtime")`.

use std::io::{BufRead, Write};
use std::time::Duration;

use serde_json::{json, Value};

#[path = "../common/directive.rs"]
mod directive;
use directive::Directive;

fn main() {
    let script_json = std::env::args()
        .nth(1)
        .expect("usage: fake-runtime <scenario-json>");
    let script: Vec<Directive> = match serde_json::from_str(&script_json) {
        Ok(script) => script,
        Err(err) => {
            eprintln!("fake-runtime: cannot parse scenario: {err}");
            std::process::exit(2);
        }
    };

    let stdin = std::io::stdin();
    let mut stdin = stdin.lock();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let mut last_id: Option<Value> = None;

    for step in script {
        match step {
            Directive::Expect {
                method,
                params_contains: expected_params,
            } => {
                let request = read_request(&mut stdin);
                let got = request.get("method").and_then(Value::as_str);
                if got != Some(method.as_str()) {
                    eprintln!("fake-runtime: expected request {method:?}, got {got:?}");
                    std::process::exit(2);
                }
                if let Some(expected) = expected_params {
                    if !params_contains(request.get("params"), &expected) {
                        eprintln!("fake-runtime: request params mismatch: {request}");
                        std::process::exit(2);
                    }
                }
                last_id = request.get("id").cloned();
            }
            Directive::Respond { result } => {
                let id = take_id(&mut last_id);
                write_line(
                    &mut out,
                    &json!({ "jsonrpc": "2.0", "id": id, "result": result }),
                );
            }
            Directive::RespondError {
                code,
                message,
                data,
            } => {
                let id = take_id(&mut last_id);
                let mut error = json!({ "code": code, "message": message });
                if let Some(data) = data {
                    error["data"] = data;
                }
                write_line(
                    &mut out,
                    &json!({ "jsonrpc": "2.0", "id": id, "error": error }),
                );
            }
            Directive::Emit { method, params } => {
                write_line(&mut out, &json!({ "method": method, "params": params }));
            }
            Directive::EmitRaw { line } => {
                writeln!(out, "{line}").expect("write raw line");
                out.flush().expect("flush raw line");
            }
            Directive::EmitBlank => {
                writeln!(out).expect("write blank line");
                out.flush().expect("flush blank line");
            }
            Directive::IgnoreAll => {
                let mut line = String::new();
                loop {
                    line.clear();
                    match stdin.read_line(&mut line) {
                        Ok(0) => break, // the client closed stdin
                        Ok(_) => {}
                        Err(err) => {
                            eprintln!("fake-runtime: stdin read failed: {err}");
                            std::process::exit(2);
                        }
                    }
                }
                std::process::exit(0);
            }
            Directive::SleepMs { ms } => std::thread::sleep(Duration::from_millis(ms)),
            Directive::Exit { code } => std::process::exit(code),
        }
    }
}

/// Read the next request line from the client; skip malformed lines
/// defensively (the client never sends any).
fn read_request(stdin: &mut impl BufRead) -> Value {
    let mut line = String::new();
    loop {
        line.clear();
        match stdin.read_line(&mut line) {
            Ok(0) => {
                eprintln!("fake-runtime: client closed stdin while a request was expected");
                std::process::exit(2);
            }
            Ok(_) => match serde_json::from_str::<Value>(&line) {
                Ok(request) => return request,
                Err(_) => continue,
            },
            Err(err) => {
                eprintln!("fake-runtime: stdin read failed: {err}");
                std::process::exit(2);
            }
        }
    }
}

/// Take the request id captured by the most recent `Expect` step.
fn take_id(last_id: &mut Option<Value>) -> Value {
    match last_id.take() {
        Some(id) => id,
        None => {
            eprintln!("fake-runtime: respond without a preceding expect");
            std::process::exit(2);
        }
    }
}

/// Write one JSON frame as a flushed line on stdout.
fn write_line(out: &mut impl Write, frame: &Value) {
    serde_json::to_writer(&mut *out, frame).expect("serialize frame");
    out.write_all(b"\n").expect("write frame newline");
    out.flush().expect("flush frame");
}

/// Whether every key of `expected` exists in `actual` with an equal value
/// (deep subset check over JSON objects).
fn params_contains(actual: Option<&Value>, expected: &Value) -> bool {
    let Value::Object(expected) = expected else {
        return false;
    };
    let Some(Value::Object(actual)) = actual else {
        return false;
    };
    expected
        .iter()
        .all(|(key, want)| actual.get(key) == Some(want))
}
