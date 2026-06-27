G = "/Users/linyiru/.rbenv/versions/3.4.1/lib/ruby/gems/3.4.0/gems"
%w[activemodel-7.0.10 activesupport-7.0.10 i18n-1.14.7 tzinfo-2.0.6 minitest-5.25.4 base64-0.2.0 logger-1.7.0 connection_pool-2.4.1 drb-2.2.1].each { |g| $LOAD_PATH.unshift("#{G}/#{g}/lib") if Dir.exist?("#{G}/#{g}/lib") }
$LOAD_PATH.unshift("#{G}/concurrent-ruby-1.3.5/lib/concurrent-ruby")
require "active_model"
class Person
  include ActiveModel::Model
  include ActiveModel::Validations
  attr_accessor :name, :age, :email
  validates :name, presence: true
  validates :age, numericality: { greater_than: 0, less_than: 200 }
  validates :email, format: { with: /\A[^@\s]+@[^@\s]+\z/ }
end
# Pure validation loop (valid instances → no i18n message path; isolates the
# dispatch/validation machinery).
i = 0
while i < 40_000
  Person.new(name: "Ada", age: 30, email: "a@b.com").valid?
  i += 1
end
