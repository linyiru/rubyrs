//! M27 D — Parity Harness v2: framework-fixture diff between rubyrs
//! and CRuby per ADR 0026 v2 §"Compatibility contract".
//!
//! ## Why this is a new harness, not a generalisation of `diff_cruby.rs`
//!
//! `diff_cruby.rs` runs script-file fixtures via `--disable=gems` and
//! diffs stdout. It cannot:
//!   - boot a server + replay an HTTP route matrix
//!   - require a real gem on the CRuby side (its `--disable=gems`
//!     flag is the opposite of what menu-item parity needs)
//!   - run a stateful lifecycle (schema seed + post-scenario dump
//!     diff) that menu item 4 (SQLite) will need
//!
//! `diff_framework` adds those three things. Each fixture lives at
//! `tests/diff_framework/fixtures/<name>/` with:
//!
//!   - `manifest.json` — declarative scenario matrix (routes,
//!     methods, headers, bodies, normalisation regexes)
//!   - `app.rb` — the application (byte-identical between runtimes)
//!   - `compat.rb` — the ONE engine-aware shim (per ADR 0026 v2
//!     §Anti-pattern — engine-branching in user-facing adapters is
//!     fine; it's only forbidden inside blessed in-tree reimpls)
//!   - optionally any fixture-specific Ruby files (e.g. a vendored
//!     micro-Sinatra)
//!
//! ## Tiered parity (ADR 0026 v2 §Negative consequences)
//!
//! The harness's CI footprint scales with menu items; mitigation is
//! "smoke matrix per PR, full matrix nightly." This module's tests
//! run the smoke tier — one or two fixtures, one Ruby version, one
//! OS. The nightly full-matrix lane (multiple Ruby versions × OS ×
//! upstream gem versions) is M28 work; the harness API is shaped to
//! absorb that scale without re-design.
//!
//! ## Skip-not-fail when CRuby (or required gems) missing
//!
//! Mirrors `diff_cruby.rs`'s `ruby_available()` pattern: a developer
//! machine without `ruby` on PATH, or a fixture that needs an
//! un-installed gem, prints a one-line skip notice rather than
//! failing the suite. CI is configured to provide both.

#![cfg(feature = "_http_server")]

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde::Deserialize;

// File-local serialisation — every test spawns its own server +
// binds a TCP port; running in parallel triggers cross-test port
// contention even with kernel-assigned ports (the kernel can reuse
// a TIME_WAIT slot two tests later). Mirrors the same Mutex pattern
// `tests/http_server_prefork.rs` uses. Poisoned-on-panic is fine —
// we recover via `into_inner` so a prior failure doesn't block the
// rest of the matrix.
static TEST_SERIAL: Mutex<()> = Mutex::new(());

#[derive(Deserialize, Debug)]
struct Manifest {
    name: String,
    // Exactly one of `server` / `script` must be set. `server` boots
    // a long-running HTTP server and replays a route matrix (the
    // Sinatra-shape fixtures). `script` runs a one-shot script and
    // byte-diffs stdout (the JSON-shape fixtures — gem-dependent but
    // no server). Validated in run_fixture.
    #[serde(default)]
    server: Option<ServerSpec>,
    #[serde(default)]
    script: Option<ScriptSpec>,
    #[serde(default)]
    cruby: CrubySpec,
    #[serde(default)]
    rubyrs: RubyrsSpec,
    #[serde(default)]
    scenarios: Vec<Scenario>,
    #[serde(default)]
    normalize: Vec<NormalizeRule>,
}

#[derive(Deserialize, Debug)]
struct ServerSpec {
    script: String,
    ready_probe_path: String,
    boot_timeout_ms: u64,
    duration_secs: u64,
}

#[derive(Deserialize, Debug)]
struct ScriptSpec {
    /// Script to run on both runtimes. Stdout byte-diffed; non-zero
    /// exit on either side fails the test.
    path: String,
    /// Wall-clock cap. Past this, the child is killed and the test
    /// fails with a timeout message. Mirrors the server-mode
    /// `duration_secs` safety net.
    timeout_secs: u64,
}

#[derive(Deserialize, Debug, Default)]
struct CrubySpec {
    #[serde(default)]
    required_gems: Vec<String>,
}

