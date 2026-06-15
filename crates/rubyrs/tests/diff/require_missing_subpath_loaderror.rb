# A missing multi-segment require (`require "foo/bar"`) raises LoadError —
# it must NOT be lenient-satisfied by the existence of a same-named
# top-level module `Foo`. concurrent-ruby's native loader relies on this:
# `require "concurrent/concurrent_ruby_ext" rescue LoadError` PICKS the
# pure-Ruby fallback; a false success made it think the C ext loaded.
module Concurrent
end

# Single-segment require of a defined module is irrelevant here; the
# point is the sub-path under it must still fail.
begin
  require "concurrent/concurrent_ruby_ext"
  puts "NO ERROR (wrong)"
rescue LoadError => e
  puts "LoadError"
end

# Same for an arbitrary undefined sub-path under a defined module.
module Foo; end
begin
  require "foo/does_not_exist"
  puts "NO ERROR (wrong)"
rescue LoadError
  puts "LoadError"
end
