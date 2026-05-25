# Loaded by require_relative_main.rb. Top-level definitions
# (method, constant) must be visible to the loader after the
# require_relative call returns.
def greet(name)
  "hello, #{name}"
end

# A method that does a non-local return from inside a block.
# Loading this file used to be aborted by an earlier round of
# the require_relative impl that bailed on the first
# method_return signal — now the unwind is handled locally and
# the rest of the file (SHARED below) still loads.
def early_first(arr)
  arr.each { |x| return x }
  :unreached
end
PROBED = early_first([10, 20, 30])

SHARED = 42

class Greeter
  def initialize(prefix); @prefix = prefix; end
  def call(name); "#{@prefix}, #{name}"; end
end
