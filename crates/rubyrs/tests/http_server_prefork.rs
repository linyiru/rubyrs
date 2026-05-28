//! NOTE: tests in this file are serialised via a
//! file-local Mutex — each spawns the rubyrs binary as a
//! subprocess + binds a TCP port, and parallel execution
//! made port races + cargo build cache contention hard to
//! reason about. The lock is cheap (held only for the
//! ~3.5s subprocess lifetime per test).
//!
//! Stage 7d: subprocess test for real fork-based pre-fork
//! workers. Spawns the rubyrs binary running a 2-worker
//! prefork server, asserts:
//!   1. Both children fork and run `on_worker_boot` (via
//!      stdout "BOOTED 0" + "BOOTED 1" markers)
//!   2. The shared port serves at least one 200 response
//!   3. The parent supervisor waitpids cleanly and exits 0
//!
//! Per-request load distribution is OS-dependent — Linux
//! SO_REUSEPORT does kernel hash-based load balancing
//! across the bound sockets; macOS's SO_REUSEPORT is
//! more permissive (multiple binds OK, but distribution
//! isn't guaranteed — connections may stick to one
//! listener). The test only asserts both children booted;
//! load distribution is observed informationally.
//!
//! This is the ONLY way to test the fork(2) path —
//! forking inside `cargo test` would kill the test
//! runner.

#![cfg(all(feature = "_http_server", target_family = "unix"))]

use std::io::{Read, Write};
use std::net::TcpStream;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::Duration;

/// File-local serialisation — see module doc. Mutex
/// poisons on test panic; we just unwrap and continue
/// since subsequent tests can still run after a prior
/// panic (a poisoned guard reflects the prior failure,
/// not any state leak).
static TEST_SERIAL: Mutex<()> = Mutex::new(());

fn http_get(addr: &str, path: &str) -> String {
    let mut client = TcpStream::connect(addr).expect("connect to prefork server");
    client.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let req = format!(
        "GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
    );
    client.write_all(req.as_bytes()).expect("write request");
    let mut buf = Vec::new();
    let _ = client.read_to_end(&mut buf);
    String::from_utf8_lossy(&buf).into_owned()
}

#[test]
fn prefork_two_workers_boot_and_serve_via_subprocess() {
    let _guard = TEST_SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let tmp = std::env::temp_dir().join("rubyrs_prefork_smoke.rb");
    // Each child prints "BOOTED <idx>" to stdout from
    // on_worker_boot — inherited FD 1 lets the test
    // observe both children fired their hook. The app
    // returns 200 + a sentinel body for response sanity.
    // 3-second duration is enough to fire a few requests
    // sequentially and reach parent waitpid completion.
    let script = r#"
on_boot = ->(idx) { puts "BOOTED #{idx}" }
app = ->(env) { [200, {"Content-Type" => "text/plain"}, ["ok"]] }
__rubyrs_http_serve_prefork("127.0.0.1:18160", 3, app, 2, on_boot)
"#;
    std::fs::write(&tmp, script).expect("write tmp driver");

    let rubyrs_bin = env!("CARGO_BIN_EXE_rubyrs");
    let child = Command::new(rubyrs_bin)
        .arg(&tmp)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn rubyrs binary");

    // Give workers time to fork + bind. On a cold cargo
    // run the binary needs ~400ms to reach the host fn;
    // 800ms is comfortably above that.
    std::thread::sleep(Duration::from_millis(800));

    // Fire a few sequential GETs to prove the port is
    // serving. Failures here mean fork/bind didn't work.
    let mut got_200 = 0;
    for _ in 0..4 {
        let resp = http_get("127.0.0.1:18160", "/p");
        if resp.contains("HTTP/1.1 200") {
            got_200 += 1;
        }
    }

    // Wait for the binary to exit. The 3-second auto-
    // shutdown fires in each child; parent waitpid all
    // children then returns. Test must wait through this
    // before reading captured stdout — otherwise pipes
    // may not be flushed.
    let output = child.wait_with_output().expect("wait_with_output");
    let _ = std::fs::remove_file(&tmp);

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();

    assert!(
        output.status.success(),
        "rubyrs prefork binary exited non-zero: {:?}\nstdout:\n{stdout}\nstderr:\n{stderr}",
        output.status,
    );
    assert!(
        got_200 >= 1,
        "expected at least one 200 OK from the prefork server; \
         fork/bind path may be broken.\nstdout:\n{stdout}\nstderr:\n{stderr}",
    );
    assert!(
        stdout.contains("BOOTED 0"),
        "worker 0 must run on_worker_boot — missing BOOTED 0 marker.\nstdout:\n{stdout}",
    );
    assert!(
        stdout.contains("BOOTED 1"),
        "worker 1 must run on_worker_boot — missing BOOTED 1 marker.\nstdout:\n{stdout}",
    );
}

