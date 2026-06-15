# net/http discovery spike — FINDINGS (ADR 0028 Phase 1)

**Date:** 2026-06-15. **Goal:** enumerate the exact surface the real MRI
`net/http.rb` drives, so the `_socket` battery (ADR 0028 §2 host-fns) and
the pure-Ruby `Net::HTTP` veneer are built to a measured spec, not a guess.

**Method:** load the real `net/http.rb` 0.6.0 + `net/protocol` 0.2.2 on
rubyrs against a **recording `TCPSocket` shim** (`shim/recording_socket.rb`)
that logs every method net/protocol's `BufferedIO` + net/http call on the
socket and feeds back a canned `HTTP/1.1 200` response. Drive a request
three ways (`Net::HTTP.start{…}.request`, `Net::HTTP.get`, faraday
`net_http` adapter) and dump the call tally.

**Result:** `Net::HTTP.start{ http.request(Get.new(uri)) }` and
`Net::HTTP.get(uri)` both **complete end to end** — `code=200
body="hello, world!"`. Reproduce: `target/release/rubyrs
poc/net-http-spike/nh-probe.rb`.

---

## 1. The socket host-fn surface (THE deliverable → ADR 0028 §2)

The complete set net/http's happy-path GET drives on the socket:

| Ruby call on the socket | Count | → `_socket` host-fn |
|---|---|---|
| `TCPSocket.open(host, port)` | 2 | `__rubyrs_socket_connect(host, port, open_timeout)` |
| `setsockopt(IPPROTO_TCP, TCP_NODELAY, 1)` | 2 | `__rubyrs_socket_setsockopt` *(or drop — see below)* |
| `write_nonblock(str, exception: false)` | 2 | `__rubyrs_socket_write(handle, bytes)` |
| `read_nonblock(BUFSIZE, buf, exception: false)` | 2 | `__rubyrs_socket_read(handle, maxlen)` |
| `closed?` | 6 | `__rubyrs_socket_closed?` (or track Ruby-side) |
| `close` | 3 | `__rubyrs_socket_close(handle)` |

**`UNHANDLED` count: zero** — the shim anticipated every method called;
the surface above is complete for the plain-HTTP GET path.

### Two surface refinements vs ADR 0028's draft host-fn list

1. **It's `read_nonblock` / `write_nonblock`, not blocking `read` / `write`.**
   net/protocol's `BufferedIO` is built on the non-blocking pair plus a
   readiness wait (`@io.to_io.wait_readable(timeout)`). The ADR §2 list
   (`_read` / `_write`) should be renamed/shaped as the `_nonblock`
   contract: `read(handle, maxlen) → bytes | nil(EOF) | :wait_readable`.

2. **A blocking `std::net` battery ELIMINATES the `wait_readable` path.**
   net/protocol only calls `to_io.wait_readable` when `read_nonblock`
   returns `:wait_readable` (data not yet ready). If the `_socket`
   battery implements the host-fn as a **blocking read with the socket's
   `read_timeout` deadline** (`TcpStream::set_read_timeout`) and returns
   data-or-`nil(EOF)`-or-raises-`Net::ReadTimeout`, it NEVER returns
   `:wait_readable` — so `to_io` / `wait_readable` / `wait_writable` are
   never exercised. **The blocking crate choice (ADR 0028 §1) removes a
   whole sub-surface** (`to_io`, `wait_readable`, `wait_writable`,
   `io/wait`). Confirmed: the spike's immediate-data socket never logged
   `to_io`/`wait_*`. This is a concrete argument FOR the blocking design.
   *(Minor net/http.rb trim: it passes `exception: false`; the veneer
   maps a deadline-elapsed to `Net::ReadTimeout` itself.)*

3. **`setsockopt(TCP_NODELAY)` is the only sockopt** net/http sets. The
   battery can either expose a one-shot `setsockopt` host-fn OR set
   `TCP_NODELAY` unconditionally inside `connect` (TCP_NODELAY is the
   only thing net/http asks for) and **drop the host-fn** — simpler.
   Recommendation: set `nodelay(true)` in `connect`, no `setsockopt`
   host-fn.

**NOT exercised in the happy path** (so NOT v1 host-fns): plain `read` /
`write` / `<<`, `flush`, `sync`, `eof?`, `peeraddr`, `to_io`,
`wait_readable`, `wait_writable`. `eof?` / `peeraddr` may surface on
error/keep-alive paths — defer until a fixture needs them.

---

## 2. VM gaps found (each blocks the path; tier + fix noted)

