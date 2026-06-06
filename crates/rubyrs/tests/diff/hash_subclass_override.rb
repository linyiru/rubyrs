# A Hash subclass can OVERRIDE Hash methods and call `super` to reach
# the Hash primitive — user overrides win over the primitives, and
# `super` from the override dispatches to the primitive. Mirrors
# safe_yaml's CaseAgnosticMap. Discovery: P3 Jekyll spike.
class CaseMap < Hash
  def initialize(*args)
    super
    @created = true
  end

  def []=(key, value)
    super(key.to_s.downcase, value)
  end

  def [](key)
    super(key.to_s.downcase)
  end

  def include?(key)
    super(key.to_s.downcase)
  end

  def freeze   # override with NO super
    self
  end

  def created?
    @created
  end
end

m = CaseMap.new
p m.created?               # initialize ran (with super)
m["ABC"] = 1               # []= override downcases the key
m[:DeF] = 2
p m["abc"]                 # [] override downcases -> finds it
p m["AbC"]
p m[:def]
p m.include?("ABC")        # include? override downcases
p m.size                   # inherited primitive
p m.keys                   # stored downcased
p m.freeze.equal?(m)       # override returns self

# merge! is NOT overridden -> the primitive fires
m2 = CaseMap.new
p m2.merge!({ "x" => 9 })
p m2.class
