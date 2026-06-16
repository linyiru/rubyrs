# Ruby 3.2+: a default (non-keyword_init) Struct accepts keyword init,
# `S.new(a: 1, b: 2)`, in addition to positional. A single Hash whose
# keys are NOT all members stays a positional value. Surfaced by
# bridgetown's front-matter `Result.new(content:, front_matter:,
# line_count:)` (was binding the whole kwargs hash to member 0).
S = Struct.new(:content, :front_matter, :line_count)
r = S.new(content: "md", front_matter: {layout: "none"}, line_count: 2)
p [r.content, r.front_matter, r.line_count]
p S.new("pos", {b: 2}, 5).to_a            # positional still works
p S.new(content: "only").to_a              # partial keywords -> rest nil
H = Struct.new(:opts)
p H.new(opts: 9).opts                      # 1-member keyword
p H.new({foo: 1}).opts                     # hash value, key not a member -> positional
# explicit keyword_init: true still works
K = Struct.new(:a, :b, keyword_init: true)
p K.new(a: 1, b: 2).to_a
