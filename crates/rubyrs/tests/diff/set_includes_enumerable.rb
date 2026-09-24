# Set includes Enumerable, so methods added to Enumerable later
# (ActiveSupport's `index_with`) are reachable on a Set.
require "set"

module Enumerable
  def index_with_twice
    to_h { |e| [e, yield(e)] }
  end
end

s = Set[1, 2, 3]
p Set.ancestors.include?(Enumerable)
p s.is_a?(Enumerable)
p s.index_with_twice { |e| e * 2 }
p s.each_slice(2).to_a
p s.min_by { |e| -e }
