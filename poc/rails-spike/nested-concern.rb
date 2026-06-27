G = "/Users/linyiru/.rbenv/versions/3.4.1/lib/ruby/gems/3.4.0/gems"
%w[activesupport-7.0.10 i18n-1.14.7 tzinfo-2.0.6 minitest-5.25.4 base64-0.2.0 logger-1.7.0 connection_pool-2.4.1 drb-2.2.1].each { |g| $LOAD_PATH.unshift("#{G}/#{g}/lib") if Dir.exist?("#{G}/#{g}/lib") }
$LOAD_PATH.unshift("#{G}/concurrent-ruby-1.3.5/lib/concurrent-ruby")
require "active_support/concern"
module A
  extend ActiveSupport::Concern
  class_methods do; def a_cm = "a-classmethod"; end
end
module B
  extend ActiveSupport::Concern
  include A
  class_methods do; def b_cm = "b-classmethod"; end
end
class C; include B; end
r1 = (C.a_cm rescue "NO a_cm")
r2 = (C.b_cm rescue "NO b_cm")
puts "C.a_cm => #{r1}"
puts "C.b_cm => #{r2}"