#[derive(Deserialize, Debug, Default)]
struct RubyrsSpec {
    /// Cext examples that must have been built (via their build.sh)
    /// for this fixture to run on the rubyrs side. Each entry is an
    /// `examples/<name>/` directory holding `<artifact>.bundle` (mac)
    /// or `<artifact>.so` (linux) files; the harness verifies the
    /// artifacts exist and exports `RUBYRS_<NAME>_DIR` env vars
    /// pointing at each example dir so the fixture's compat shim
    /// can `require` them by absolute path. Skip-not-fail if any
    /// artifact is missing.
    #[serde(default)]
    required_cext_examples: Vec<CextExampleSpec>,
}

#[derive(Deserialize, Debug)]
struct CextExampleSpec {
    /// `examples/<name>/` directory under crates/rubyrs.
    name: String,
    /// Artifact stems (no extension); the harness checks for
    /// `<name>.{bundle|so}` per host OS.
    artifacts: Vec<String>,
}

#[derive(Deserialize, Debug)]
struct Scenario {
    name: String,
    method: String,
    path: String,
    #[serde(default)]
    headers: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    body: Option<String>,
}

#[derive(Deserialize, Debug)]
struct NormalizeRule {
    pattern: String,
    replacement: String,
}

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn fixtures_dir() -> PathBuf {
    manifest_dir().join("tests/diff_framework/fixtures")
}

fn rubyrs_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_rubyrs"))
}

fn ruby_available() -> bool {
    Command::new("ruby")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn ruby_gem_available(gem: &str) -> bool {
    // `ruby -e "require 'X'"` is a sturdier check than `gem list`:
    // some Ruby installs vendor gems without registering them with
    // `gem`, and conversely `gem list` doesn't know about stdlib-
    // promoted libraries like `webrick`.
    //
    // `Kernel#exit!` (not `exit`) is the no-side-effect exit — it
    // skips at_exit handlers. classic-style Sinatra registers an
    // at_exit hook that boots Puma on port 4567; without exit! the
    // gem-availability probe would clobber the harness's own
    // free-port pick. Any gem that uses at_exit for server
    // autostart (or similar side effects) would have the same
    // failure mode; exit! defuses the class.
    let script = format!(
        "begin; require '{gem}'; rescue LoadError; exit!(1); end; exit!(0)"
    );
    Command::new("ruby")
        .args(["-e", &script])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn pick_free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")
        .expect("bind 127.0.0.1:0 for free-port probe");
    let port = listener.local_addr().expect("local_addr").port();
    drop(listener);
    port
}

/// Spawn `cmd` configured to run `<fixture>/app.rb`. The server's
/// bind port is passed via `HARNESS_PORT`; its in-process duration
/// via `HARNESS_SECS`. The framework polls `ready_probe_path` until
/// 200 OK or timeout — gives the runtime a known boot window.
///
/// Returns the child plus a background drainer thread for its
/// stderr (collected into a shared buffer). The drainer is
/// essential: with `Stdio::piped()` the OS pipe buffer is finite
/// (~16–64 KB on macOS/Linux). If the harness only reads stderr
/// after wait_for_ready fails — but the child happens to come up
/// fine and then logs heavily during run_matrix (e.g. Puma access
/// logs, ActiveRecord SQL, Sinatra startup banner) — the child
/// blocks on stderr write once the buffer fills, freezing the
/// whole server. Past M27 D debug session lost ~30 min to this.
fn spawn_server(
    cmd: &mut Command,
    fixture: &Path,
    port: u16,
    spec: &ServerSpec,
) -> (std::process::Child, StderrDrain) {
    let mut child = cmd
        .arg(fixture.join(&spec.script))
        .current_dir(fixture)
        .env("HARNESS_PORT", port.to_string())
        .env("HARNESS_SECS", spec.duration_secs.to_string())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn server process");
    let stderr_pipe = child.stderr.take().expect("child stderr piped");
    let buf = Arc::new(Mutex::new(Vec::<u8>::new()));
    let buf_thread = buf.clone();
    let handle = std::thread::spawn(move || {
        let mut reader = stderr_pipe;
        let mut chunk = [0u8; 4096];
        loop {
            match reader.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => buf_thread.lock().unwrap_or_else(|p| p.into_inner()).extend_from_slice(&chunk[..n]),
                Err(_) => break,
            }
        }
    });
    (child, StderrDrain { buf, handle: Some(handle) })
}

