# Array#freeze enforcement: every mutating method raises FrozenError on
# a frozen array — UNCONDITIONALLY, even a no-op `uniq!` on an already-
# unique array (CRuby checks frozen before deciding whether the call
# would change anything). `clone` preserves the frozen bit; `dup`
# resets it. rack's Lock relies on `[].freeze.pop` raising so its
# `ensure` unlocks the mutex.

def t(desc); yield; puts "#{desc}: ok"; rescue => e; puts "#{desc}: #{e.class}"; end

a = [1, 2, 3].freeze
p a.frozen?

# Mutators raise FrozenError (incl. assignment syntax + block forms +
# no-op bang methods).
t("<<")        { a << 4 }
t("push")      { a.push(4) }
t("pop")       { a.pop }
t("shift")     { a.shift }
t("unshift")   { a.unshift(0) }
t("[]=")       { a[0] = 9 }
t("concat")    { a.concat([5]) }
t("insert")    { a.insert(1, 9) }
t("delete")    { a.delete(2) }
t("clear")     { a.clear }
t("fill")      { a.fill(0) }
t("uniq!")     { a.uniq! }          # already unique — still raises
t("sort!")     { a.sort! }
t("compact!")  { a.compact! }       # no nils — still raises
t("reverse!")  { a.reverse! }
t("rotate!")   { a.rotate! }
t("map!")      { a.map! { |x| x } } # block form
t("select!")   { a.select! { |x| x > 1 } }
t("reject!")   { a.reject! { |x| x > 1 } }
t("slice!")    { a.slice!(0) }

# Reads never raise on a frozen array.
p a.first
p a.map { |x| x * 2 }
p a.length
p a.include?(2)

# dup resets frozen; clone preserves it.
d = a.dup
p d.frozen?
d << 4
p d
c = a.clone
p c.frozen?
t("clone <<")  { c << 9 }

# The full error message includes the array's inspect.
begin
  [1, 2].freeze << 3
rescue FrozenError => e
  puts e.message
end
