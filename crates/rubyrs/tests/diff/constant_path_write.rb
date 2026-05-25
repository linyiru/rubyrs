# `Foo::Bar = expr` — ConstantPathWriteNode follow-up to PR #30's
# bare ConstantWriteNode. rubyrs flattens the path into a joined
# "A::B" name and routes through the existing Vm.constants table.
# Both rubyrs and CRuby must produce identical stdout (CRuby's
# "already initialized" warnings go to stderr and don't count).

# Basic path write on a top-level class.
Foo = Class.new
Foo::Bar = 42
puts Foo::Bar

# Re-assigning rebinds the joined name.
Foo::Bar = "hello"
puts Foo::Bar

# Path write inside a class body — same joined-name storage as
# top-level (rubyrs has no real module nesting; the path is
# already flat by the time it reaches the constants table).
class Box
end
Box::SIZE = 10
Box::COLOR = "red"
puts Box::SIZE
puts Box::COLOR

# Three-segment path.
class Tree
end
Tree::Node = Class.new
Tree::Node::CAPACITY = 100
puts Tree::Node::CAPACITY

# Multiple constants under the same class.
Box::WIDTH = 3
Box::HEIGHT = 4
puts Box::WIDTH + Box::HEIGHT

# Expression-form: the write yields its value (matches `FOO = 42`
# from PR #30).
x = (Foo::Z = 99)
puts x
puts Foo::Z
