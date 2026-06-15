# net/http discovery spike (ADR 0028 Phase 1)

Maps the exact socket surface the real MRI `net/http.rb` drives, so the
`_socket` battery (ADR 0028) is built to a measured spec. **Findings:
[`FINDINGS.md`](FINDINGS.md).**

Run: `target/release/rubyrs poc/net-http-spike/nh-probe.rb`
(full-feature build). Gem sources read from rbenv 3.4.1's gem dir.

Result: real `net/http` 0.6.0 + `net/protocol` 0.2.2 complete a GET
request/response end to end on rubyrs (`code=200 body="hello, world!"`)
against a **recording `TCPSocket` shim** that logs every socket call.

Headline: the happy-path GET needs only **4 socket host-fns**
(`connect` / `write` / `read` / `close`); a blocking `std::net` battery
eliminates the `wait_readable` sub-surface entirely. Plus five
prerequisite gaps to land first — `String#chop`, `String#clear`,
`Errno::EALREADY` + `ECONNABORTED` (Tier 1 core), and the `class <<
<Const>; alias` parser routing — and a separate **URI** blocker
(rubyrs's URI stub is insufficient AND the real `uri` gem won't load).

Files:
- `nh-probe.rb` — driver; builds a patched net/http.rb under /tmp at
  runtime (no committed symlinks), drives 3 request shapes, dumps the
  surface.
- `shim/recording_socket.rb` — recording TCPSocket + minimal
  URI/Socket/Errno/String-gap shims; each names the wall it bridges.
