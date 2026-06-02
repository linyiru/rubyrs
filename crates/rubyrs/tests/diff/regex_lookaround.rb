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
## This PR lands ONLY the compile fallback. Match-time
## operations (is_match, replace, find_iter, captures, etc.)
## haven't been migrated to the dual-engine dispatcher yet;
## using them on a fancy-engine pattern raises a clear
## RuntimeError naming the unsupported operation. Subsequent
## PRs migrate each operation incrementally as gem layers
## demand it.

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