/// Background-drained stderr buffer. `take()` joins the drainer
/// thread (closes its read end when the child's stderr pipe shuts)
/// and returns the accumulated text.
struct StderrDrain {
    buf: Arc<Mutex<Vec<u8>>>,
    handle: Option<JoinHandle<()>>,
}

impl StderrDrain {
    fn take(mut self) -> String {
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
        let bytes = self.buf.lock().unwrap_or_else(|p| p.into_inner()).clone();
        String::from_utf8_lossy(&bytes).into_owned()
    }
}

fn wait_for_ready(addr: &str, ready_path: &str, timeout_ms: u64) -> bool {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    while Instant::now() < deadline {
        if let Ok(mut s) = TcpStream::connect(addr) {
            let _ = s.set_read_timeout(Some(Duration::from_millis(500)));
            let req = format!(
                "GET {ready_path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
            );
            if s.write_all(req.as_bytes()).is_ok() {
                let mut buf = Vec::new();
                if s.read_to_end(&mut buf).is_ok()
                    && String::from_utf8_lossy(&buf).contains("HTTP/1.1 200")
                {
                    return true;
                }
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

/// Run all scenarios against a live server at `addr` and concatenate
/// the responses into a transcript suitable for byte-diff. Format
/// per scenario:
///
/// ```text
/// ### <name> <method> <path> -> <status>
/// <header lines (filtered to {Content-Type, Location})>
/// --body--
/// <body bytes>
/// --end--
/// ```
fn run_matrix(addr: &str, scenarios: &[Scenario]) -> String {
    let mut out = String::new();
    for s in scenarios {
        let mut req = format!(
            "{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n",
            method = s.method,
            path = s.path,
        );
        for (k, v) in &s.headers {
            req.push_str(&format!("{k}: {v}\r\n"));
        }
        if let Some(body) = &s.body {
            req.push_str(&format!("Content-Length: {}\r\n", body.len()));
            req.push_str("\r\n");
            req.push_str(body);
        } else {
            req.push_str("\r\n");
        }

        let mut client = TcpStream::connect(addr)
            .unwrap_or_else(|e| panic!("connect for scenario `{}`: {e}", s.name));
        client.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        client.write_all(req.as_bytes()).expect("write request");
        let mut buf = Vec::new();
        let _ = client.read_to_end(&mut buf);
        let resp = String::from_utf8_lossy(&buf);

        // Split status / headers / body. Tolerant: a runtime that
        // sends "HTTP/1.0" or a non-CRLF separator still parses by
        // first space + first blank line.
        let (head, body_raw) = match resp.find("\r\n\r\n") {
            Some(idx) => (&resp[..idx], &resp[idx + 4..]),
            None => (resp.as_ref(), ""),
        };
        let mut lines = head.lines();
        let status_line = lines.next().unwrap_or("");
        let status = status_line.split_whitespace().nth(1).unwrap_or("?");
        out.push_str(&format!("### {} {} {} -> {}\n", s.name, s.method, s.path, status));
        let mut chunked = false;
        // Collect into BTreeMap so the emit order is canonical (sorted
        // by header name) — Sinatra's WEBrick handler sends Location
        // before Content-Type while rubyrs's _http_server emits the
        // reverse. Sorting eliminates that per-runtime variation.
        let mut kept: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
        for line in lines {
            let (name, val) = match line.find(':') {
                Some(idx) => (&line[..idx], line[idx + 1..].trim()),
                None => continue,
            };
            let nlc = name.to_ascii_lowercase();
            if nlc == "transfer-encoding" && val.eq_ignore_ascii_case("chunked") {
                chunked = true;
            }
            // Filter to the small header set that matters for
            // route parity; ignore Date / Server / per-runtime
            // chrome that would force per-runtime normalisation
            // for every fixture. Canonicalise the name to
            // title-case so rubyrs `content-type` and
            // WEBrick's `Content-Type` produce identical
            // transcript bytes.
            if matches!(nlc.as_str(), "content-type" | "location") {
                kept.insert(title_case_header(name), val.to_string());
            }
        }
        for (k, v) in &kept {
            out.push_str(k);
            out.push_str(": ");
            out.push_str(v);
            out.push('\n');
        }
        let body = if chunked { decode_chunked(body_raw) } else { body_raw.to_string() };
        out.push_str("--body--\n");
        out.push_str(body.trim_end_matches('\0'));
        if !body.ends_with('\n') {
            out.push('\n');
        }
        out.push_str("--end--\n");
    }
    out
}

/// Convert "content-type" → "Content-Type" so the same header from
/// different runtimes byte-diffs cleanly. Hyphen-segment title-case
/// per RFC 7230 §3.2 convention.
fn title_case_header(name: &str) -> String {
    name.split('-').map(|seg| {
        let mut chars = seg.chars();
        match chars.next() {
            Some(c) => c.to_ascii_uppercase().to_string() + chars.as_str().to_ascii_lowercase().as_str(),
            None => String::new(),
        }
    }).collect::<Vec<_>>().join("-")
}

/// Decode HTTP/1.1 chunked transfer-encoding into the raw payload.
/// `<hex-size>\r\n<bytes>\r\n` repeated until a zero-size chunk +
/// trailing CRLF. rubyrs's `_http_server` battery sends chunked when
/// Content-Length isn't pre-computed; WEBrick sends Content-Length
/// for small responses. Without decoding, the diff would compare
/// `1A\r\nhello\n\r\n0\r\n\r\n` against `hello\n` and always fail.
fn decode_chunked(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut i = 0;
    let mut out = Vec::new();
    while i < bytes.len() {
        // Find the next CRLF — chunk-size line terminator.
        let line_end = match bytes[i..].windows(2).position(|w| w == b"\r\n") {
            Some(p) => i + p,
            None => break,
        };
        let size_str = std::str::from_utf8(&bytes[i..line_end]).unwrap_or("0");
        let size = usize::from_str_radix(size_str.trim(), 16).unwrap_or(0);
        if size == 0 {
            break;
        }
        let data_start = line_end + 2;
        let data_end = (data_start + size).min(bytes.len());
        out.extend_from_slice(&bytes[data_start..data_end]);
        // Skip the trailing CRLF after the chunk payload.
        i = data_end + 2;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Apply the manifest's normalisation regexes to a transcript. Each
/// rule is `pattern -> replacement`; runs sequentially. Used to
/// neutralise self-reported runtime tokens (`runtime=rubyrs` vs
/// `runtime=cruby`) so the post-normalise transcripts can byte-diff.
fn normalize(transcript: &str, rules: &[NormalizeRule]) -> String {
    let mut out = transcript.to_string();
    for r in rules {
        let re = regex::Regex::new(&r.pattern)
            .unwrap_or_else(|e| panic!("invalid normalize regex `{}`: {e}", r.pattern));
        out = re.replace_all(&out, r.replacement.as_str()).into_owned();
    }
    out
}

/// Run a script-mode fixture: invoke `cmd` on `fixture/<script.path>`,
/// capture stdout, return it for byte-diff. Stderr is drained in the
/// background (same rationale as `spawn_server` — finite OS pipe
/// buffer would otherwise block the child on heavy logging). A
/// timeout fires after `script.timeout_secs` to keep a hung fixture
/// from stalling the suite indefinitely.
fn probe_script(
    label: &str,
    mut cmd: Command,
    fixture: &Path,
    spec: &ScriptSpec,
) -> Result<String, String> {
    let script_path = fixture.join(&spec.path);
    let mut child = cmd
        .arg(&script_path)
        .current_dir(fixture)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("{label} spawn: {e}"))?;
    let stderr_pipe = child.stderr.take().expect("child stderr piped");
    let err_buf = Arc::new(Mutex::new(Vec::<u8>::new()));
    let err_clone = err_buf.clone();
    let err_handle = std::thread::spawn(move || {
        let mut reader = stderr_pipe;
        let mut chunk = [0u8; 4096];
        loop {
            match reader.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => err_clone.lock().unwrap_or_else(|p| p.into_inner()).extend_from_slice(&chunk[..n]),
                Err(_) => break,
            }
        }
    });
    let stdout_pipe = child.stdout.take().expect("child stdout piped");
    let out_buf = Arc::new(Mutex::new(Vec::<u8>::new()));
    let out_clone = out_buf.clone();
    let out_handle = std::thread::spawn(move || {
        let mut reader = stdout_pipe;
        let mut chunk = [0u8; 4096];
        loop {
            match reader.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => out_clone.lock().unwrap_or_else(|p| p.into_inner()).extend_from_slice(&chunk[..n]),
                Err(_) => break,
            }
        }
    });

    let deadline = Instant::now() + Duration::from_secs(spec.timeout_secs);
    let exit = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => return Err(format!("{label} wait: {e}")),
        }
    };
    let _ = out_handle.join();
    let _ = err_handle.join();
    let stdout = String::from_utf8_lossy(&out_buf.lock().unwrap_or_else(|p| p.into_inner())).into_owned();
    let stderr = String::from_utf8_lossy(&err_buf.lock().unwrap_or_else(|p| p.into_inner())).into_owned();

    match exit {
        None => Err(format!(
            "{label} script timed out after {}s\nstdout:\n{stdout}\nstderr:\n{stderr}",
            spec.timeout_secs
        )),
        Some(s) if !s.success() => Err(format!(
            "{label} script exited non-zero ({:?})\nstdout:\n{stdout}\nstderr:\n{stderr}",
            s.code()
        )),
        Some(_) => Ok(stdout),
    }
}

fn probe(
    label: &str,
    mut cmd: Command,
    fixture: &Path,
    spec: &ServerSpec,
    scenarios: &[Scenario],
) -> Result<String, String> {
    let port = pick_free_port();
    let addr = format!("127.0.0.1:{port}");
    let (mut child, stderr_drain) = spawn_server(&mut cmd, fixture, port, spec);
    let ready = wait_for_ready(&addr, &spec.ready_probe_path, spec.boot_timeout_ms);
    if !ready {
        let _ = child.kill();
        let _ = child.wait();
        // Pull stderr for diagnostics — boot failures are almost
        // always a SyntaxError / LoadError visible there.
        let stderr = stderr_drain.take();
        return Err(format!("{label} server failed to become ready within {}ms\nstderr:\n{stderr}", spec.boot_timeout_ms));
    }
    let transcript = run_matrix(&addr, scenarios);
    let _ = child.kill();
    let _ = child.wait();
    // Drainer thread finishes once the child's stderr pipe closes
    // (which kill+wait above guarantees). Joining it here keeps the
    // thread count clean and avoids spurious orphans showing up in
    // test output across the matrix.
    drop(stderr_drain.take());
    Ok(transcript)
}

fn cext_artifact_ext() -> &'static str {
    if cfg!(target_os = "macos") { "bundle" } else { "so" }
}

