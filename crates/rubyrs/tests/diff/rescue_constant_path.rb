# `rescue Foo::Bar`: the rescue clause's constant path should
# resolve to the nested class, matching exactly what `raise
# Foo::Bar` would have raised. Previously rubyrs only used the
# trailing segment (`Bar`), which mismatched whenever the
# qualified-form alias was the only key under which the class
# lived.
#
# Note: this fixture avoids `class Bar; end` at top level
# colliding with `module Foo; class Bar; end; end` — rubyrs's
# single-class table is documented separately (issue #224).
# Names below are unique to each scope.

module Outer
  class InnerErr < StandardError; end
end

def raise_qualified
  raise Outer::InnerErr, "boom"
rescue Outer::InnerErr => e
  "matched:#{e.message}"
end
puts raise_qualified

# Multi-segment path also works.
module Net
  module HTTP
    class TimeoutError < StandardError; end
  end
end

def deep_path
  raise Net::HTTP::TimeoutError, "timed out"
rescue Net::HTTP::TimeoutError => e
  "deep:#{e.message}"
end
puts deep_path

# `Gem::LoadError`-style gem-helper pattern.
module Gem
  class LoadError < StandardError; end
end

def gem_style
  raise Gem::LoadError, "missing dep"
rescue Gem::LoadError => e
  "gem:#{e.message}"
end
puts gem_style

# Mixed with multi-class rescue: qualified + bare in the same
# clause. Both raised forms route into the same handler body.
class TopErr < StandardError; end

module Wrap
  class WErr < StandardError; end
end

def mixed_first
  raise TopErr, "top"
rescue Wrap::WErr, TopErr => e
  "mixed:#{e.class.name}:#{e.message}"
end
puts mixed_first

def mixed_second
  raise Wrap::WErr, "w"
rescue Wrap::WErr, TopErr => e
  "mixed:#{e.class.name}:#{e.message}"
end
puts mixed_second
