# Logger::LogDevice — the wrapper CRuby's logger stores in `@logdev`
# (logger/log_device.rb). ActiveSupport 7.0's
# `Logger.logger_outputs_to?(logger, STDOUT)` reads
# `logger.instance_variable_get(:@logdev)` and calls `.dev` on it, and
# AS's LoggerThreadSafeLevel#add writes via `@logdev.write` directly,
# so the wrapper's shape (class, dev identity, filename, the
# write/close/reopen trio) must match CRuby. Pins only the
# deterministic surface: log LINES carry timestamps/pids, so sink
# content is asserted by shape (prefix/suffix/line count), never
# byte-wise. Exercises the 1.6∩1.7 common surface — the
# `--disable=gems` oracle serves ruby/3.4.x's stdlib logger copy
# (1.6 shape: no `skip_header:`, `reopen(logdev)`-only arity), while
# the vendored subset carries the 1.7-gem superset. Deliberately NO
# StringIO: this fixture must stay green on the BARE build, where
# stdlib requires resolve to constant shells (see stdlib_require_stub)
# whose instances answer respond_to?(:write) => false — duck sinks
# walk the identical set_dev write+close branch. Discovery: S4 —
# logger_outputs_to? returned false on the vendored path (@logdev was
# the raw IO, no `.dev`), found in the Logger::Severity round.
require "logger"

# --- IO-backed LogDevice shape
l = Logger.new(STDOUT)
d = l.instance_variable_get(:@logdev)
p d.class
p d.dev.equal?(STDOUT)
p d.filename
p d.respond_to?(:dev)
p d.respond_to?(:filename)
p d.respond_to?(:write)
p d.respond_to?(:close)
p d.respond_to?(:reopen)

# --- the exact AS 7.0.10 logger_outputs_to? body
# (activesupport-7.0.10 active_support/logger.rb:16-20, verbatim)
def logger_outputs_to?(logger, *sources)
  logdev = logger.instance_variable_get(:@logdev)
  logger_source = logdev.dev if logdev.respond_to?(:dev)
  sources.any? { |source| source == logger_source }
end
p logger_outputs_to?(l, STDOUT)
p logger_outputs_to?(l, STDERR)
p logger_outputs_to?(l, STDOUT, STDERR)
p logger_outputs_to?(Logger.new(STDERR), STDOUT)
p logger_outputs_to?(Logger.new(nil), STDOUT)

# --- duck device (CRuby set_dev gate: BOTH #write and #close
# required). IOError on double-close mirrors an IO sink, so the
# close-swallow pin below exercises LogDevice#close's `rescue nil`.
class Sink
  attr_reader :written
  def initialize; @written = []; @closed = false; end
  def write(s); @written << s; s.length; end
  def close
    raise IOError, "closed stream" if @closed
    @closed = true
  end
  def closed?; @closed; end
  def string; @written.join; end
end
sink = Sink.new
dl = Logger.new(sink)
dd = dl.instance_variable_get(:@logdev)
p dd.class
p dd.dev.equal?(sink)
p dd.filename
p(dl << "raw")                      # Logger#<< returns the device write count
dl.info("hello")
p sink.string.start_with?("raw")
p sink.string.end_with?("hello\n")
p dl.add(Logger::ERROR, "boom")     # true; the formatted line hits the sink
p sink.written.size
p sink.written.last.end_with?("boom\n")
dl.close
p sink.closed?

# --- file-path logger: LogDevice opens the file, filename adopted
path = "/tmp/rubyrs_diff_logdev.log"
File.delete(path) if File.exist?(path)
fl = Logger.new(path)
fd = fl.instance_variable_get(:@logdev)
p fd.class
p fd.filename == path
p fd.dev.class
p fd.dev.path == path
p logger_outputs_to?(fl, STDOUT)
fl.info("to file")
fl.close
lines = File.readlines(path)
p lines.size                        # creation header + one entry
p lines.first.start_with?("# Logfile created on ")
p lines.last.end_with?("to file\n")

# --- Logger#reopen with no args: SAME LogDevice object reopens the
# same filename; an existing file gets no second header
fl.reopen
p fl.instance_variable_get(:@logdev).equal?(fd)
p fd.filename == path
fl.info("again")
fl.close
p File.readlines(path).size
File.delete(path)

# --- rotation args: accepted at CRuby's Logger.new arity, and for an
# IO device they are ignored on BOTH sides (CRuby only rotates
# file-backed devices)
rot = Logger.new(STDOUT, 10, 2048)
p rot.instance_variable_get(:@logdev).filename
rot2 = Logger.new(STDOUT, "daily")
p rot2.instance_variable_get(:@logdev).class
path2 = "/tmp/rubyrs_diff_logdev_rot.log"
File.delete(path2) if File.exist?(path2)
rp = Logger.new(path2, 3, 1024, shift_period_suffix: "%Y", binmode: false)
p rp.instance_variable_get(:@logdev).filename == path2
rp.close
File.delete(path2)

# --- nil / File::NULL: no device at all
p Logger.new(nil).instance_variable_get(:@logdev)
p Logger.new(File::NULL).instance_variable_get(:@logdev)
n = Logger.new(nil)
p n.reopen(STDOUT).equal?(n)
p n.instance_variable_get(:@logdev)  # still nil — reopen never grows a device
p(n << "x")                          # nil (no device)
p n.info("dropped")                  # true (no-op)
n.close
puts "nil-close-ok"

# --- Logger#reopen(io): swaps the sink INSIDE the same LogDevice; the
# old user-supplied sink is NOT closed (CRuby closes only files it
# opened itself)
sink2 = Sink.new
rl = Logger.new(sink2)
rld = rl.instance_variable_get(:@logdev)
p rl.reopen(STDERR).equal?(rl)
p rl.instance_variable_get(:@logdev).equal?(rld)
p rld.dev.equal?(STDERR)
p rld.filename
p sink2.closed?
p rld.reopen(sink2).equal?(rld)      # LogDevice#reopen returns self
p rld.dev.equal?(sink2)

# --- close swallows sink close errors (already-closed device raises
# IOError from Sink#close; LogDevice#close's `rescue nil` eats it)
sink2.close
begin
  rl.close
  puts "close-swallowed"
rescue => e
  p [:raised, e.class]
end

# --- user-opened File as the device: dev identity kept AND filename
# adopted from #path; Logger writes no creation header (it didn't
# create the file)
path3 = "/tmp/rubyrs_diff_logdev_userfile.log"
File.delete(path3) if File.exist?(path3)
uf = File.open(path3, "a")
p File.exist?(path3)                 # append-open CREATES at open (open(2))
ul = Logger.new(uf)
ud = ul.instance_variable_get(:@logdev)
p ud.dev.equal?(uf)
p ud.filename == path3
ul.close
p File.read(path3).empty?
File.delete(path3)

# --- LogDevice direct construction
ld = Logger::LogDevice.new(STDOUT)
p ld.dev.equal?(STDOUT)
p ld.filename
