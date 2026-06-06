# Focused pure-Ruby `Logger` (stdlib subset) — reopens the Logger
# shell the require path installs. Covers the surface real code (and
# subclasses like Jekyll's `Stevenson < Logger`) reach for: the
# severity-level constants, `new(logdev, level:/progname:/formatter:)`,
# the `debug`/`info`/`warn`/`error`/`fatal`/`unknown` + `add` methods,
# the `*?` predicates, and the `format_severity`/`format_message`
# helpers subclasses call. Not the full stdlib Logger (no log
# rotation / LogDevice / reopen).
#
# Discovery: P3 Jekyll spike — jekyll/log_adapter.rb references
# `Logger::DEBUG` etc. and wraps a `Stevenson < Logger` writer.

class Logger
  DEBUG   = 0
  INFO    = 1
  WARN    = 2
  ERROR   = 3
  FATAL   = 4
  UNKNOWN = 5

  SEV_LABEL = %w[DEBUG INFO WARN ERROR FATAL ANY].freeze

  attr_accessor :level, :progname, :formatter

  def initialize(logdev = nil, _shift_age = 0, _shift_size = 1_048_576,
                 level: DEBUG, progname: nil, formatter: nil, datetime_format: nil, **_opts)
    @logdev = logdev
    @level = level
    @progname = progname
    @formatter = formatter
    @datetime_format = datetime_format
  end

  def add(severity, message = nil, progname = nil)
    severity ||= UNKNOWN
    return true if @logdev.nil? || severity < @level
    progname ||= @progname
    if message.nil?
      if block_given?
        message = yield
      else
        message = progname
        progname = @progname
      end
    end
    line = format_message(format_severity(severity), Time.now, progname, message)
    if @logdev.respond_to?(:puts)
      @logdev.puts(line)
    elsif @logdev.respond_to?(:write)
      @logdev.write(line.end_with?("\n") ? line : "#{line}\n")
    end
    true
  end
  alias_method :log, :add

  def debug(progname = nil, &block);   add(DEBUG, nil, progname, &block);   end
  def info(progname = nil, &block);    add(INFO, nil, progname, &block);    end
  def warn(progname = nil, &block);    add(WARN, nil, progname, &block);    end
  def error(progname = nil, &block);   add(ERROR, nil, progname, &block);   end
  def fatal(progname = nil, &block);   add(FATAL, nil, progname, &block);   end
  def unknown(progname = nil, &block); add(UNKNOWN, nil, progname, &block); end

  def debug?;   @level <= DEBUG;   end
  def info?;    @level <= INFO;    end
  def warn?;    @level <= WARN;    end
  def error?;   @level <= ERROR;   end
  def fatal?;   @level <= FATAL;   end

  def <<(msg)
    if @logdev.respond_to?(:write)
      @logdev.write(msg)
    elsif @logdev.respond_to?(:puts)
      @logdev.puts(msg)
    end
    msg.to_s.length
  end

  def close; end
  def reopen(logdev = nil); @logdev = logdev if logdev; self; end

  def format_severity(severity)
    SEV_LABEL[severity] || "ANY"
  end

  def format_message(severity, time, progname, msg)
    if @formatter
      @formatter.call(severity, time, progname, msg)
    else
      "#{severity[0]}, [#{time}] #{severity} -- #{progname}: #{msg}\n"
    end
  end
end
