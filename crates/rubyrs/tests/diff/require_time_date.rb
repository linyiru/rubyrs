# CRuby's lib/time.rb does `require 'date'` internally, so
# `require "time"` makes Date / DateTime resolvable. Discovery: P3
# Jekyll spike — safe_yaml/parse/date.rb does `require 'time'` then
# references bare `DateTime`.
require "time"
p defined?(Date)
p defined?(DateTime)
p defined?(Time)
# resolvable as a bare constant from a nested scope too
module Wrapper
  class Inner
    p defined?(DateTime)
  end
end
