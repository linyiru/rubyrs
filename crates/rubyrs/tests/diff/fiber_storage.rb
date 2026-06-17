# Fiber storage API (Ruby 3.2+): `Fiber[]` / `Fiber[]=`. Single-fiber
# model backs it with one process-global store. multi_json caches its
# per-call adapter override in `Fiber[:multi_json_adapter]`.
p Fiber[:missing]            # nil  (unset reads as nil)
Fiber[:a] = 1
p Fiber[:a]                  # 1
p((Fiber[:b] = 9))           # 9    (setter returns the assigned value)
Fiber["str_key"] = "v"       # String keys allowed too
p Fiber["str_key"]           # "v"

# overwrite
Fiber[:a] = 2
p Fiber[:a]                  # 2

# non-Symbol/String keys raise TypeError
begin; Fiber[1]; rescue TypeError => e; puts e.message; end       # 1 is not a symbol nor a string
begin; Fiber[nil]; rescue TypeError => e; puts e.message; end     # nil is not a symbol nor a string
begin; Fiber[[1]] = 0; rescue TypeError => e; puts e.message; end # [1] is not a symbol nor a string

# save/restore pattern (multi_json's adapter= override)
def with_override(v)
  prev = Fiber[:adapter]
  Fiber[:adapter] = v
  yield
ensure
  Fiber[:adapter] = prev
end
Fiber[:adapter] = :default
with_override(:oj) { p Fiber[:adapter] }   # :oj
p Fiber[:adapter]                            # :default
