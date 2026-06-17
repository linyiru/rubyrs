# `Monitor` is available without an explicit `require "monitor"` in a
# full Ruby environment (rubygems pre-loads it); gems lean on that.
# dotenv references `Monitor.new` at module-load time without requiring
# it. (Oracle uses gem-enabled CRuby; `--disable=gems` lacks Monitor.)
m = Monitor.new
p m.synchronize { 1 + 1 }                # 2
p m.synchronize { :ok }                  # :ok
# MonitorMixin used as a module
class Resource
  include MonitorMixin
  def initialize; super; @n = 0; end
  def bump; synchronize { @n += 1 }; end
  def n; @n; end
end
r = Resource.new
r.bump; r.bump
p r.n                                    # 2
