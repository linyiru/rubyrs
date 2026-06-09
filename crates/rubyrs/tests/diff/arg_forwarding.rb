# Anonymous argument forwarding: `*`, `**`, `&`, their combinations, and
# Ruby 3.0 `...`. The def side binds reserved sentinel slots; the call
# site reads them back. An EMPTY keyword-rest forward (`**{}` / a `...`
# with no kwargs) must contribute nothing, not a phantom positional `{}`.

# --- anonymous rest `*` ---
def t2(a, b); [a, b]; end
def m_star(*); t2(*); end
p m_star(1, 2)

def tc(*a); a; end
def m_star2(*); tc(*); end
p m_star2(1, 2, 3)

# leading positional + anonymous rest
def t3(a, b, c); [a, b, c]; end
def m_lead(x, *); t3(x, *); end
p m_lead(1, 2, 3)

# --- anonymous keyword-rest `**` ---
def tk(a:, b:); [a, b]; end
def m_dstar(**); tk(**); end
p m_dstar(a: 1, b: 2)

def tkr(**o); o; end
def m_dstar2(**); tkr(**); end
p m_dstar2(x: 9, y: 8)

# empty `**` contributes nothing
def tnone; 42; end
def m_empty(**); tnone(**); end
p m_empty

# --- anonymous block `&` combined with a splat ---
def tb(a); yield a; end
def m_starblk(*, &); tb(*, &); end
p m_starblk(7) { |x| x + 1 }

# --- mixed `*, **`, empty kwargs must drop ---
def t2b(a, b); [a, b]; end
def m_mix(*, **); t2b(*, **); end
p m_mix(1, 2)

# --- Ruby 3.0 `...` ---
def tt(a, b); [a, b]; end
def d_pos(...); tt(...); end
p d_pos(1, 2)

def ttk(a, b:, c:); [a, b, c]; end
def d_kw(...); ttk(...); end
p d_kw(1, b: 2, c: 3)

def ttb(a); yield a; end
def d_blk(...); ttb(...); end
p d_blk(5) { |x| x * 10 }

def ttn; 99; end
def d_none(...); ttn(...); end
p d_none

def tt3(x, a, b); [x, a, b]; end
def d_lead(x, ...); tt3(x, ...); end
p d_lead(1, 2, 3)

def tall(*a, **k, &b); [a, k, b ? b.call : nil]; end
def d_all(...); tall(...); end
p d_all(1, 2, k: 9) { 42 }
p d_all(1)
