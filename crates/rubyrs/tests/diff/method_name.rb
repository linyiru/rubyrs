# `__method__` and `__callee__` — return the enclosing method's
# name as a Symbol, or nil at the toplevel. Aliases are not
# modelled, so both forms resolve to the same name in this subset
# (CRuby distinguishes them when method aliasing is involved).

def foo
  __method__
end

def bar
  __callee__
end

p foo
p bar

# Inside a block — walks past the block frame to find the
# enclosing method.
def greet
  [1].each { |_| puts __method__ }
  [1, 2, 3].map { |n| __callee__ }
end

greet.each { |s| p s }

# Two-level call chain.
def outer
  inner
end

def inner
  __method__
end

p outer
p inner

# Class methods.
class Calculator
  def add(a, b)
    log(__method__)
    a + b
  end

  def sub(a, b)
    log(__method__)
    a - b
  end

  def log(m)
    puts "in #{m}"
  end
end

c = Calculator.new
p c.add(1, 2)
p c.sub(10, 4)

# In an instance method called via block inside another method.
class Reporter
  def run(items)
    items.map { |i| transform(i) }
  end
  def transform(x)
    "[#{__method__}: #{x}]"
  end
end

p Reporter.new.run([1, 2, 3])

# At the toplevel returns nil.
p __method__

# Inside a class body's instance-method dispatch returns the
# innermost method, not the class body.
class Probe
  def self_name
    __method__
  end
end
p Probe.new.self_name

# Method that returns its own name as a debug breadcrumb.
def debug_id
  __method__.to_s
end
puts debug_id
puts debug_id.length

# Inside nested blocks.
def nested
  result = nil
  [1].each do
    [1].each do
      result = __method__
    end
  end
  result
end

p nested

# Method that's been redefined still reports the current name.
def first_name; __method__; end
p first_name
def first_name; __method__; end   # redefined — same name
p first_name
