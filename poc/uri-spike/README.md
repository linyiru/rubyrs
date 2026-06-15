# uri discovery spike (net/http URI track)

Probes loading + using the **real MRI `uri` 1.0.2 gem** on rubyrs (the
URI dependency net/http needs). Run:
`target/release/rubyrs poc/uri-spike/uri-probe.rb`.

**Outcome: RESOLVED.** The real `uri` gem now loads and parses URLs on
rubyrs — `URI("http://h:8080/p?q=1")` and `URI.parse(...)` return correct
host/port/path/query/scheme. URI did NOT need a vendored canon; three
small Tier-1 core fixes (Rule 6 canonical) cleared it:

1. `fix(const): const_defined?(name, false)` — own-only (no ancestor
   walk). Unblocked the gem's load-time
   `remove_const(sym) if const_defined?(sym, false)` loop
   (`URI::SCHEME not defined`).
2. `feat(string): String#delete!` — `uri/generic.rb`'s `query=`.
3. `fix(dispatch): Module#=== honours included modules` — net/http's
   `if URI === uri_or_path` (URI::Generic does `include URI`).

Each shipped with a diff_cruby test. The net/http spike
(`../net-http-spike/`) now drives the real `uri` directly (no URI shim).
