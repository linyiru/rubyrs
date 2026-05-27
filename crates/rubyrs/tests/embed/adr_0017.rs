//! ADR 0017 host-capability defaults — embed users get sandbox-
//! friendly emptiness until they explicitly inject capability via
//! `Config::env` / `Config::pid` / `Runtime::set_stdout`. The CLI
//! binary `rubyrs` overrides all three; library users do not
//! inherit those overrides.
//!
//! Two complementary tests for the default-stdout policy. Together
//! they cover the spec contract from two sides; neither alone
//! catches every regression, and the gap is documented.
//!
//!   - `adr_0017_default_stdout_does_not_panic` — `Runtime::new`
//!     without a `set_stdout` call accepts `puts` and `Runtime::eval`
//!     returns `Ok`. The test is intentionally weak: rubyrs's
//!     `puts` impl in `vm/kernel.rs` silently swallows
//!     `write!`/`writeln!` errors via `let _ = …`, so even if the
//!     default sink were swapped back to `std::io::stdout()` on a
//!     closed-stdio fixture this test would still pass. The strict
//!     "default truly drops bytes" assertion requires intercepting
//!     fd 1 (a new dev-dep like `gag` or fork-and-pipe scaffolding)
//!     and is left as a known gap; the source-level guarantee is
//!     `vm.rs::Vm::new`'s `stdout: Box::new(std::io::sink())`.
//!
//!   - `adr_0017_set_stdout_routes_writes_to_host_sink` — after
//!     `set_stdout(buf)`, `puts X` lands in `buf`. Catches the
//!     regression class the first test cannot — `set_stdout`
//!     silently no-oping or being mis-wired — and proves the
//!     host-controlled-sink path the spec depends on is actually
//!     wired end-to-end.

use super::SharedBuf;

#[test]
fn adr_0017_default_stdout_does_not_panic() {
    let mut rt = rubyrs::Runtime::new();
    rt.eval(r#"puts "should be silent""#, "embed-test").expect("puts should not error");
}

#[test]
fn adr_0017_set_stdout_routes_writes_to_host_sink() {
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(r#"puts "captured""#, "embed-test").expect("puts should not error");
    assert_eq!(buf.snapshot(), "captured\n",
        "set_stdout(buf) must route puts into the host-provided sink");
}

#[test]
fn adr_0017_default_env_is_empty() {
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(r#"puts ENV.size"#, "embed-test").expect("ENV access should not error");
    assert_eq!(buf.snapshot().trim(), "0",
        "Config::env default is None → script-visible ENV is empty");
}

#[test]
fn adr_0017_default_pid_is_zero_sentinel() {
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(r#"puts $$"#, "embed-test").expect("$$ access should not error");
    assert_eq!(buf.snapshot().trim(), "0",
        "Config::pid default is None → $$ returns 0 sentinel, never the real host PID");
}

#[test]
fn adr_0017_config_env_overrides_inject_into_script() {
    let buf = SharedBuf::new();
    let mut env = std::collections::HashMap::new();
    env.insert("INJECTED".to_string(), "from_host".to_string());
    let cfg = rubyrs::Config { env: Some(env), ..Default::default() };
    let mut rt = rubyrs::Runtime::with_config(cfg);
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(r#"puts ENV["INJECTED"]"#, "embed-test").expect("ENV read should succeed");
    assert_eq!(buf.snapshot().trim(), "from_host",
        "Config::env Some(map) should expose exactly that map and nothing else");
}

#[test]
fn adr_0017_config_pid_overrides_dollar_dollar() {
    let buf = SharedBuf::new();
    let cfg = rubyrs::Config { pid: std::num::NonZeroU32::new(42), ..Default::default() };
    let mut rt = rubyrs::Runtime::with_config(cfg);
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(r#"puts $$"#, "embed-test").expect("$$ read should succeed");
    assert_eq!(buf.snapshot().trim(), "42",
        "Config::pid Some(n) should expose that n through $$ verbatim");
}