/// Returns Some(per-example-dir map) if all required cext artifacts
/// are present; None otherwise (caller skips). The map's keys are
/// the env-var names the harness will set on the rubyrs subprocess
/// (`RUBYRS_<UPPER_NAME>_DIR`); values are absolute paths.
fn collect_cext_examples(spec: &RubyrsSpec) -> Option<std::collections::BTreeMap<String, PathBuf>> {
    let examples_dir = manifest_dir().join("examples");
    let mut env_pairs = std::collections::BTreeMap::new();
    let ext = cext_artifact_ext();
    for ex in &spec.required_cext_examples {
        let dir = examples_dir.join(&ex.name);
        for artifact in &ex.artifacts {
            let path = dir.join(format!("{artifact}.{ext}"));
            if !path.exists() {
                eprintln!(
                    "missing cext artifact: {} (build with `bash crates/rubyrs/examples/{}/build.sh`)",
                    path.display(),
                    ex.name,
                );
                return None;
            }
        }
        // RUBYRS_FLORI_JSON_CEXT_DIR-style name. Hyphens → underscores,
        // then uppercased.
        let env_name = format!("RUBYRS_{}_DIR", ex.name.replace('-', "_").to_uppercase());
        env_pairs.insert(env_name, dir);
    }
    Some(env_pairs)
}

