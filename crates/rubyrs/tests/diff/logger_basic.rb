# Logger subset: severity constants, level get/set, the `*?`
# predicates, and that add/debug/info don't crash with a nil logdev.
# (Output FORMAT differs across implementations, so we use a nil
# logdev and assert only the level logic — parity-safe.) Discovery:
# P3 Jekyll spike — jekyll's Stevenson < Logger.
require "logger"

p Logger::DEBUG
p Logger::INFO
p Logger::WARN
p Logger::ERROR
p Logger::FATAL

log = Logger.new(nil)
log.level = Logger::WARN
p log.level
p log.debug?
p log.info?
p log.warn?
p log.error?
# logging to a nil device is a no-op, returns true
p log.info("ignored")
p log.warn("ignored")

# level via integer
log.level = 1
p log.info?
p log.debug?

# a subclass (like jekyll's Stevenson) can call super in initialize
class MyLog < Logger
  def initialize
    super(nil, level: Logger::ERROR)
  end
end
ml = MyLog.new
p ml.level
p ml.error?
p ml.warn?
