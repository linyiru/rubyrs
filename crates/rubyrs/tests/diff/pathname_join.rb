# `Pathname#join(*parts)` — append parts via `#+` (normalises `.`/`..`,
# an absolute part resets). Surfaced by bridgetown-core/collection.rb's
# `relative_path` (`container.join(relative_directory)`).
require "pathname"
puts Pathname.new("/usr").join("bin", "ruby")
puts Pathname.new("/usr").join("a", "/b")
puts Pathname.new("rel").join("x", "y")
puts Pathname.new("/x").join
