# Focused pure-Ruby `Logger` (stdlib subset) — reopens the Logger
# shell the require path installs. Covers the surface real code (and
# subclasses like Jekyll's `Stevenson < Logger`) reach for: the
# `Logger::Severity` module (constants + `coerce`, mixed into Logger
# exactly like CRuby), `new(logdev, level:/progname:/formatter:)`,
# the `debug`/`info`/`warn`/`error`/`fatal`/`unknown` + `add` methods,
# the `*?` predicates, the `format_severity`/`format_message` helpers
# subclasses call, and a CRuby-shaped `Logger::LogDevice` wrapper
# (`@logdev` holds a LogDevice exposing `dev`/`filename`/`write`/
# `close`/`reopen`, matching logger 1.7's logger/log_device.rb) so
# introspectors like ActiveSupport 7.0's
# `Logger.logger_outputs_to?(logger, STDOUT)` — which reads
# `logger.instance_variable_get(:@logdev)` and calls `.dev` — and its
# LoggerThreadSafeLevel#add override — which calls `@logdev.write`
# directly — both work.
#
# Out of subset (documented divergences from logger 1.7):
# - log ROTATION: the shift_age/shift_size/shift_period_suffix args
#   are accepted at every CRuby arity (Logger.new positionals,
#   LogDevice.new/reopen kwargs) but never rotate; CRuby only rotates
#   file-backed devices, so IO-backed loggers behave identically.
# - `reraise_write_errors:`/`binmode:` accepted, ignored (CRuby also
#   accepts-and-ignores binmode on already-open IO devices).
# - no MonitorMixin on LogDevice (no `synchronize`; the `mon_*`
#   surface is absent) and no inter-process flock on rotation.
# - no `Logger#with_level` / `@level_override` (logger 1.6+): AS 7.0
#   never touches it (it ships its own `log_at` over `local_level`);
#   add it when a consumer actually needs it.
# - no `Logger::VERSION`/`ProgName` constants — deliberately, so
#   version feature-detection can't mistake this subset for the full
#   1.7 gem. The logfile creation header is written with the 3.4.8
#   oracle's ProgName string ("logger.rb/v1.7.0") for shape parity.
#
# Discovery: P3 Jekyll spike — jekyll/log_adapter.rb references
# `Logger::DEBUG` etc. and wraps a `Stevenson < Logger` writer.
# ActiveSupport 7.0's logger_thread_safe_level.rb iterates
# `Logger::Severity.constants` at require time and its test_helper
# does `include ActiveSupport::Logger::Severity`, so the constants
# must live in the mixin (shape matches logger 1.6/1.7's
# logger/severity.rb), not directly on Logger. LogDevice discovery:
# S4 — `ActiveSupport::Logger.logger_outputs_to?(logger, STDOUT)`
# returned false because the vendored @logdev was the raw IO (no
# `.dev`), found in the Logger::Severity round.

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

  # Device wrapper (logger 1.7's logger/log_device.rb subset). Holds
  # the sink in `@dev` (`attr_reader :dev` — the handle
  # `logger_outputs_to?` compares against) plus `@filename` when the
  # device owns a path it opened. Duck-typing gate matches CRuby's
  # set_dev: an object with BOTH #write and #close is used as the
  # device directly (its #path supplies `filename` when it names an
  # existing file); anything else is treated as a filename and opened
  # for append. Rotation kwargs accepted, never rotate (see header).
  class LogDevice
    attr_reader :dev
    attr_reader :filename

    def initialize(log = nil, shift_age: nil, shift_size: nil,
                   shift_period_suffix: nil, binmode: false,
                   reraise_write_errors: [], skip_header: false)
      @dev = @filename = nil
      @binmode = binmode
      @skip_header = skip_header
      set_dev(log)
    end

    # Returns the sink's write result (char count for IO); a failed
    # write warns like CRuby's handle_write_errors instead of raising.
    def write(message)
      @dev.write(message)
    rescue
      warn("log writing failed. #{$!}")
    end

    def close
      @dev.close rescue nil
    end

    # No argument: reopen the same filename (no-op for IO devices).
    # With a device/path argument: swap the sink in place — the
    # LogDevice object identity is stable across reopen, like CRuby.
    def reopen(log = nil, shift_age: nil, shift_size: nil,
               shift_period_suffix: nil, binmode: nil)
      log ||= @filename if @filename
      if log
        if @filename and @dev
          @dev.close rescue nil # close only the file opened by Logger
          @filename = nil
        end
        set_dev(log)
      end
      self
    end

    private

    def set_dev(log)
      if log.respond_to?(:write) and log.respond_to?(:close)
        @dev = log
        if log.respond_to?(:path) and path = log.path
          if File.exist?(path)
            @filename = path
          end
        end
      else
        @dev = open_logfile(log)
        @filename = log
      end
    end

    # CRuby opens WRONLY|APPEND (creating with EXCL + flock and a
    # "# Logfile created on ..." header when the file is new); the
    # subset uses append mode and writes the same-shaped header on
    # creation. `sync = true` where the runtime supports it (CRuby
    # always does; rubyrs Files flush on close).
    def open_logfile(filename)
      existed = File.exist?(filename)
      dev = File.open(filename, "a")
      dev.sync = true if dev.respond_to?(:sync=)
      unless existed || @skip_header
        dev.write("# Logfile created on #{Time.now} by logger.rb/v1.7.0\n")
      end
      dev
    end
  end

  attr_accessor :progname, :formatter
  attr_reader :level

  # Symbol/String levels coerce like CRuby (`log.level = :warn`).
  def level=(severity)
    @level = Severity.coerce(severity)
  end

  # CRuby 1.7 signature: `logdev` is a required positional; nil and
  # File::NULL mean "no device" (@logdev stays nil); anything else is
  # wrapped in a LogDevice. Rotation positionals/kwargs pass through
  # to LogDevice, which accepts-and-ignores them.
  def initialize(logdev, shift_age = 0, shift_size = 1_048_576,
                 level: DEBUG, progname: nil, formatter: nil, datetime_format: nil,
                 binmode: false, shift_period_suffix: "%Y%m%d",
                 reraise_write_errors: [], skip_header: false)
    self.level = level
    @progname = progname
    @formatter = formatter
    @datetime_format = datetime_format
    @logdev = nil
    if logdev && logdev != File::NULL
      @logdev = LogDevice.new(logdev, shift_age: shift_age,
                              shift_size: shift_size,
                              shift_period_suffix: shift_period_suffix,
                              binmode: binmode,
                              reraise_write_errors: reraise_write_errors,
                              skip_header: skip_header)
    end
  end

  def add(severity, message = nil, progname = nil)
    severity ||= UNKNOWN
    return true if @logdev.nil? || severity < level
    progname = @progname if progname.nil?
    if message.nil?
      if block_given?
        message = yield
      else
        message = progname
        progname = @progname
      end
    end
    @logdev.write(
      format_message(format_severity(severity), Time.now, progname, message))
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

  # Raw write through the device; returns the characters-written count
  # (the device's write result), or nil when there is no device —
  # exactly `@logdev&.write(msg)` like CRuby.
  def <<(msg)
    @logdev&.write(msg)
  end

  def close
    @logdev&.close
  end

  # Delegates to LogDevice#reopen; a nil @logdev stays nil (CRuby:
  # `Logger.new(nil).reopen(STDOUT)` does NOT grow a device). Returns
  # self. Rotation positionals accepted-and-ignored downstream.
  def reopen(logdev = nil, shift_age = nil, shift_size = nil,
             shift_period_suffix: nil, binmode: nil)
    @logdev&.reopen(logdev, shift_age: shift_age, shift_size: shift_size,
                    shift_period_suffix: shift_period_suffix, binmode: binmode)
    self
  end

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
