# A required file's body runs at top-level lexical nesting: its top-level
# `def`s become private Object methods (global functions), regardless of
# whether the `require` call sits inside a class/module body. Mirrors
# mustermann's `require 'delegate'` inside Hanami::Router's class body,
# where `DelegateClass` must end up global, not on Router.
HELPER = "/tmp/rubyrs_req_scope_helper.rb"
File.write(HELPER, <<~RUBY)
  def global_helper_fn(x)
    "helper:\#{x}"
  end

  module TopLevelMod
    def self.tag = "toplevelmod"
  end
RUBY

class Router
  require HELPER
  # The required file's def landed at top level, so the bare call here
  # resolves it as a private Object method — not as a Router method.
  RESULT = global_helper_fn("in-body")
end

p Router::RESULT
p global_helper_fn("top-level")
p TopLevelMod.tag
# The method is private on Object (a global function), so it is NOT a
# public method of an arbitrary instance.
p Router.new.respond_to?(:global_helper_fn)
# The required file's module landed at top level too.
p Object.const_defined?(:TopLevelMod)

File.delete(HELPER)