| Gap | Where it bit | Tier / fix |
|---|---|---|
| **`String#chop` missing** | `net/protocol.rb:209 readline` (`readuntil("\n").chop`) | **Tier 1 core** — trivial method to add (`"\r\n"`-aware). Shimmed in spike. |
| **`String#clear` missing** | `net/protocol.rb:243 rbuf_flush` (`@rbuf.clear`) | **Tier 1 core** — `replace("")` in place. Shimmed in spike. |
| **`Errno::EALREADY`, `Errno::ECONNABORTED` missing** | faraday-net_http's exception list (`net_http.rb:18`) | **Tier 1** — add the 2 missing `SystemCallError` subclasses (the other 8 in faraday's list already exist). Shimmed in spike. |
| **`class << HTTP; alias …`** rejected | `net/http.rb:758/1073/1133/1830` | **Parser** — `class << <Const>` (Const == enclosing class) with `alias`/non-def body bails in the restricted singleton-class translator. Same fix shape as the earlier if/elsif/case routing: add `AliasMethodNode` (and bare-`alias`) to `singleton_body_needs_real_eval` so it runs in the real eigenclass body. **Vendor-patched** in the spike (`class << HTTP` → `class << self`, semantically identical here). |
| **`Socket::IPPROTO_TCP` / `Socket::TCP_NODELAY`** undefined | `net/http.rb:1671 connect` | The `_socket` battery defines them (or `connect` sets nodelay itself — see §1.3). Shimmed. |

All five are **independent of the socket battery's network reach** —
`chop`/`clear`/`Errno` are core; `class << Const alias` is parser. They
should land as their own small commits (the String/Errno ones are
diff_cruby one-liners), not bundled into the battery PR.

---

## 3. URI is a SEPARATE blocker (not ADR 0028 scope)

net/http needs a real `URI`. rubyrs today:

- The **vendored URI stub is insufficient**: no `URI()` Kernel method,
  no `URI::HTTP` / `URI::Generic`, no parsing.
- The **real `uri` gem fails to load** on rubyrs: `NameError:
  uninitialized constant URI::SCHEME` (`uri/common.rb:37`, in
  `parser=`).

The spike provides a ~30-line `URI` shim. The surface net/http actually
drives (→ what a real rubyrs `URI` must cover): `URI()`, `URI.parse`,
`URI === obj`, and on the instance: `scheme/host/port/path/query` (read
AND write — `update_uri` mutates), `request_uri`, `hostname`,
`default_port`, `dup`, and the **9-arg component constructor**
`URI::HTTP.new(scheme, userinfo, host, port, reg, path, opaque, query,
frag)` + `find_proxy`.

**This is its own canon/battery decision, NOT part of the `_socket`
battery.** Net::HTTP can't ship without it; recommend a follow-up:
either get the real `uri` gem loading (fix `URI::SCHEME`) or vendor a
pure-Ruby URI canon. Flag for a separate ADR/spike.

---

## 4. `Net::HTTP` public surface driven (→ veneer freeze surface)

From the passing phases B/C: `Net::HTTP.start(host, port) { |http| … }`,
`Net::HTTP::Get.new(uri)`, `http.request(req)`, `Net::HTTP.get(uri)`,
and on the response: `#code`, `#body`. Request build path touches
`Net::HTTPGenericRequest#initialize` / `#update_uri`, response path
`Net::HTTPResponse.read_new` / `#read_body` / `#body` (with the gzip
`inflater` wrapper — `HAVE_ZLIB` true, so the body reader goes through
the zlib path; rubyrs's `stdlib` zlib covered it). This is the minimal
`Net::HTTP` surface ADR 0028 §6 should freeze for v1.

## 5. faraday over the net_http adapter — phase D

Phase D did **not** complete, but for a faraday-INTERNAL reason, not a
socket one: `Faraday.new` → `connection_options.rb:20 new_builder` →
`undefined method 'new' for nil` (`RackBuilder` resolved nil in this
minimal harness — faraday's full dep tree isn't loaded here; the
Bridgetown spike loads faraday further). The **net/http path (B/C) is
the authoritative socket-surface source**; faraday merely wraps it. A
later end-to-end faraday check belongs in the Bridgetown spike once
`_socket` + a real URI land.

---

## 6. Net for Phase 3 (the battery build)

1. `_socket` host-fns: `connect(host, port, open_timeout)` (sets
   TCP_NODELAY), `write(handle, bytes)`, `read(handle, maxlen)` (blocking
   w/ read_timeout → bytes | nil | ReadTimeout), `close(handle)`. **Four
   host-fns, no `setsockopt`, no `wait_readable`** (§1).
2. Pure-Ruby `TCPSocket` veneer exposing `read_nonblock`/`write_nonblock`
   (mapped onto the blocking host-fns; never returns `:wait_readable`).
3. Land the core fixes (`String#chop`, `String#clear`, `Errno::EALREADY`
   + `ECONNABORTED`, `class << Const alias`) as separate commits FIRST —
   they unblock loading the real `net/http.rb` + `net/protocol`.
4. Resolve URI (§3) on its own track before `Net::HTTP` can ship.
5. `https` waits on the `_openssl` battery (ADR 0028 §6, separate).

## Spike artifacts

- `nh-probe.rb` — the driver + surface dump.
- `shim/recording_socket.rb` — recording TCPSocket + minimal URI/Socket/
  Errno/String-gap shims. Every shim names the wall it bridges.
- `vendor-net/net/http.rb` — `class << HTTP`→`class << self` patched copy
  (the rest of the gem's `net/` tree symlinked alongside).
