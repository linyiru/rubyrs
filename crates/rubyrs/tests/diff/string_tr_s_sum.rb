# String#tr_s — tr + squeeze of the TRANSLATED runs only; String#sum —
# byte checksum with optional bit width.
p "hello".tr_s("l", "r")
p "aabbaabb".tr_s("ab", "xy")
p "mississippi".tr_s("ps", "**")
p "hello world".tr_s("lo", "*")
p "bookkeeper".tr_s("ok", "_")
p "hello".tr_s("el", "ip")
p "al".tr_s("l", "a")          # translated never merges with untranslated
p "aabb".tr_s("a", "b")
p "aabb".tr_s("c", "x")        # no translation → unchanged
p "hello".tr_s("l", "")        # empty to → delete
p "hello".tr_s("a-y", "b-z")
p "hello".sum
p "".sum
p "hello".sum(8)
p "hello world".sum
p "hello".sum(0)
p "x".respond_to?(:tr_s)
p "x".respond_to?(:sum)
