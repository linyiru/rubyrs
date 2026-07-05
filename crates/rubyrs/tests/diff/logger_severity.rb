# Logger::Severity — the mixin module CRuby's logger defines the
# severity constants in (logger/severity.rb) and mixes into Logger.
# ActiveSupport 7.0's logger_thread_safe_level.rb iterates
# `Logger::Severity.constants` + `const_get`s them at require time,
# and its test_helper does `include ActiveSupport::Logger::Severity`,
# so the module shape (constants list, private LEVELS, `coerce`) and
# the Logger mixin relationship must match CRuby exactly. Output is
# pinned to the deterministic surface only — no actual log writes
# (Logger lines carry timestamps/pids).
require "logger"

# module shape
p Logger::Severity.class
p Logger::Severity.constants.sort
p Logger::Severity::DEBUG
p Logger::Severity::INFO
p Logger::Severity::WARN
p Logger::Severity::ERROR
p Logger::Severity::FATAL
p Logger::Severity::UNKNOWN

# Logger mixes it in; the class-level constants are the module's
p Logger.include?(Logger::Severity)
p Logger.ancestors.include?(Logger::Severity)
p Logger::DEBUG == Logger::Severity::DEBUG
p Logger::UNKNOWN

# Severity.coerce — Integer passthrough, Symbol/String lookup
p Logger::Severity.coerce(3)
p Logger::Severity.coerce(:info)
p Logger::Severity.coerce("WARN")
begin
  Logger::Severity.coerce(:nope)
rescue ArgumentError => e
  puts e.message
end

# LEVELS is a private constant: hidden from the listing, raises on
# qualified access
p Logger::Severity.constants.include?(:LEVELS)
begin
  Logger::Severity::LEVELS
rescue NameError => e
  puts e.class
  puts e.message
end

# `include Logger::Severity` from user code (the AS test_helper shape)
class MyLevels
  include Logger::Severity
  def top; FATAL; end
end
p MyLevels.new.top
p MyLevels::WARN
p MyLevels.include?(Logger::Severity)

# the AS LoggerThreadSafeLevel shape: iterate + const_get
Logger::Severity.constants.sort.each do |sev|
  puts "#{sev.to_s.downcase}=#{Logger::Severity.const_get(sev.to_s.upcase)}"
end

# level= coerces Symbol / String / Integer like CRuby
log = Logger.new(nil)
p log.level
log.level = :warn
p log.level
p log.debug?
p log.info?
p log.warn?
log.level = "ERROR"
p log.level
log.level = 1
p log.level
begin
  log.level = :bogus
rescue ArgumentError => e
  puts e.message
end

# constructor level: kwarg goes through the same coercion
out = Logger.new(STDOUT, level: :warn)
p out.level
p out.debug?
p out.warn?
p out.error?
sub = Logger.new(STDOUT)
p sub.level
