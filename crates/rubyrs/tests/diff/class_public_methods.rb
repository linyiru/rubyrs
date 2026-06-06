# Bareword reflection (`public_methods` / `methods`) at the top level
# of a `module`/`class` body — self is the Class, so it reports the
# class-method (singleton) chain. Discovery: P3 Jekyll spike —
# colorator's `CORE_METHODS = (public_methods - Object.methods)`.
module M
  def self.alpha; end
  def self.beta; end
  # bareword, no receiver
  pm = public_methods
  ms = methods
  puts pm.include?(:alpha)
  puts pm.include?(:beta)
  puts ms.include?(:alpha)
  puts pm.is_a?(Array)
  # public_methods and methods agree for the all-public class tier
  puts (pm.sort == ms.sort)
end

# explicit receiver
puts M.public_methods.include?(:alpha)
puts M.methods.include?(:beta)
puts M.public_methods.is_a?(Array)

# subtracting a baseline isolates the freshly-defined class methods
class C
  def self.one; end
  def self.two; end
end
own = (C.methods - Class.new.methods).sort
puts own.inspect
