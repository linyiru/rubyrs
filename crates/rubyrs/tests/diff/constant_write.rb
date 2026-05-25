# Differential fixture for top-level `FOO = expr` constants.
# Both rubyrs and CRuby must produce identical stdout.

# Basic
FOO = 42
puts FOO

# Method-call chain on RHS (the bundler/version.rb shape)
VERSION = "1.2.3".freeze
puts VERSION

# RHS arithmetic
THIRTY = 10 + 20
puts THIRTY

# Assignment is itself an expression — `a = (FOO2 = 7)` puts both into scope
x = (FOO2 = 7)
puts x
puts FOO2

# Constant inside a class body — same storage as top-level for our
# scope (no real module nesting).
class Box
  HOLDER = "boxed"
  def show
    HOLDER
  end
end
puts Box.new.show

# Reading a constant defined later in the program but before this point
puts FOO + FOO2

# Multiple constants in one expression
A = 1
B = 2
C = 3
puts A + B + C

# Constant with an Array RHS
LIST = [10, 20, 30]
puts LIST.length
puts LIST.first

# Constant with a Hash RHS
MAP = { a: 1, b: 2 }
puts MAP[:a]
puts MAP[:b]