fn run_fixture(fixture_name: &str) {
    let _guard = TEST_SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let fixture = fixtures_dir().join(fixture_name);
    assert!(fixture.exists(), "missing fixture: {}", fixture.display());

    let manifest_path = fixture.join("manifest.json");
    let manifest_src = std::fs::read_to_string(&manifest_path)
        .unwrap_or_else(|e| panic!("read {}: {e}", manifest_path.display()));
    let manifest: Manifest = serde_json::from_str(&manifest_src)
        .unwrap_or_else(|e| panic!("parse manifest.json for `{fixture_name}`: {e}"));

    let mode_check = (manifest.server.is_some(), manifest.script.is_some());
    assert!(
        matches!(mode_check, (true, false) | (false, true)),
        "manifest must declare exactly one of `server` / `script` (got server={}, script={})",
        mode_check.0, mode_check.1,
    );

    // Verify required rubyrs cext artifacts exist; skip-not-fail
    // when missing. CI is expected to build them before invoking
    // the framework-parity job (per-example `build.sh`).
    let cext_env = match collect_cext_examples(&manifest.rubyrs) {
        Some(env) => env,
        None => {
            eprintln!("skipping diff_framework::{fixture_name} — required rubyrs cext artifact missing");
            return;
        }
    };

    // rubyrs side — always probed (the binary is our own).
    let rubyrs_cmd_factory = || {
        let mut c = Command::new(rubyrs_bin());
        c.env("HARNESS_RUNTIME_HINT", "rubyrs");
        for (k, v) in &cext_env {
            c.env(k, v);
        }
        c
    };
    let rubyrs_transcript = if let Some(srv) = &manifest.server {
        probe("rubyrs", rubyrs_cmd_factory(), &fixture, srv, &manifest.scenarios)
            .unwrap_or_else(|e| panic!("rubyrs probe for `{fixture_name}`: {e}"))
    } else {
        let sc = manifest.script.as_ref().unwrap();
        probe_script("rubyrs", rubyrs_cmd_factory(), &fixture, sc)
            .unwrap_or_else(|e| panic!("rubyrs script for `{fixture_name}`: {e}"))
    };

    // CRuby side — skip-not-fail when ruby missing or a required
    // gem isn't `require`-able. Mirrors `diff_cruby.rs`. CI is
    // expected to provide both via `actions/setup-ruby` + a
    // dedicated `gem install` step.
    if !ruby_available() {
        eprintln!("skipping diff_framework::{fixture_name} — `ruby` not on PATH");
        return;
    }
    for gem in &manifest.cruby.required_gems {
        if !ruby_gem_available(gem) {
            eprintln!(
                "skipping diff_framework::{fixture_name} — required gem `{gem}` not installed (try `gem install {gem}`)"
            );
            return;
        }
    }
    let cruby_transcript = if let Some(srv) = &manifest.server {
        probe("cruby", Command::new("ruby"), &fixture, srv, &manifest.scenarios)
            .unwrap_or_else(|e| panic!("cruby probe for `{fixture_name}`: {e}"))
    } else {
        let sc = manifest.script.as_ref().unwrap();
        probe_script("cruby", Command::new("ruby"), &fixture, sc)
            .unwrap_or_else(|e| panic!("cruby script for `{fixture_name}`: {e}"))
    };

    let rn = normalize(&rubyrs_transcript, &manifest.normalize);
    let cn = normalize(&cruby_transcript, &manifest.normalize);
    assert_eq!(
        rn, cn,
        "transcripts differ for `{}`:\n--- rubyrs (normalised):\n{}\n--- cruby (normalised):\n{}",
        manifest.name, rn, cn,
    );
}

#[test]
fn hello_smoke() {
    run_fixture("hello_smoke");
}

#[test]
fn sinatra_hello() {
    run_fixture("sinatra_hello");
}

#[test]
fn sinatra_plugin_smoke() {
    run_fixture("sinatra_plugin_smoke");
}

#[test]
fn json_smoke() {
    run_fixture("json_smoke");
}

#[test]
fn json_canon() {
    run_fixture("json_canon");
}

#[test]
fn as_lite_canon() {
    run_fixture("as_lite_canon");
}

#[cfg(feature = "_sqlite")]
#[test]
fn sqlite_smoke() {
    run_fixture("sqlite_smoke");
}
