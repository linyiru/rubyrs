# `require 'logger'` stub. base.rb declares `::Logger` as the
# default for the `debug` / `warn` helpers; our middlewares only
# reach those when an attack is detected AND logging is enabled,
# neither of which fire on the scenarios this fixture exercises.
class Logger
  def initialize(_); end
  def debug(_); end
  def warn(_); end
end
