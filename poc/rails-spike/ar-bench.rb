G = "/Users/linyiru/.rbenv/versions/3.4.1/lib/ruby/gems/3.4.0/gems"
%w[activerecord-7.0.10 activemodel-7.0.10 activesupport-7.0.10 i18n-1.14.7 tzinfo-2.0.6 minitest-5.25.4 base64-0.2.0 logger-1.7.0 connection_pool-2.4.1 drb-2.2.1 timeout-0.4.1].each { |g| $LOAD_PATH.unshift("#{G}/#{g}/lib") if Dir.exist?("#{G}/#{g}/lib") }
$LOAD_PATH.unshift("#{G}/concurrent-ruby-1.3.5/lib/concurrent-ruby")
require "active_record"
ActiveRecord::Base.logger = nil
ActiveRecord::Base.establish_connection(adapter: "sqlite3", database: ":memory:")
ActiveRecord::Schema.verbose = false
ActiveRecord::Schema.define do
  create_table :users, force: true do |t|
    t.string :name; t.integer :age; t.string :email
  end
end
class User < ActiveRecord::Base; end
def now; Process.clock_gettime(Process::CLOCK_MONOTONIC); end
N = (ENV["N"] || 2000).to_i
User.create(name: "w", age: 1, email: "w@x"); User.delete_all   # warm

t = now; N.times { |i| User.create(name: "user#{i}", age: i % 100, email: "u#{i}@example.com") }; ins = now - t
t = now; q = 0; 500.times { q += User.where(age: 50).to_a.size }; qry = now - t
t = now; 500.times { |i| u = User.find_by(name: "user#{i}"); u.update(age: u.age + 1) }; upd = now - t
t = now; c = 0; 1000.times { c += User.count }; cnt = now - t

puts "inserts:     #{N} rows   #{(ins*1000).round}ms   (#{(N/ins).round}/s)"
puts "where.to_a:  500x        #{(qry*1000).round}ms"
puts "find+update: 500x        #{(upd*1000).round}ms"
puts "count:       1000x       #{(cnt*1000).round}ms"
puts "TOTAL:                   #{((ins+qry+upd+cnt)*1000).round}ms"
