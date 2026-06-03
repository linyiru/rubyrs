## `String#split(regex[, limit])` — dual-engine: native
## `regex` for simple patterns, fancy-regex fallback for
## lookaround / backref patterns (transparent — same call
## site).
##
## Discovery: TRY_RUNS pass-14 — sinatra-4's `cleaned_caller`
## (sinatra/base.rb:1913) does `line.split(/:(?=\d|in )/, 3)`.
## Layer #17 (PR #353) unlocked the lookahead pattern's
## compilation; this layer makes the split actually run.
## (Layer #18.)

## Shape 1: native regex, no limit — drop trailing empties.
puts "shape1=#{"a,b,,".split(/,/).inspect}"

## Shape 2: native regex with positive limit — at most N
## fields, last is the unsplit remainder.
puts "shape2=#{"a,b,,".split(/,/, 2).inspect}"

## Shape 3: native regex with negative limit — keep trailing
## empties.
puts "shape3=#{"a,b,,".split(/,/, -1).inspect}"

## Shape 4: capture groups are included in the result between
## the surrounding chunks (CRuby's `split` rule).
puts "shape4=#{"a1b2c".split(/(\d)/).inspect}"

## Shape 5: empty source returns [] regardless of pattern.
puts "shape5=#{"".split(/,/).inspect}"

## Shape 6: the motivating sinatra pattern — lookahead
## `(?=...)` triggers the fancy-regex fallback. The split
## logic walks engine-agnostic owned positions, so the
## behavior is identical to the native path.
puts "shape6a=#{"a:1:in c".split(/:(?=\d|in )/).inspect}"
puts "shape6b=#{"a:1:in c".split(/:(?=\d|in )/, 2).inspect}"
puts "shape6c=#{"a:1:in c:in d".split(/:(?=\d|in )/, 3).inspect}"
puts "shape6d=#{"a:1:in c".split(/:(?=\d|in )/, -1).inspect}"

## Shape 7: negative lookahead — also fancy-engine.
puts "shape7=#{"foo barr baz bar".split(/bar(?!r)/).inspect}"

## Shape 8: lookbehind — also fancy-engine.
puts "shape8=#{"$10 $20 fifty".split(/(?<=\$)/).inspect}"

## Shape 9: limit=0 is equivalent to no limit (drop trailing
## empties).
puts "shape9=#{"a,b,,".split(/,/, 0).inspect}"

## Shape 10: split that produces no matches returns the
## original string as a single-element array.
puts "shape10=#{"hello".split(/X/).inspect}"

## Shape 11: positive limit interacting with capture groups.
## Per CRuby docs: "captured groups will be returned as well,
## but are not counted towards the limit". So `split(/(\d)/, 2)`
## takes the first split (limit-1 = 1 chunk before remainder),
## emits the capture group for that match, then jams the
## remainder verbatim into the final slot. Result has 3
## elements even though limit was 2. Code-review #357 round 3
## flagged this path as worth pinning.
puts "shape11a=#{"a1b2c".split(/(\d)/, 2).inspect}"
puts "shape11b=#{"a1b2c".split(/(\d)/, 3).inspect}"
puts "shape11c=#{"a1b2c".split(/(\d)/, 99).inspect}"

## Shape 12: same with multiple capture groups — each match
## pushes its (matched_chunk, group_1, group_2, ...) tuple in
## order, none of them counting toward the limit.
puts "shape12=#{"abc-123_def-456".split(/(-)(\d+)/, 2).inspect}"
