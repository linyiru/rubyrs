# Per-instance singleton methods on built-in HEAP objects (Array /
# Proc) via the heap_singletons side-table — both define_singleton_method
# and the `def obj.x` form. Per-instance (a sibling instance does NOT
# see them), and the native dispatch still works underneath. rack's
# Deflater/Lock define :close on an Array body; ContentLength defines
# on a Proc body. Zero require — covers the always-compiled dispatch /
# lookup / def paths.
def t(l); r = begin; yield.inspect; rescue => e; "#{e.class}: #{e.message[0, 50]}"; end; puts "#{l}: #{r}"; end

# --- Array instance ---
a = [1, 2, 3]
a.define_singleton_method(:closed) { "closed:#{size}" }
t("arr dsm")        { a.closed }                 # closure sees the array
t("arr respond")    { a.respond_to?(:closed) }   # true
t("arr sibling")    { [9, 9].respond_to?(:closed) }  # false — per-instance
t("arr native")     { a.map { |x| x * 2 } }      # native dispatch intact
t("arr ivar+sing")  { a << 4; a.closed }          # mutation visible in closure
def a.tag; "A-tag"; end                            # def-receiver form
t("arr def-recv")   { a.tag }

# --- Proc instance ---
pr = proc { |x| x + 1 }
pr.define_singleton_method(:label) { "P-label" }
t("proc dsm")       { pr.label }
t("proc respond")   { pr.respond_to?(:label) }
t("proc call")      { pr.call(10) }              # native call still works
def pr.note; "P-note"; end
t("proc def-recv")  { pr.note }

# A built-in WITHOUT a singleton still rejects unknown methods.
t("no singleton")   { [7].frobnicate rescue ($!.class) }
