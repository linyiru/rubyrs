# frozen_string_literal: true
# Minimal vendored `benchmark` stdlib. ActiveSupport requires it for
# `Benchmark.realtime` (it adds `Benchmark.ms` itself in
# active_support/core_ext/benchmark.rb). `measure`/`Tms` are provided for the
# common log-subscriber path; CPU-time fields fall back to wall time.
module Benchmark
  class Tms
    attr_reader :utime, :stime, :cutime, :cstime, :real, :label

    def initialize(utime = 0.0, stime = 0.0, cutime = 0.0, cstime = 0.0, real = 0.0, label = nil)
      @utime = utime
      @stime = stime
      @cutime = cutime
      @cstime = cstime
      @real = real
      @label = label || ""
    end

    def total
      @utime + @stime + @cutime + @cstime
    end

    def to_s
      "%10.6f %10.6f %10.6f ( %10.6f)" % [@utime, @stime, total, @real]
    end

    def format(_fmt = nil, *_args)
      to_s
    end
  end

  def self.realtime
    t0 = Process.clock_gettime(Process::CLOCK_MONOTONIC)
    yield
    Process.clock_gettime(Process::CLOCK_MONOTONIC) - t0
  end

  def self.measure(label = nil)
    t0 = Process.clock_gettime(Process::CLOCK_MONOTONIC)
    yield
    dt = Process.clock_gettime(Process::CLOCK_MONOTONIC) - t0
    Tms.new(dt, 0.0, 0.0, 0.0, dt, label)
  end
end
