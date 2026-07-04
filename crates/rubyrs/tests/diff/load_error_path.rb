# LoadError#path — CRuby stores the failed feature name on require-
# raised LoadErrors (nil on manually-constructed ones). ActiveRecord's
# adapter resolution branches on it (connection_handler.rb:272
# `e.path == path_to_adapter`), so `establish_connection` NoMethodErrors
# without it. rubyrs recovers the path from the canonical
# "cannot load such file -- NAME" message (kernel.rs phrases every
# require miss that way). Documented narrowing (NOT pinned here): a
# hand-raised LoadError whose message happens to match that phrasing
# reports the suffix where CRuby reports nil.
begin
  require "definitely-missing-xyz123"
rescue LoadError => e
  p e.path
  p e.message
end

# Manually-constructed LoadErrors carry no path.
p LoadError.new("boom").path
p LoadError.new("boom").message

# The method exists on the class (feature-detection surface).
p LoadError.method_defined?(:path)
