# timeout — vendored subset (ADR 0026). Real Timeout preempts via a
# watchdog thread; the single-threaded model can't interrupt, so
# `Timeout.timeout` runs the block WITHOUT enforcement (documented
# divergence — a block that overruns simply isn't cut short). The
# constants exist so `rescue Timeout::Error` references resolve.
#
# Motivating consumer: rack's test helper chain requires "timeout".
module Timeout
  class Error < RuntimeError
  end

  def self.timeout(_sec, _klass = nil, _message = nil)
    yield(_sec)
  end
end

# CRuby exposes the top-level alias.
TimeoutError = Timeout::Error
def timeout(sec, klass = nil, message = nil, &block)
  Timeout.timeout(sec, klass, message, &block)
end
