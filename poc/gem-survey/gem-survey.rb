# Survey: how many popular rubygems load + run a smoke test on rubyrs vs CRuby.
#   ruby poc/gem-survey/gem-survey.rb
# Each gem runs in its own process via gem-probe.rb (isolation). CRuby is the
# oracle (should PASS all pure-Ruby ones); rubyrs is what we're probing.
ROOT   = File.expand_path("../..", __dir__)
PROBE  = File.join(__dir__, "gem-probe.rb")
CRUBY  = ENV["CRUBY"]  || "ruby"
RUBYRS = ENV["RUBYRS"] || File.join(ROOT, "target/release/rubyrs")

# [require-name, smoke-expr]. Popular gems; pure-Ruby ones should be tractable,
# C-extension ones (nokogiri/oj/bcrypt) are expected to fail cleanly.
GEMS = [
  ["rake",                   "Rake::VERSION"],
  ["thor",                   "Class.new(Thor) { desc 'x','x'; def x; end }.commands.key?('x')"],
  ["rainbow",                "Rainbow('hi').red.is_a?(String)"],
  ["dotenv",                 "Dotenv::Parser.call(\"A=1\\nB=2\")"],
  ["multi_json",             "MultiJson.dump({'a' => 1})"],
  ["mini_mime",              "MiniMime.lookup_by_extension('txt').content_type"],
  ["builder",                "Builder::XmlMarkup.new.tag!('a','x')"],
  ["diff/lcs",               "Diff::LCS.diff(%w[a b], %w[a c]).size"],
  ["connection_pool",        "ConnectionPool.new(size: 1) { Object.new }.with { |o| o.class }"],
  ["unicode/display_width",  "Unicode::DisplayWidth.of('a')"],
  ["regexp_parser",          "Regexp::Parser.parse('a(b)c').class"],
  ["set",                    "Set[1,2,2].size"],
  ["ostruct",                "OpenStruct.new(a: 1).a"],
  ["securerandom",           "SecureRandom.hex(4).length"],
  ["public_suffix",          "PublicSuffix.parse('example.com').sld"],
  ["addressable/uri",        "Addressable::URI.parse('https://x.io/a?b=1').host"],
  ["tzinfo",                 "TZInfo::Timezone.get('UTC').identifier"],
  ["money",                  "Money.new(100, 'USD').cents"],
  ["sequel",                 "Sequel.mock.class"],
  ["mail",                   "Mail.new(from: 'a@b.com', subject: 'hi').subject"],
  ["rss",                    "defined?(RSS)"],
  ["sorbet-runtime",         "defined?(T::Struct)"],
  ["rspec/core",             "defined?(RSpec::Core)"],
  ["parser/current",         "Parser::CurrentRuby.parse('1 + 1').type"],
  ["faraday",                "Faraday.new.class"],
  ["nokogiri",               "Nokogiri::VERSION"],   # C ext — expect clean fail
  ["oj",                     "Oj.dump({})"],          # C ext — expect clean fail
  ["bcrypt",                 "BCrypt::Password.create('x')"], # C ext — expect clean fail
]

def probe(interp, req, smoke)
  out = IO.popen({ "GEM_REQUIRE" => req, "GEM_SMOKE" => smoke }, [*interp.split, PROBE], err: [:child, :out], &:read)
  ok = $?.success?
  [ok, out.to_s.strip]
end

printf "%-24s %-8s %-8s  %s\n", "gem (require)", "CRuby", "rubyrs", "rubyrs detail (on divergence)"
puts "-" * 90
pass = 0
GEMS.each do |req, smoke|
  c_ok, _c = probe(CRUBY, req, smoke)
  r_ok, r  = probe(RUBYRS, req, smoke)
  pass += 1 if r_ok
  mark = ->(b) { b ? "PASS" : "fail" }
  detail = r_ok ? r.sub(/\APASS\s*/, "") : r.lines.first.to_s.strip.sub(/\AFAIL\s*/, "")
  printf "%-24s %-8s %-8s  %s\n", req, mark.call(c_ok), mark.call(r_ok), detail.to_s[0, 58]
end
puts "-" * 90
puts "rubyrs: #{pass}/#{GEMS.size} gems load + smoke-pass"
