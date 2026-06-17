# Per-gem probe: set up a rubygems-free $LOAD_PATH over the installed gems,
# require ONE gem, run a one-line smoke, print PASS / FAIL. Driven once per gem
# per interpreter by gem-survey.rb (separate process each, for isolation).
#
#   GEM_REQUIRE="rake" GEM_SMOKE="defined?(Rake) && Rake::VERSION" \
#     <interp> poc/gem-survey/gem-probe.rb
G = "/Users/linyiru/.rbenv/versions/3.4.1/lib/ruby/gems/3.4.0/gems"

# Gems rubyrs vendors NATIVELY (often C-ext-backed default gems). We must NOT
# put the on-disk copies on the load path, or `require "date"` etc. resolve to
# the gem's C-ext loader (`date_core`, `bigdecimal.so`, `openssl.so`) instead of
# rubyrs' built-in — a load-path artifact, not a real capability gap.
VENDORED = %w[
  date bigdecimal openssl securerandom json csv strscan stringio set
  digest psych yaml zlib fiddle etc fcntl io-console pathname
].freeze

# Add the newest installed version of every gem's lib/ dir. No rubygems on
# rubyrs, so we wire the load path by hand; "newest of each name" avoids
# double-versioned shadowing without real dependency resolution.
latest = {}
Dir.children(G).each do |d|
  next unless (m = d.match(/\A(.+)-(\d[\w.]*)\z/))
  name, ver = m[1], m[2]
  next if VENDORED.include?(name)
  cur = latest[name]
  latest[name] = [ver, d] if cur.nil? || (ver.split(".").map(&:to_i) <=> cur[0].split(".").map(&:to_i)) > 0
end
latest.each_value { |(_, d)| $LOAD_PATH.unshift("#{G}/#{d}/lib") }
# concurrent-ruby's require path is lib/concurrent-ruby, not lib.
cr = latest["concurrent-ruby"] and $LOAD_PATH.unshift("#{G}/#{cr[1]}/lib/concurrent-ruby")

# Pure-Ruby stdlib files rubyrs doesn't vendor (copied, not the whole dir, so
# on-disk rubygems/bundler don't shadow rubyrs' built-ins).
SRC = "/Users/linyiru/.rbenv/versions/3.4.1/lib/ruby/3.4.0"
SUB = "/tmp/gem-survey-stdlib"
Dir.mkdir(SUB) unless Dir.exist?(SUB)
%w[find.rb fileutils.rb tmpdir.rb tsort.rb].each do |f|
  s = File.join(SRC, f)
  File.write(File.join(SUB, f), File.read(s)) if File.exist?(s) && !File.exist?(File.join(SUB, f))
end
$LOAD_PATH.push(SUB)

# Minimal stubs for the unvendored rubygems/bundler boot surface.
module Bundler
  module SharedHelpers; def self.in_bundle?; false; end; end
  def self.setup(*); end
  def self.require(*); end
  def self.with_unbundled_env; yield if block_given?; end
end unless defined?(Bundler)
module Gem
  module Deprecate
    def deprecate(*); self; end
    def rubygems_deprecate(*); self; end
    def skip_during; yield if block_given?; end
  end
end unless defined?(Gem::Deprecate)

req   = ENV.fetch("GEM_REQUIRE")
smoke = ENV["GEM_SMOKE"]

begin
  require req
  if smoke && !smoke.empty?
    val = eval(smoke) # rubocop:disable Security/Eval — survey harness
    puts "PASS smoke=#{val.inspect[0, 60]}"
  else
    puts "PASS"
  end
rescue Exception => e # rubocop:disable Lint/RescueException — survey wants everything
  line = (e.backtrace || []).find { |f| !f.include?("gem-probe.rb") }
  puts "FAIL #{e.class}: #{e.message.to_s.gsub("\n", " ")[0, 100]}"
  puts "   @ #{line}" if line
  exit 2
end
