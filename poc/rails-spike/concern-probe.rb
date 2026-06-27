G = "/Users/linyiru/.rbenv/versions/3.4.1/lib/ruby/gems/3.4.0/gems"
%w[activesupport-7.0.10 i18n-1.14.7 tzinfo-2.0.6 minitest-5.25.4 base64-0.2.0 logger-1.7.0 connection_pool-2.4.1 drb-2.2.1].each { |g| $LOAD_PATH.unshift("#{G}/#{g}/lib") if Dir.exist?("#{G}/#{g}/lib") }
$LOAD_PATH.unshift("#{G}/concurrent-ruby-1.3.5/lib/concurrent-ruby")
require "active_support/concern"
module Greet
  extend ActiveSupport::Concern
  included do
    puts "  included block ran on #{self}"
  end
  class_methods do
    def greeting = "class-method greeting"
  end
  def instance_greet = "instance greeting"
end
class C; include Greet; end
puts "C.greeting        => #{C.greeting}"
puts "C.new.instance_greet => #{C.new.instance_greet}"
puts "C.ancestors incl Greet? => #{C.ancestors.include?(Greet)}"
