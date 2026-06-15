# Harness stub for `require "pp"`. rubyrs has Kernel#pp/#p; minitest's
# mu_pp failure-formatter calls Object#pretty_inspect (defined by the
# real pp.rb), so define a minimal version so a test FAILURE renders
# as a failure instead of erroring inside the formatter.
class Object
  def pretty_inspect
    inspect
  end
end
