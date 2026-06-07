# Regression guard for the block-locals recycle-pool reuse in
# invoke_block. The optimization reuses one pooled locals cell across
# non-escaping block invocations (the strong_count==1 guard skips
# escaping ones), so this pins the exact semantics it could break:
#   - outer-var write-through accumulates across iterations
#   - an ESCAPING closure created per-iteration keeps its OWN snapshot
#     (must NOT alias the reused cell -> the .each-capture-leak)
#   - block-body locals are fresh each invocation (not carried over)
#   - nested blocks (a reused inner cell inside a reused outer cell)

# (1) outer-write accumulation across many reused-cell iterations
total = 0
(1..1000).each { |x| total += x }
p total                                 # 500500

# (2) escaping closures must each capture a distinct value (capture
#     isolation preserved despite cell reuse)
procs = (1..5).map { |n| -> { n * 10 } }
p procs.map(&:call)                     # [10, 20, 30, 40, 50]

# (3) interleave escaping and non-escaping blocks on the same pool
acc = []
3.times do |i|
  (1..3).each { |k| acc << (i * 10 + k) }   # non-escaping (reuses cell)
end
captured = []
3.times { |i| captured << -> { i } }        # escaping (own cells)
p acc                                   # [1,2,3,11,12,13,21,22,23]
p captured.map(&:call)                  # [0, 1, 2]

# (4) block-body local must reset to nil each invocation, not carry
#     the previous iteration's value
seen = []
(1..4).each do |x|
  tmp = (x if x.even?)                  # tmp first-assigned in body
  seen << tmp.inspect
end
p seen                                  # ["nil", "2", "nil", "4"]

# (5) nested reused cells: inner each inside outer each, both
#     accumulating outer-method vars
grid = []
(1..3).each do |r|
  row = []
  (1..3).each { |c| row << r * c }
  grid << row
end
p grid                                  # [[1,2,3],[2,4,6],[3,6,9]]
