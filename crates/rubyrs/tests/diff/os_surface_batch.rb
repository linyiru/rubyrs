# OS-surface preamble batch: Thread::Queue / Signal.list /
# Gem.find_files / Process / Enumerable#grep. Only the
# CRuby-identical surface is asserted here — the documented
# divergences (Queue#pop blocking on empty, Etc.nprocessors'
# value) stay out of the diff.

q = Thread::Queue.new
q << 1
q.push(2)
p q.pop
p q.pop
p q.size
p q.empty?
p q.closed?
q.close
p q.closed?
p Queue == Thread::Queue

p Signal.list["INT"]
p Signal.list["TERM"]
p Signal.list["KILL"]
p Signal.list.key?("HUP")

# The diff harness runs CRuby with rubygems disabled, so guard:
# both sides print [] (rubyrs always defines the Gem shell).
p(defined?(Gem) ? Gem.find_files("zz_no_such_dir/*_plugin.rb") : [])

p Process.pid == $$
p Process.respond_to?(:clock_gettime)
p Process.clock_gettime(Process::CLOCK_MONOTONIC).is_a?(Float)
p Process.clock_gettime(Process::CLOCK_MONOTONIC, :millisecond).is_a?(Integer)
begin
  Process.clock_gettime(Process::CLOCK_MONOTONIC, :bogus)
rescue ArgumentError => e
  p e.message
end

p [1, "a", 2, :b, 3].grep(Integer)
p (1..5).grep(2..4) { |x| x * 10 }
p %w[ant bee cat].grep(/t/)
p [1, "a", 2].grep_v(Integer)
p [1, "a", 2].grep_v(Integer) { |x| x.to_s * 2 }
