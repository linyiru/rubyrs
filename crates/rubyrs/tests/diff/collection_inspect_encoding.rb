# Array/Hash/Range to_s/inspect: CRuby seeds the result encoding from
# the FIRST element's inspect (Integer/Symbol-simple/nil seed US-ASCII;
# a String element seeds UTF-8 even when ascii), then promotes to UTF-8
# on any non-ASCII byte.
def e(x); x.encoding.name; end
p e([1, 2, 3].inspect)
p e([1, "a"].inspect)
p e(["a"].inspect)
p e(["a", 1].inspect)
p e([:sym, "x"].inspect)
p e([nil, true].inspect)
p e(["é"].inspect)
p e([1, [2, "é"]].inspect)
p e({a: 1}.inspect)
p e({"a" => 1}.inspect)
p e({"k" => "é"}.inspect)
p e({1 => "a"}.inspect)
p e((1..3).inspect)
p e(("a".."z").inspect)
p e([1, 2].to_s)
p e({a: 1}.to_s)
p e((1..3).to_s)
p e([].inspect)
p e({}.inspect)
p e([1.0, :x].inspect)
p [1, "a", :s].inspect
p({a: 1, "b" => 2}.inspect)
p (1..3).to_s
