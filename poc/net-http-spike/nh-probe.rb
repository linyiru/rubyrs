# net/http discovery spike (ADR 0028 Phase 1). Loads the REAL MRI
# net/http.rb (+ net/protocol, uri) on rubyrs against a recording socket
# shim, drives a request both directly and through faraday's net_http
# adapter, and prints the exact socket host-fn surface + Net::HTTP public
# surface that gets exercised. Run:
#   target/release/rubyrs poc/net-http-spike/nh-probe.rb
G = "/Users/linyiru/.rbenv/versions/3.4.1/lib/ruby/gems/3.4.0/gems"
$LP = $LOAD_PATH
[
  "net-http-0.6.0", "net-protocol-0.2.2", "uri-1.0.2",
  "faraday-2.12.2", "faraday-net_http-3.4.4", "ruby2_keywords-0.0.5",
].each { |g| $LP.unshift("#{G}/#{g}/lib") }
# net/http.rb now parses UNPATCHED — the `class << HTTP; alias` wall was
# fixed in rubyrs (feat(parser): alias in class << <Const>). No vendor
# patch needed; the real gem net/http.rb loads directly.
$LP.unshift(File.expand_path("shim", __dir__))

require "recording_socket"

puts "== phase A: require net/http =="
begin
  require "net/http"
  require "uri"
  puts "OK: Net::HTTP defined=#{defined?(Net::HTTP)} version=#{Net::HTTP::VERSION rescue '?'}"
rescue Exception => e
  puts "A-ERR: #{e.class}: #{e.message}"
  (e.backtrace || []).first(20).each { |f| puts "  #{f}" }
  exit 1
end

puts "== phase B: direct Net::HTTP request =="
begin
  uri = URI("http://example.test/path?q=1")
  res = Net::HTTP.start(uri.host, uri.port) do |http|
    req = Net::HTTP::Get.new(uri)
    http.request(req)
  end
  puts "OK: code=#{res.code} body=#{res.body.inspect}"
rescue Exception => e
  puts "B-ERR: #{e.class}: #{e.message}"
  (e.backtrace || []).first(20).each { |f| puts "  #{f}" }
end

puts "== phase C: Net::HTTP.get convenience =="
begin
  body = Net::HTTP.get(URI("http://example.test/x"))
  puts "OK: body=#{body.inspect}"
rescue Exception => e
  puts "C-ERR: #{e.class}: #{e.message}"
  (e.backtrace || []).first(12).each { |f| puts "  #{f}" }
end

puts "== phase D: faraday over net_http adapter =="
begin
  require "faraday"
  conn = Faraday.new(url: "http://example.test") do |f|
    f.adapter :net_http
  end
  resp = conn.get("/api")
  puts "OK: status=#{resp.status} body=#{resp.body.inspect}"
rescue Exception => e
  puts "D-ERR: #{e.class}: #{e.message}"
  (e.backtrace || []).first(20).each { |f| puts "  #{f}" }
end

puts
puts "== DISCOVERED SOCKET HOST-FN SURFACE (method => calls) =="
$NH_CALLS.sort.each { |m, n| puts format("  %-26s %d", m, n) }
unhandled = $NH_CALLS.keys.select { |k| k.start_with?("UNHANDLED:") }
puts
if unhandled.empty?
  puts "no UNHANDLED socket methods — the shim surface is complete"
else
  puts "UNHANDLED socket methods (NOT anticipated — extend host-fn list):"
  unhandled.each { |u| puts "  #{u}" }
end
