# M27 A1+A2+A3: Module#define_method body — block-arg capture +
# block_given? + yield semantics. The Sinatra route table's
# `define_method(verb) do |path, &block|; ...; block.call; end`
# idiom needs the `&block` arrival to land in the slot. CRuby's
# define_method'd body is a Proc, so `block_given?` returns false
# and `yield` is a parse-time SyntaxError (both runtimes agree on
# the latter — covered by m27_define_method_yield_syntax_error.rb
# under tests/syntax_errors if/when added; here we just sanity-
# check block_given? and the &blk capture).

# 1. Explicit-capture &blk gets the caller's block.
class A
  define_method(:dm) do |&blk|
    blk ? blk.call : "none"
  end
end
puts A.new.dm              # "none"
puts(A.new.dm { "yes" })   # "yes"

# 2. With positional + &blk.
class B
  define_method(:b) do |x, &blk|
    "x=#{x} blk=#{blk ? blk.call : :none}"
  end
end
puts B.new.b(42)               # x=42 blk=:none  (no caller block)
puts(B.new.b(7) { "from-blk" })  # x=7 blk=from-blk

# 3. block_given? inside a define_method'd body returns FALSE even
# when the caller passes a block (CRuby's documented semantics —
# the body is a Proc, not a method).
class C
  define_method(:c) do
    block_given? ? "yes" : "no"
  end
end
puts C.new.c              # "no"
puts(C.new.c { "x" })     # "no" (CRuby), not "yes"

# 4. *rest in a define_method'd body now works too (drive-by fix —
# proto.rest_param wasn't being stamped on block protos before).
class D
  define_method(:d) do |*args|
    args.inspect
  end
end
puts D.new.d(1, 2, 3)     # [1, 2, 3]
puts D.new.d              # []
