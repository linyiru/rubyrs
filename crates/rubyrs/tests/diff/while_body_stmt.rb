# A `while` / `until` body is compiled with every value discarded, the
# last statement included (`i += 1` → IncLocalNoPush). The loop's own
# value, break values, next / redo, and side effects must not change.
def count(n); i = 0; while i < n; i += 1; end; i; end
p count(5), count(0)

def last_ivar; @k = 0; j = 0; while j < 3; j += 1; @k += 1; end; @k; end
p last_ivar

# The loop expression's value: nil normally, the break value on break.
r = (i = 0; while i < 3; i += 1; end)
p r
r = (i = 0; while true; i += 1; break i * 10 if i == 4; end)
p r
r = (i = 0; until i >= 2; i += 1; end)
p r

# Every statement kind as the last one of the body.
def shapes
  out = []; i = 0; s = +""; h = {}; a = []
  while i < 3
    out << i
    s << "x"
    h[i] = i * i
    a[i] = i
    x = i * 2
    $g = x
    i += 1
  end
  [out, s, h, a, $g]
end
p shapes
Kst = []
def const_last; i = 0; while i < 2; i += 1; Kst << i; end; Kst; end
p const_last

# Last statement is a call with a side effect, or an if/else.
def side; log = []; i = 0; while i < 3; i += 1; log.push(i) if i.odd?; end; log; end
p side
def branchy; t = 0; i = 0; while i < 4; i += 1; if i.even? then t += 10 else t += 1 end; end; t; end
p branchy

# next / redo / empty body / post-condition form.
def nexts; i = 0; t = 0; while i < 6; i += 1; next if i.odd?; t += i; end; t; end
p nexts
def redos; i = 0; tries = 0; while i < 3; tries += 1; i += 1; redo if tries == 2; end; [i, tries]; end
p redos
def empty; i = 0; while (i += 1) < 3; end; i; end
p empty
def post; i = 10; begin; i += 1; end while i < 3; i; end
p post
def post_until; i = 0; begin i += 2 end until i > 5; i; end
p post_until

# Integer limit and a non-Integer `+ 1` as the last statement.
def big; v = 2**63 - 2; i = 0; while i < 3; i += 1; v += 1; end; v; end
p big
def flt; f = 0.5; i = 0; while i < 2; i += 1; f += 1; end; f; end
p flt

# Nested loops, and a loop inside a block.
def nested; t = 0; i = 0; while i < 3; j = 0; while j < 3; j += 1; t += j; end; i += 1; end; t; end
p nested
p [1, 2].map { |m| k = 0; while k < m; k += 1; end; k }
