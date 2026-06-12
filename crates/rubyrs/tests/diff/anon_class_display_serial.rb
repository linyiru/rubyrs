# Anonymous Class/Module display: CRuby renders #<Class:0xADDR>;
# rubyrs substitutes a deterministic creation serial for the hex
# digits (ADR 0017 keeps raw addresses out). Compare SHAPE, not
# digits.
norm = ->(s) { s.sub(/0x[0-9a-f]+/, "0xN") }
c = Class.new(StandardError)
puts norm[c.to_s]
puts norm[c.inspect]
puts norm["#{c}"]
p c.name
# named classes never show an id
D2 = Class.new
p D2.to_s
class Plain; end
p Plain.to_s
