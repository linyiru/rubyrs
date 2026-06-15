G = "/Users/linyiru/.rbenv/versions/3.4.1/lib/ruby/gems/3.4.0/gems"
$LOAD_PATH.unshift("#{G}/uri-1.0.2/lib")
begin
  require "uri"
  puts "OK: real uri loaded; URI() => #{URI("http://h:8080/p?q=1").inspect rescue $!.message}"
  u = URI.parse("http://user\@host:81/a/b?x=1#f")
  puts "host=#{u.host} port=#{u.port} path=#{u.path} query=#{u.query} scheme=#{u.scheme}"
rescue Exception => e
  puts "WALL: #{e.class}: #{e.message}"
  (e.backtrace || []).first(8).each { |f| puts "  #{f}" }
end
