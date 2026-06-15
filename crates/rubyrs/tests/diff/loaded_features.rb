# $LOADED_FEATURES / $" — the script-visible Array of loaded paths.
# Was nil (rubyrs tracked loads internally but didn't expose the
# global); zeitwerk's Kernel#require wrapper reads `.last` and its
# unload path calls `.reject!`.

# Exists as a real Array before any user require (preamble doesn't
# pollute it; lazily materialised on read).
p $LOADED_FEATURES.is_a?(Array)
p $LOADED_FEATURES.equal?($")          # $" is the alias
p $LOADED_FEATURES.respond_to?(:reject!)

# A real file require appends its canonical path; `.last` recovers it.
dir = File.expand_path("loaded_features_lib", __dir__)
$LOAD_PATH.unshift(dir)
before = $LOADED_FEATURES.size
r = require "leaf"
p r                                    # true
p $LOADED_FEATURES.last.end_with?("leaf.rb")
p $LOADED_FEATURES.size == before + 1
p require("leaf")                      # false — already loaded
p LeafConst                            # :leaf

# reject! mutates the Array and returns it.
removed = $LOADED_FEATURES.reject! { |f| f.include?("__definitely_absent__") }
p removed.nil?                         # nil — nothing rejected (CRuby reject! returns nil when no change)
