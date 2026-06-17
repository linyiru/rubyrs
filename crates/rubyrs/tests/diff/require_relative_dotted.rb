# require_relative of a path whose basename has a dotted segment
# (`require_relative_dotted_v1.0`): its trailing ".0" is NOT a
# loadable extension, so Ruby appends ".rb" to find the file —
# it must NOT replace the ".0". rss requires `maker/1.0` this way.
result = require_relative "require_relative_dotted_v1.0"
puts result
puts DOTTED_LOADED
puts dotted_greet

# Idempotent: second require returns false (already loaded).
puts require_relative "require_relative_dotted_v1.0"

# Explicit ".rb" still resolves to the same canonical file (dedup).
puts require_relative "require_relative_dotted_v1.0.rb"
