# `ensure` must run when a method/begin exits via `return` (it
# previously only ran on fall-through and on exceptions).
def ret_val; $a = []; return 9; ensure; $a << :ens; end
v = ret_val
p [v, $a]

def bare_ret; $b = []; return; ensure; $b << :ens; end
bare_ret
p $b

def begin_ret; $c = []; begin; return :x; ensure; $c << :ens; end; end
r = begin_ret
p [r, $c]

# Conditional return.
def cond(x); $d = []; return :early if x; $d << :normal; ensure; $d << :ens; end
cond(true)
p $d

# Nested ensures both run, innermost first.
def nest
  $o = []
  begin
    begin
      return :x
    ensure
      $o << :inner
    end
  ensure
    $o << :outer
  end
end
nest
p $o

# An ensure that itself `return`s overrides the body's value.
def ens_overrides
  return :body
ensure
  return :ensure_wins
end
p ens_overrides

# return inside a rescue still runs the ensure.
def resc_ret
  $r = []
  begin
    raise "x"
  rescue
    return :rescued
  ensure
    $r << :ens
  end
end
p [resc_ret, $r]

# An exception still propagates through the ensure (no return).
def raises
  $x = []
  begin
    raise "boom"
  ensure
    $x << :ens
  end
end
begin
  raises
rescue => e
  p [e.message, $x]
end

# Normal fall-through still runs the ensure (regression guard).
def fall; $f = []; $f << :body; ensure; $f << :ens; end
fall
p $f
