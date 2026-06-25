# Wire a rubygems-free $LOAD_PATH over the installed gems so `require
# "sinatra/base"` resolves on rubyrs (which has no rubygems). The gems
# directory is taken from $SINATRA_FAST_GEMS, else GEM_HOME, else the
# rbenv default. Skips names rubyrs vendors natively so e.g. `require
# "json"` hits the built-in. Mirrors the gem-survey probe's load-path
# wiring (poc/gem-survey/gem-probe.rb).
G = ENV["SINATRA_FAST_GEMS"] ||
    (ENV["GEM_HOME"] && File.join(ENV["GEM_HOME"], "gems")) ||
    "#{ENV['HOME']}/.rbenv/versions/3.4.1/lib/ruby/gems/3.4.0/gems"

unless Dir.exist?(G)
  warn "sinatra-fast: gems dir not found: #{G} (set SINATRA_FAST_GEMS)"
  exit 2
end

VENDORED = %w[
  date bigdecimal openssl securerandom json csv strscan stringio set
  digest psych yaml zlib fiddle etc fcntl io-console pathname erb cgi
].freeze
# NB: do NOT vendor-skip `uri` — mustermann's AST translator needs the real
# uri gem's RFC2396_Parser#escape(regexp); rubyrs's vendored URI stub lacks it.

latest = {}
Dir.children(G).each do |d|
  next unless (m = d.match(/\A(.+)-(\d[\w.]*)\z/))
  name, ver = m[1], m[2]
  next if VENDORED.include?(name)
  cur = latest[name]
  latest[name] = [ver, d] if cur.nil? ||
                             (ver.split(".").map(&:to_i) <=> cur[0].split(".").map(&:to_i)) > 0
end
latest.each_value { |(_, d)| $LOAD_PATH.unshift("#{G}/#{d}/lib") }
cr = latest["concurrent-ruby"] and $LOAD_PATH.unshift("#{G}/#{cr[1]}/lib/concurrent-ruby")

# rubygems shims some apps reach for even with gems off.
unless defined?(Gem)
  module Gem
    def self.find_files(*) = []
    def self.loaded_specs = {}
  end
end
