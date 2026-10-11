# Recursion that passes through a block on every level. Each level
# re-enters dispatch_until (a Rust-stack recursion), and rubyrs caps
# that at 150 in release builds (DEFAULT_MAX_DISPATCH_DEPTH in
# vm/gc.rs), so these raise SystemStackError where CRuby, bounded only
# by its VM stack, returns. Quarantined as `known:` in diff_cruby.rs
# until block calls stop recursing on the Rust stack.
class O; def y; yield; end; end
O_ = O.new
def deep(o, n) = n.zero? ? o.y { :bottom } : o.y { deep(o, n - 1) }
def y; yield; end
def deep_self(n) = n.zero? ? y { :bottom } : y { deep_self(n - 1) }
def deep_each(n) = n.zero? ? :bottom : [1].each { return deep_each(n - 1) }
def deep_plain(n) = n.zero? ? :bottom : deep_plain(n - 1)

def try(label)
  [100, 160, 1000].each do |n|
    r = begin; yield(n); rescue SystemStackError; :SystemStackError; end
    puts "#{label} n=#{n}: #{r}"
  end
end
try("explicit receiver yield") { |n| deep(O_, n) }
try("implicit self yield") { |n| deep_self(n) }
try("native each") { |n| deep_each(n) }
try("plain call") { |n| deep_plain(n) }
