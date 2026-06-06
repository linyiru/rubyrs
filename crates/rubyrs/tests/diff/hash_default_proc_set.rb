# Hash#default_proc= sets (or clears) the missing-key default block.
# Discovery: P3 Jekyll spike — jekyll's merge_default_proc copies one
# Hash's default_proc onto another.
h = {}
p h.default_proc
h.default_proc = ->(hash, key) { hash[key] = "made:#{key}" }
p h.default_proc.nil?
p h[:x]                 # default block fires + stores
p h
src = Hash.new { |hash, k| k.to_s * 2 }
dst = {}
dst.default_proc = src.default_proc
p dst[:ab]
h.default_proc = nil
p h.default_proc
