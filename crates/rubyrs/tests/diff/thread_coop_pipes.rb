# Cooperative scheduler x fork(2) x pipe(2): the parallel gem's
# work_in_processes protocol in miniature. N forked workers are fed
# jobs CONCURRENTLY by N green supervisor threads — each supervisor
# loops { pop job index under a Mutex; Marshal frame to its worker's
# pipe; blocking pipe read of the result } — the exact shape rubocop
# --parallel runs. Needs `--features _fiber` (fiber-backed threads)
# and the process-spawn capability (CLI default).
#
# Everything printed is scheduling-order-INDEPENDENT (results keyed by
# job index; per-worker counts are inherently racy and only their sum
# is asserted) so preemptive CRuby prints identical bytes.

# Shared state via objects (ivars / element stores) — the same
# discipline the parallel gem uses (JobFactory's @index under @mutex).
class JobCounter
  def initialize(n)
    @i = -1
    @n = n
    @m = Mutex.new
  end

  def next_index
    i = @m.synchronize { @i += 1 }
    i < @n ? i : nil
  end
end

NWORKERS = 3
NJOBS = 9

jobs = JobCounter.new(NJOBS)
results = Array.new(NJOBS)
counts = Array.new(NWORKERS, 0)

workers = []
w = 0
while w < NWORKERS
  child_read, parent_write = IO.pipe
  parent_read, child_write = IO.pipe
  pid = Process.fork do
    # Child: single-threaded worker loop (the scheduler world reset by
    # the fork wrapper — a child must never resume parent threads).
    parent_write.close
    parent_read.close
    until child_read.eof?
      job = Marshal.load(child_read)
      Marshal.dump([job, job * 10], child_write)
    end
    child_read.close
    child_write.close
  end
  child_read.close
  child_write.close
  workers << [parent_read, parent_write, pid]
  w += 1
end

threads = []
w = 0
while w < NWORKERS
  # NOTE: the block locals are deliberately named apart from the fork
  # loop's `pid`/`child_*` locals above — same-named assignments inside
  # the block would CAPTURE and share the outer variable across all
  # three supervisors (a plain closure-scoping race on preemptive
  # CRuby, nothing thread-model-specific).
  threads << Thread.new(w) do |wi|
    sup_rd, sup_wr, sup_pid = workers[wi]
    loop do
      idx = jobs.next_index
      break unless idx
      Marshal.dump(idx, sup_wr)
      _job, tenfold = Marshal.load(sup_rd)
      results[idx] = tenfold
      counts[wi] += 1
    end
    sup_wr.close
    sup_rd.close
    Process.wait(sup_pid)
  end
  w += 1
end
threads.each(&:join)

p results
p counts.sum
p counts.all? { |c| c >= 1 } # dynamic distribution reached every worker

# Exit statuses were collected per worker (Process.wait in each
# supervisor); a fresh wait has no child left.
begin
  Process.wait(-1)
rescue Errno::ECHILD => e
  p e.class.name
end
