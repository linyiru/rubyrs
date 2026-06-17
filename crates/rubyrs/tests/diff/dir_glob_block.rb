# Dir.glob(pat) { |f| ... } block form — yields each match, returns nil.
# (net-smtp loads adapters via `Dir.glob(...) { |r| require_relative r }`.)
# Dir[pat] ignores a block (returns the Array).
base = "/tmp/rubyrs_dirglob_fixture"
Dir.mkdir(base) unless Dir.exist?(base)
%w[a.txt b.txt c.log].each { |f| File.write("#{base}/#{f}", "") }

acc = []
ret = Dir.glob("#{base}/*.txt") { |f| acc << File.basename(f) }
p acc.sort                                  # ["a.txt", "b.txt"]
p ret                                       # nil

# break propagates its value
p(Dir.glob("#{base}/*") { |_f| break :stop })  # :stop

# Dir[] ignores the block, returns the Array
arr = Dir["#{base}/*.log"] { |_f| raise "should not yield" }
p arr.map { |f| File.basename(f) }          # ["c.log"]

# cleanup (also exercises the block form for deletion)
Dir.glob("#{base}/*") { |f| File.delete(f) }
Dir.rmdir(base)
p Dir.exist?(base)                          # false