/// FU1: parent forwards SIGTERM to all children, children
/// shut down gracefully via `install_signal_handler=true`
/// in their accept loop's `select!`. Test sends SIGTERM
/// to the parent pid; expects total elapsed << duration
/// (proves the signal cut short serving, not the timer).
#[test]
fn prefork_signal_forwarding_cuts_serving_short() {
    let _guard = TEST_SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let tmp = std::env::temp_dir().join("rubyrs_prefork_sigterm.rb");
    // Long duration (30s) so a passing test must EXIT
    // because of the signal, not the timer.
    let script = r#"
on_boot = ->(idx) { puts "BOOTED #{idx}" }
app = ->(env) { [200, {"Content-Type" => "text/plain"}, ["ok"]] }
__rubyrs_http_serve_prefork("127.0.0.1:18161", 30, app, 2, on_boot)
"#;
    std::fs::write(&tmp, script).expect("write tmp driver");

    let rubyrs_bin = env!("CARGO_BIN_EXE_rubyrs");
    let child = Command::new(rubyrs_bin)
        .arg(&tmp)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn rubyrs binary");
    let parent_pid = child.id() as i32;

    // Let the parent fork + both children bind. 800ms
    // matches the boot-and-serve test's slack.
    std::thread::sleep(Duration::from_millis(800));

    // Verify it's actually serving before we signal it —
    // otherwise we don't know if SIGTERM hit a live
    // process group or one still in setup.
    let resp = http_get("127.0.0.1:18161", "/p");
    assert!(
        resp.contains("HTTP/1.1 200"),
        "expected 200 before SIGTERM, got:\n{resp}",
    );

    // Send SIGTERM to parent's pid ONLY (NOT the pgroup).
    // This is the case our process-group-default fallback
    // doesn't cover — children would survive until the
    // 30-second duration if FU1 forwarding wasn't wired.
    let started = std::time::Instant::now();
    unsafe { libc::kill(parent_pid, libc::SIGTERM); }

    let output = child.wait_with_output().expect("wait_with_output");
    let elapsed = started.elapsed();
    let _ = std::fs::remove_file(&tmp);

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();

    // The parent's waitpid blocks until children exit;
    // children's serve loop must have seen SIGTERM via
    // tokio::signal and returned. Bound elapsed at 5s —
    // generous slack over typical "graceful shutdown
    // within a few hundred ms" but well below the 30s
    // duration timer.
    assert!(
        elapsed < Duration::from_secs(5),
        "expected graceful exit < 5s after SIGTERM, took {elapsed:?} \
         — children didn't receive the forwarded signal.\n\
         stdout:\n{stdout}\nstderr:\n{stderr}",
    );

    // The PROCESS exit code for SIGTERM-via-waitpid is
    // implementation-defined. We don't assert success/
    // failure — just that it exited within the bound and
    // produced the boot markers (i.e., the children
    // actually ran).
    assert!(
        stdout.contains("BOOTED 0") && stdout.contains("BOOTED 1"),
        "expected both workers to have booted before signal.\n\
         stdout:\n{stdout}\nstderr:\n{stderr}",
    );
}
