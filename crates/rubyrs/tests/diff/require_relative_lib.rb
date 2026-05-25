# Loaded by require_relative_main.rb. Top-level definitions
# (method, constant) must be visible to the loader after the
# require_relative call returns.
def greet(name)
  "hello, #{name}"
end

SHARED = 42

class Greeter
  def initialize(prefix); @prefix = prefix; end
  def call(name); "#{@prefix}, #{name}"; end
end
