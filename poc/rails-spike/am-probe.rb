# Rails ActiveModel 7.0.10 spike — how far does rubyrs get with
# `require "active_model"` + a validating model? Rides on activesupport.
G = "/Users/linyiru/.rbenv/versions/3.4.1/lib/ruby/gems/3.4.0/gems"
%w[
  activemodel-7.0.10 activesupport-7.0.10
  i18n-1.14.7 tzinfo-2.0.6 minitest-5.25.4
  base64-0.2.0 logger-1.7.0 connection_pool-2.4.1 drb-2.2.1
].each { |g| $LOAD_PATH.unshift("#{G}/#{g}/lib") if Dir.exist?("#{G}/#{g}/lib") }
$LOAD_PATH.unshift("#{G}/concurrent-ruby-1.3.5/lib/concurrent-ruby") if Dir.exist?("#{G}/concurrent-ruby-1.3.5/lib/concurrent-ruby")

puts "== phase 1: require active_model =="
begin
  require "active_model"
  puts "OK: ActiveModel #{ActiveModel::VERSION::STRING}"
rescue Exception => e
  puts "P1-ERR: #{e.class}: #{e.message}"
  (e.backtrace || []).first(12).each { |f| puts "  #{f}" }
  exit 1
end

puts "== phase 2: define a validating model =="
begin
  class Person
    include ActiveModel::Model
    include ActiveModel::Validations

    attr_accessor :name, :age

    validates :name, presence: true
    validates :age, numericality: { greater_than: 0 }
  end
  puts "P2 OK: Person defined"
rescue Exception => e
  puts "P2-ERR: #{e.class}: #{e.message}"
  (e.backtrace || []).first(10).each { |f| puts "  #{f}" }
end

I18n.load_path << "/Users/linyiru/.rbenv/versions/3.4.1/lib/ruby/gems/3.4.0/gems/activemodel-7.0.10/lib/active_model/locale/en.yml"
I18n.backend.load_translations
puts "== phase 3: validate instances =="
begin
  good = Person.new(name: "Ada", age: 30)
  bad  = Person.new(name: "", age: -5)
  puts "good.valid? = #{good.valid?}"        # true
  puts "bad.valid?  = #{bad.valid?}"         # false
  puts "bad.errors  = #{bad.errors.full_messages.inspect}"
rescue Exception => e
  puts "P3-ERR: #{e.class}: #{e.message}"
  (e.backtrace || []).first(10).each { |f| puts "  #{f}" }
end
