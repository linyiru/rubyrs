# Focused pure-Ruby `Logger` (stdlib subset) — reopens the Logger
# shell the require path installs. Covers the surface real code (and
# subclasses like Jekyll's `Stevenson < Logger`) reach for: the
# `Logger::Severity` module (constants + `coerce`, mixed into Logger
# exactly like CRuby), `new(logdev, level:/progname:/formatter:)`,
# the `debug`/`info`/`warn`/`error`/`fatal`/`unknown` + `add` methods,
# the `*?` predicates, and the `format_severity`/`format_message`
# helpers subclasses call. Not the full stdlib Logger (no log
# rotation / LogDevice / reopen).
#
# Discovery: P3 Jekyll spike — jekyll/log_adapter.rb references
# `Logger::DEBUG` etc. and wraps a `Stevenson < Logger` writer.
# ActiveSupport 7.0's logger_thread_safe_level.rb iterates
# `Logger::Severity.constants` at require time and its test_helper
# does `include ActiveSupport::Logger::Severity`, so the constants
# must live in the mixin (shape matches logger 1.6/1.7's
# logger/severity.rb), not directly on Logger.

class Logger
  # Logging severity.
  module Severity
    DEBUG   = 0
    INFO    = 1
    WARN    = 2
    ERROR   = 3
    FATAL   = 4
    UNKNOWN = 5

    LEVELS = {
      "debug" => DEBUG,
      "info" => INFO,
      "warn" => WARN,
      "error" => ERROR,
      "fatal" => FATAL,
      "unknown" => UNKNOWN,
    }
    private_constant :LEVELS

    def self.coerce(severity)
      if severity.is_a?(Integer)
        severity
      else
        key = severity.to_s.downcase
        LEVELS[key] || raise(ArgumentError, "invalid log level: #{severity}")
      end
    end
  end
  include Severity

  SEV_LABEL = %w[DEBUG INFO WARN ERROR FATAL ANY].freeze

  # Default log-line formatter (logger/formatter.rb). ActiveSupport
  # 7.0's `SimpleFormatter < ::Logger::Formatter` subclasses it at
  # require time (active_support/logger.rb:86) and overrides #call,
  # so the base class + accessor must exist; the base #call itself
  # only fires for direct Formatter users.
  class Formatter
    Format = "%s, [%s #%d] %5s -- %s: %s\n"
    DatetimeFormat = "%Y-%m-%dT%H:%M:%S.%6N"

    attr_accessor :datetime_format

    def initialize
      @datetime_format = nil
    end

    def call(severity, time, progname, msg)
      format(Format, severity[0..0], format_datetime(time), Process.pid, severity, progname,
             msg2str(msg))
    end

    private

    def format_datetime(time)
      time.strftime(@datetime_format || DatetimeFormat)
    end

    def msg2str(msg)
      case msg
      when ::String
        msg
      when ::Exception
        "#{msg.message} (#{msg.class})\n" + (msg.backtrace || []).join("\n")
      else
        msg.inspect
      end
    end
  end

  attr_accessor :progname, :formatter
  attr_reader :level

  # Symbol/String levels coerce like CRuby (`log.level = :warn`).
  def level=(severity)
    @level = Severity.coerce(severity)
  end

  def initialize(logdev = nil, _shift_age = 0, _shift_size = 1_048_576,
                 level: DEBUG, progname: nil, formatter: nil, datetime_format: nil, **_opts)
    @logdev = logdev
    self.level = level
    @progname = progname
    @formatter = formatter
    @datetime_format = datetime_format
  end

  def add(severity, message = nil, progname = nil)
    severity ||= UNKNOWN
    return true if @logdev.nil? || severity < level
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

  # Against `level` (the reader), not `@level`, so subclasses that
  # override #level (ActiveSupport's thread-local level) are honoured
  # — CRuby's predicates read the same way.
  def debug?;   level <= DEBUG;   end
  def info?;    level <= INFO;    end
  def warn?;    level <= WARN;    end
  def error?;   level <= ERROR;   end
  def fatal?;   level <= FATAL;   end

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
