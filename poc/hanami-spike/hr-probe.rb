# Hanami::Router 2.3.1 spike probe — mustermann + rack stack (no
# zeitwerk/dry), the most rubyrs-tractable slice of Hanami. Mirrors the
# sinatra spike: define routes, drive a Rack request end-to-end.
# Run (needs the stdlib battery for strscan): target/release/rubyrs
#   --features stdlib  (probe binary built with `cargo build --release
#   -p rubyrs --features stdlib`)  poc/hanami-spike/hr-probe.rb
#
# STATUS (2026-06-16): ALL THREE PHASES PASS — Hanami::Router boots,
# compiles routes, and serves Rack requests end-to-end. The phase-2
# walls fell in order: bare-super implicit block forwarding (mustermann
# Capture#parse → Node#parse yield), Regexp#names (parser.scan branch),
# and `super` from an is_a? override (Node#is_a?). strscan needs the
# stdlib feature (the vendored StringScanner lives there).
G = "/Users/linyiru/.rbenv/versions/3.4.1/lib/ruby/gems/3.4.0/gems"
$LP = $LOAD_PATH
[
  "hanami-router-2.3.1", "mustermann-3.1.1", "mustermann-contrib-3.1.1",
  "rack-3.1.10", "csv-3.3.2",
  # Real uri gem — mustermann's ast/translator.rb references
  # `URI::RFC2396_Parser`. Without it on the load path rubyrs falls to
  # its minimal vendored URI stub, which is incompatible with
  # mustermann's metaprogrammed `parser`/`Parser` const dance and made
  # `Mustermann::Rails.parser` resolve to the stub's `URI::RFC2396_Parser`
  # (→ "undefined method `on'"). CRuby autoloads `uri` implicitly.
  "uri-1.0.2", "net-protocol-0.2.2",
].each { |g| $LP.unshift("#{G}/#{g}/lib") }
$LP.unshift(File.expand_path("shim", __dir__))
require "shims" if File.exist?(File.expand_path("shim/shims.rb", __dir__))
require "uri"  # load the real uri BEFORE mustermann (see note above)

puts "== phase 1: require hanami/router"
begin
  require "hanami/router"
  puts "OK: Hanami::Router defined=#{defined?(Hanami::Router)}"
rescue Exception => e
  puts "P1-ERR: #{e.class}: #{e.message}"
  (e.backtrace || []).first(15).each { |f| puts "  #{f}" }
  exit 1
end

puts "== phase 2: define routes"
begin
  router = Hanami::Router.new do
    get "/",        to: ->(env) { [200, {"content-type" => "text/plain"}, ["home"]] }
    get "/users/:id", to: ->(env) { [200, {"content-type" => "text/plain"}, ["user #{env["router.params"][:id]}"]] }
  end
  puts "OK: router built"
rescue Exception => e
  puts "P2-ERR: #{e.class}: #{e.message}"
  (e.backtrace || []).first(15).each { |f| puts "  #{f}" }
  exit 1
end

puts "== phase 3: drive requests"
require "stringio"
def call_route(router, path)
  env = {
    "REQUEST_METHOD" => "GET", "PATH_INFO" => path, "QUERY_STRING" => "",
    "SERVER_NAME" => "localhost", "SERVER_PORT" => "80",
    "SERVER_PROTOCOL" => "HTTP/1.1", "rack.url_scheme" => "http",
    "rack.input" => StringIO.new(""), "rack.errors" => StringIO.new("".dup),
  }
  status, _h, body = router.call(env)
  out = []
  body.each { |c| out << c }
  [status, out.join]
end

begin
  p call_route(router, "/")
  p call_route(router, "/users/42")
  puts "OK: requests served"
rescue Exception => e
  puts "P3-ERR: #{e.class}: #{e.message}"
  (e.backtrace || []).first(15).each { |f| puts "  #{f}" }
  exit 1
end
