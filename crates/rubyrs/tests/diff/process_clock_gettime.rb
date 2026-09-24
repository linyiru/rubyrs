# Process.clock_gettime — result type per unit, monotonic ordering,
# and the error surface for a bad clock id / unit. Values themselves
# are host-dependent, so only shapes and relations are printed.
UNITS = %i[float_second float_millisecond float_microsecond
           second millisecond microsecond nanosecond].freeze

UNITS.each do |u|
  p [u, Process.clock_gettime(Process::CLOCK_MONOTONIC, u).class]
end
p Process.clock_gettime(Process::CLOCK_MONOTONIC).class
p Process.clock_gettime(Process::CLOCK_REALTIME).class

a = Process.clock_gettime(Process::CLOCK_MONOTONIC, :nanosecond)
b = Process.clock_gettime(Process::CLOCK_MONOTONIC, :nanosecond)
p b >= a

# The realtime clock agrees with Time.now to well within a second.
p (Process.clock_gettime(Process::CLOCK_REALTIME) - Time.now.to_f).abs < 1.0
p (Process.clock_gettime(Process::CLOCK_REALTIME, :second) - Time.now.to_i).abs <= 1

# Unit conversions of one reading are mutually consistent.
s = Process.clock_gettime(Process::CLOCK_REALTIME, :second)
ms = Process.clock_gettime(Process::CLOCK_REALTIME, :millisecond)
p (ms / 1000 - s).abs <= 1

[
  [Process::CLOCK_MONOTONIC, :furlong],
  [Process::CLOCK_MONOTONIC, "second"],
  [9999],
  ["x"],
].each do |args|
  begin
    Process.clock_gettime(*args)
    p :no_error
  rescue SystemCallError, ArgumentError, TypeError => e
    p [e.class.ancestors.include?(SystemCallError) ? SystemCallError : e.class,
       e.message.sub(/\(\d+\)\z/, "")]
  end
end
