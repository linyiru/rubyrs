G = "/Users/linyiru/.rbenv/versions/3.4.1/lib/ruby/gems/3.4.0/gems"
%w[activemodel-7.0.10 activesupport-7.0.10 i18n-1.14.7 tzinfo-2.0.6 minitest-5.25.4 base64-0.2.0 logger-1.7.0 connection_pool-2.4.1 drb-2.2.1].each { |g| $LOAD_PATH.unshift("#{G}/#{g}/lib") if Dir.exist?("#{G}/#{g}/lib") }
$LOAD_PATH.unshift("#{G}/concurrent-ruby-1.3.5/lib/concurrent-ruby")

def now = Process.clock_gettime(Process::CLOCK_MONOTONIC)

t0 = now
require "active_model"
I18n.load_path << "#{G}/activemodel-7.0.10/lib/active_model/locale/en.yml"
I18n.backend.load_translations

class Person
  include ActiveModel::Model
  include ActiveModel::Validations
  attr_accessor :name, :email, :age
  validates :name, presence: true
  validates :age, numericality: { greater_than: 0, less_than: 200 }
  validates :email, format: { with: /\A[^@\s]+@[^@\s]+\z/ }
end
t_boot = now - t0

def run(n)
  ok = 0
  n.times do |i|
    good = i.even?
    p = Person.new(name: good ? "Ada" : "", age: good ? 30 : -1, email: good ? "a@b.com" : "bad")
    if p.valid? then ok += 1 else p.errors.full_messages end
  end
  ok
end

run(1000) # warmup
N = 20_000
t1 = now
run(N)
elapsed = now - t1

puts "boot_ms=#{(t_boot * 1000).round(1)}  valid_per_sec=#{(N / elapsed).round}  loop_ms=#{(elapsed * 1000).round}"
