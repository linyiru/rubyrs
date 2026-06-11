# monitor — vendored subset (ADR 0026 blessed-reimpl). MonitorMixin
# is the reentrant cousin of Mutex; in the single-threaded model
# every synchronize degenerates to yield (same shape as the
# preamble Mutex — see preamble/mutex.rb for the rationale).
#
# Motivating consumer: logger 1.7 (`LogDevice` includes
# MonitorMixin and calls mon_initialize/synchronize per write),
# reached by rack's CommonLogger on the Sinatra/rack spike paths.
module MonitorMixin
  def mon_initialize
    self
  end

  def mon_synchronize
    yield
  end
  alias synchronize mon_synchronize

  def mon_enter
    nil
  end

  def mon_exit
    nil
  end

  def mon_try_enter
    true
  end

  def new_cond
    ConditionVariable.new
  end
end

class Monitor
  include MonitorMixin

  def initialize
    mon_initialize
  end

  def enter
    mon_enter
  end

  def exit
    mon_exit
  end

  def try_enter
    mon_try_enter
  end
end
