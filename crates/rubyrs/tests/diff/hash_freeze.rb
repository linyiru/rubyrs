# Hash#freeze enforcement (the twin of array_freeze): every mutating
# method raises FrozenError on a frozen hash, unconditionally. `clone`
# preserves the frozen bit; `dup` resets it.

def t(d); yield; puts "#{d}: ok"; rescue => e; puts "#{d}: #{e.class}"; end

h = {"a" => 1, "b" => 2}.freeze
p h.frozen?

t("[]=")               { h["c"] = 3 }
t("store")             { h.store("c", 3) }
t("delete")            { h.delete("a") }
t("clear")             { h.clear }
t("merge!")            { h.merge!("c" => 3) }
t("update")            { h.update("c" => 3) }
t("replace")           { h.replace({}) }
t("delete_if")         { h.delete_if { true } }
t("reject!")           { h.reject! { true } }
t("select!")           { h.select! { true } }
t("keep_if")           { h.keep_if { true } }
t("transform_values!") { h.transform_values! { |v| v } }
t("transform_keys!")   { h.transform_keys! { |k| k } }
t("compare_by_identity") { h.compare_by_identity }

# Reads never raise.
p h["a"]
p h.merge("c" => 3)    # non-bang merge returns a new hash — ok
p h.size
p h.key?("b")
h.each { |k, v| }

# dup resets frozen; clone preserves it.
d = h.dup
p d.frozen?
d["z"] = 9
p d["z"]
c = h.clone
p c.frozen?
t("clone []=")         { c["z"] = 9 }

# Full message includes the hash's inspect.
begin
  {"x" => 1}.freeze["y"] = 2
rescue FrozenError => e
  puts e.message
end
