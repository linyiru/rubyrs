# Array#find / #detect with an `ifnone` callable: when no element
# matches, the ifnone proc is invoked and its result returned (CRuby
# Enumerable#find). Discovery: P3 Jekyll spike — configuration.rb's
# `%w(yml yaml toml).find(-> { "yml" }) { |ext| File.exist?(...) }`.
p [1, 2, 3].find { |x| x > 1 }                 # plain block form
p [1, 2, 3].find(-> { :none }) { |x| x > 10 }  # ifnone fires (no match)
p [1, 2, 3].find(-> { :none }) { |x| x > 1 }   # match → ifnone ignored
p %w[a b c].detect(-> { "fallback" }) { |s| s == "z" }
p %w[a b c].detect(-> { "fallback" }) { |s| s == "b" }
# ifnone result can be computed
default = -> { [10, 20].sum }
p [].find(default) { |x| x > 0 }
