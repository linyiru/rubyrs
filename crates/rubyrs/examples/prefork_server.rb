# rubyrs `_http_server` battery — pre-fork multi-core example.
#
# Run with:
#   cargo run --features _http_server -p rubyrs -- \
#     crates/rubyrs/examples/prefork_server.rb
#
# Then in another terminal:
#   curl 127.0.0.1:9292/
#   wrk -t4 -c100 -d10s http://127.0.0.1:9292/
#
# Per ADR 0022 v3 §"Multi-core scaling" + FU3:
# - Linux 3.9+: production target. Kernel hash-distributes
#   connections across N SO_REUSEPORT sockets via 4-tuple hash.
#   Pin N_WORKERS = vCPU count.
# - macOS: dev-only. Workers fork cleanly + boot + serve, but
#   Darwin has NO SO_REUSEPORT_LB (verified against libc 0.2.169
#   `unix/bsd/apple/mod.rs`). The kernel typically routes new
#   connections to the most-recent listener — load distribution
#   is OS-discretion. CoreFoundation/dispatch are officially
#   fork-unsafe; production should be Linux.
# - FreeBSD: SO_REUSEPORT_LB exists but isn't wired here yet
#   (PoC TODO). Falls back to permissive SO_REUSEPORT.
# - Windows: unsupported. N>=2 returns ArgumentError.
#
# Vm state across fork (per ADR 0022 v3 "Inherited state"):
# - Class definitions, method tables, constants, host fn closures
#   inherited via COW — per-process modifications don't propagate
#   to siblings.
# - File descriptors opened pre-fork ARE shared kernel FDs. DB
#   connections, logfile handles etc. MUST be closed and reopened
#   in on_worker_boot — same discipline as Puma's on_worker_boot.
# - Arc<Mutex<...>> captured in host fn closures looks shared but
#   isn't post-fork; mutex state is per-process. Cross-worker
#   synchronisation needs an external mechanism (Redis, etc.).

# Worker-local state. Each child sees its own copy after fork.
class WorkerState
  @booted_at = nil
  @request_count = 0

  def self.boot!(idx)
    @worker_index = idx
    @booted_at = Time.now
    # In a real app, this is where you'd close any
    # pre-fork-inherited DB connections and reopen them
    # fresh for this child's address space.
    puts "[worker #{idx}] booted at #{@booted_at}"
  end

  def self.worker_index; @worker_index; end
  def self.booted_at; @booted_at; end
  def self.bump; @request_count += 1; end
  def self.request_count; @request_count; end
end

on_worker_boot = ->(idx) { WorkerState.boot!(idx) }

app = ->(env) {
  WorkerState.bump
  body = "worker=#{WorkerState.worker_index} " \
         "served_by_this_process=#{WorkerState.request_count} " \
         "method=#{env['REQUEST_METHOD']} " \
         "path=#{env['PATH_INFO']}\n"
  [200, {"Content-Type" => "text/plain"}, [body]]
}

# Bind on 127.0.0.1:9292, run for 600 seconds (10 minutes),
# fork 4 workers. Kill with Ctrl+C — the SIGINT goes to the
# whole process group; each child handles it via its serve
# loop default + the parent's waitpid completes naturally.
PORT = (ENV["PORT"] || "9292").to_i
N_WORKERS = (ENV["N_WORKERS"] || "4").to_i
DURATION = (ENV["DURATION_SECS"] || "600").to_i

puts "starting prefork server: 127.0.0.1:#{PORT}, #{N_WORKERS} workers, #{DURATION}s"
__rubyrs_http_serve_prefork(
  "127.0.0.1:#{PORT}",
  DURATION,
  app,
  N_WORKERS,
  on_worker_boot,
)
puts "all workers exited cleanly"
