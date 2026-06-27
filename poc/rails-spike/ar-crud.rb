G = "/Users/linyiru/.rbenv/versions/3.4.1/lib/ruby/gems/3.4.0/gems"
%w[activerecord-7.0.10 activemodel-7.0.10 activesupport-7.0.10 i18n-1.14.7 tzinfo-2.0.6 minitest-5.25.4 base64-0.2.0 logger-1.7.0 connection_pool-2.4.1 drb-2.2.1 timeout-0.4.1].each { |g| $LOAD_PATH.unshift("#{G}/#{g}/lib") if Dir.exist?("#{G}/#{g}/lib") }
$LOAD_PATH.unshift("#{G}/concurrent-ruby-1.3.5/lib/concurrent-ruby")
require "active_record"
ActiveRecord::Base.establish_connection(adapter: "sqlite3", database: ":memory:")
def phase(n); print "#{n}: "; begin; yield; rescue Exception => e; puts "ERR #{e.class}: #{e.message}"; (e.backtrace||[]).first(4).each{|f| puts "    #{f}"}; end; end

phase("P3 schema") do
  ActiveRecord::Schema.verbose = false
  ActiveRecord::Schema.define do
    create_table :users, force: true do |t|
      t.string :name
      t.integer :age
    end
  end
  puts "OK"
end
phase("P4 model+create") do
  Object.const_set(:User, Class.new(ActiveRecord::Base))
  u = User.create(name: "Alice", age: 30)
  puts "created ##{u.id} #{u.name} #{u.age}"
end
phase("P5 find/where/count") do
  User.create(name: "Bob", age: 25)
  puts "count=#{User.count} where(age:30)=#{User.where(age: 30).first&.name} order=#{User.order(:age).map(&:name).inspect}"
end
phase("P6 update/destroy") do
  a = User.find_by(name: "Alice"); a.update(age: 31)
  puts "updated=#{User.find(a.id).age} destroyed→count=#{(a.destroy; User.count)}"
end
