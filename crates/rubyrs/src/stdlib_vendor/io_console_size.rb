# io/console/size — vendored subset (ADR 0026 blessed-reimpl). The
# real file tries `IO.console.winsize` (the io-console C extension)
# and falls back to $LINES/$COLUMNS; rubyrs has no winsize, so the
# fallback IS the implementation — same as CRuby when io/console
# fails to load.
#
# Motivating consumer: actionpack's routing/inspector.rb
# (`require "io/console/size"` at load; `IO.console_size[1]` sizes
# the `bin/rails routes` table), loaded on every Rails boot via
# ActionDispatch::DebugExceptions.
def IO.default_console_size
  lines = ENV["LINES"].to_i
  columns = ENV["COLUMNS"].to_i
  [
    lines.positive? ? lines : 25,
    columns.positive? ? columns : 80,
  ]
end

def IO.console_size
  default_console_size
end
