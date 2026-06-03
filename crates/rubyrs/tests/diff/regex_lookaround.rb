## Lookaround patterns compile via the fancy-regex fallback.
## rubyrs uses `regex` (linear-time, ReDoS-immune) as the
## primary backend; when the linear engine rejects a pattern
## as unsupported syntax — typically `(?=...)` / `(?!...)` /
## `(?<=...)` lookaround — we fall back to fancy-regex for
## that single pattern.
##
## Discovery: TRY_RUNS pass-13 — sinatra-4's `cleaned_caller`
## (sinatra/base.rb:1913) splits on `/:(?=\d|in )/`, which
## previously raised "regex parse error: look-around ... not
## supported" at compile time. (Layer #17.)
##
## This PR lands the compile fallback PLUS dual-engine impls
## for the simple-shape ops (is_match, replace, replace_all —
## used by String#match?/Regexp#match? and string-form
## sub/gsub). Capture-bearing ops (captures, captures_iter,
## find_iter, captures_len — used by =~, Regexp#===,
## String#match, String#scan, String#[]/slice, block-form
## sub/gsub) are NOT dual-engine yet because the two engines
## return distinct `Captures` types with different lifetimes;
## those call sites raise RuntimeError on fancy-engine
## patterns (rubyrs doesn't model NotImplementedError as a
## RubyError variant yet — RuntimeError with a "not yet
## supported" message is the closest fit). Follow-up PRs
## migrate each capture-bearing op to a normalized
## owned-captures shape as gem layers demand it.

## Shape 1: lookahead `(?=...)` — the motivating sinatra
## pattern. Must compile and round-trip through `#source`.
## (Skipping `.class` because the `Regexp` constant isn't
## exposed to user-Ruby in rubyrs; an orthogonal gap.)
re1 = /:(?=\d|in )/
puts "shape1-source=#{re1.source}"

## Shape 2: negative lookahead `(?!...)`.
re2 = /foo(?!bar)/
puts "shape2-source=#{re2.source}"

## Shape 3: lookbehind `(?<=...)`.
re3 = /(?<=\$)\d+/
puts "shape3-source=#{re3.source}"

## Shape 4: simple patterns still take the native path —
## no behavioural change, just a parity check.
re4 = /hello/
puts "shape4-match=#{re4 =~ 'hello world'}"
puts "shape4-nomatch=#{(re4 =~ 'goodbye').inspect}"

## Shape 5: native-path captures still work end-to-end via
## String#match (the migration left captures on the linear
## engine through `as_native()`).
re5 = /(\w+):(\d+)/
m = 'foo:42'.match(re5)
puts "shape5-cap0=#{m[0]}"
puts "shape5-cap1=#{m[1]}"
puts "shape5-cap2=#{m[2]}"
