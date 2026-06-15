# `alias` inside `class << <Const>` / `class << <obj>` (a NON-self
# singleton-class receiver) — routes to the real eigenclass body so the
# alias lands on the singleton-method table. Previously bailed with
# "`alias` only supported when receiver is `self`". Surfaced by stdlib
# net/http.rb (`class << HTTP; alias is_version_1_1? version_1_1?`).
class Foo
  def self.original(x) = "orig:#{x}"
  class << Foo
    alias aliased original
  end
end
p Foo.aliased(5)
p Foo.original(5)

# `class << self; alias` (self receiver) keeps its existing path.
class Bar
  def self.real = "real"
  class << self
    alias alt real
  end
end
p Bar.alt

# Mixed def + alias in a non-self singleton body.
module Baz
  def self.base = 1
  class << Baz
    def extra = 2
    alias base_alias base
  end
end
p [Baz.extra, Baz.base_alias]

# class << <obj> (a plain object's singleton) with alias.
obj = Object.new
def obj.greet = "hi"
class << obj
  alias hello greet
end
p obj.hello
