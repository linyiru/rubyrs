# Proc#parameters — [[kind, name?], …]. A non-lambda proc reports its
# required positionals as :opt (procs are arity-lenient); a lambda as
# :req. Keywords/rest/block match. dry-core's container Item does
# `item.parameters.empty?` to decide whether to call a stored proc.
p ->(a, b = 1, *c, d:, e: 2, **f, &g) {}.parameters
p lambda { |x| }.parameters
p proc { |x, y| }.parameters
p proc { |x, y = 1, *z| }.parameters
p ->() {}.parameters
p proc {}.parameters
p ->(a, b) {}.parameters
p proc { |*| }.parameters
p ->(**) {}.parameters
p proc { |a:, b: 2| }.parameters
p ->(x, *rest, &blk) {}.parameters
p proc {}.parameters.empty?
p proc { |x| }.parameters.empty?
