# Core-only twin of rack_spec_lib_fixes.rb (which requires
# stringio/tempfile/yaml and therefore doesn't run in the bare /
# Coverage configurations): pins the Hash in-place/query arms
# (vm/hash.rs + vm/iter.rs), the String slice!/index/rindex/scrub
# family (vm/string.rs), and the eigenclass-undef builtin
# (vm/kernel.rs) with ZERO requires so the per-file coverage
# ratchet sees them in every build.
def t(l); r = begin; yield.inspect; rescue => e; "#{e.class}: #{e.message[0, 70]}"; end; puts "#{l}: #{r}"; end

h = { "a" => "1", "b" => "2" }
t("assoc hit")  { h.assoc("a") }
t("assoc miss") { h.assoc("z") }
t("rassoc")     { [h.rassoc("2"), h.rassoc("9")] }
t("shift")      { x = h.dup; [x.shift, x, {}.shift] }
t("value?")     { [h.value?("1"), h.has_value?("9")] }
t("select!")    { x = h.dup; [x.select! { |k, _| k == "a" }, x.select! { true }] }
t("keep_if")    { x = h.dup; [x.keep_if { true }.size, x.keep_if { |k, _| k == "b" }] }
t("reject!")    { x = h.dup; [x.reject! { |k, _| k == "a" }, x.reject! { false }] }
t("delete_if")  { x = h.dup; x.delete_if { |_, v| v == "2" } }

# String#slice! — every read shape, destructively
t("slice! n")    { x = +"hello world"; [x.slice!(0, 5), x] }
t("slice! i")    { x = +"hello"; [x.slice!(1), x] }
t("slice! neg")  { x = +"abc"; [x.slice!(-2, 1), x] }
t("slice! oob")  { x = +"abc"; [x.slice!(9, 2), x] }
t("slice! rng")  { x = +"hello"; [x.slice!(1...3), x] }
t("slice! str")  { x = +"hello world"; [x.slice!("lo w"), x, x.slice!("zz")] }
t("slice! self") { x = +"abc"; [x.slice!(x), x] }
t("slice! mb")   { x = +"héllo"; [x.slice!(1, 2), x] }
t("slice! re")   { x = +"hello"; [x.slice!(/l+o/), x, $~ && $~[0]] }
t("slice! cap")  { x = +"hello"; [x.slice!(/l(l)o/, 1), x] }
t("slice! miss") { x = +"abc"; [x.slice!(/zz/), x] }
t("slice! 0arg") { begin; (+"abc").slice!; rescue ArgumentError => e; e.message; end }

# String#index(regexp[, offset]) + rindex(str, offset)
s = "a;b;c;d"
t("idx re")      { s.index(/;/) }
t("idx re off")  { [s.index(/[;]/, 3), s.index(/z/, 2)] }
t("idx re $~")   { s.index(/(;)./); [$1, $~ && $~[0]] }
t("rindex off")  { [s.rindex(";", 4), s.rindex(";", 0), s.rindex("", 3)] }
t("idx fancy")   { "foobar".index(/foo(?=bar)/) }

# String#scrub / scrub!
t("scrub!")      { x = +"ab\xFFc"; [x.scrub!, x] }
t("scrub! rep")  { x = +"ab\xFF\xFEc"; x.scrub!("?") }
t("scrub! nil")  { (+"abc").scrub! }
t("scrub")       { x = "ab\xFFc"; [x.scrub, x.valid_encoding?] }

# regex captures through the dual engine (String#[] / case-eq)
t("bracket fancy") { "ab1c"[/ab.(?=c)/] }
t("bracket cap")   { "x: 42"[/x: (\d+)/, 1] }
t("case-eq fancy") { case "foobar"; when /foo(?=bar)/ then "hit #{$~[0]}"; else "miss"; end }
t("eq3 backref")   { /(\w+) bb \1/ === "aa bb aa" }

# Struct-subclass member resolution (rack's MimePart hierarchy:
# `class BufferPart < MimePart` where MimePart < Struct.new(...));
# the members/keyword tables live on the anonymous generated
# parent, reached via a superclass walk. Also the reproducer for
# the GC superclass-chain root hole (the anonymous parent's
# @__struct_attrs Array was unreachable from every root).
SPBase = Struct.new(:a, :b)
class SPSub < SPBase
  def extra; "#{a}-#{b}"; end
end
sp = SPSub.new(1, 2)
t("struct sub") { [sp.a, sp.extra, sp.members, SPSub.members, sp.to_h] }
SPKw = Struct.new(:x, keyword_init: true)
class SPKwSub < SPKw; end
t("struct sub kw") { SPKwSub.new(x: 9).x }

# undef inside instance_eval (eigenclass tombstone)
class UndefIeProbe
  def gone; :reachable; end
end
u = UndefIeProbe.new
u.instance_eval { undef :gone }
t("undef ie") { [u.respond_to?(:gone), UndefIeProbe.new.respond_to?(:gone)] }
t("undef ie call") { begin; u.gone; rescue NoMethodError; :nme; end }
