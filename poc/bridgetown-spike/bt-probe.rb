# Bridgetown 2.2.1 spike probe — see how far rubyrs gets requiring/booting
# bridgetown-core. Gem sources read from rbenv 3.4.1's gem dir.
# Run: target/release/rubyrs poc/bridgetown-spike/bt-probe.rb
G = "/Users/linyiru/.rbenv/versions/3.4.1/lib/ruby/gems/3.4.0/gems"
$LP = $LOAD_PATH
[
  "bridgetown-core-2.2.1", "bridgetown-foundation-2.2.1", "addressable-2.8.7",
  "amazing_print-1.8.1", "csv-3.3.2", "erubi-1.13.1",
  "faraday-2.12.2", "faraday-follow_redirects-0.5.0", "freyia-0.6.2",
  "i18n-1.14.7", "kramdown-2.5.2", "kramdown-parser-gfm-1.1.0",
  "liquid-5.4.0", "listen-3.10.0", "rack-3.1.10", "rackup-2.2.1",
  "rake-13.2.1", "roda-3.105.0", "rouge-4.7.0", "samovar-2.4.1",
  "serbea-2.4.1", "signalize-1.3.1",
  "streamlined-0.6.2", "tilt-2.7.0", "zeitwerk-2.7.1",
  "public_suffix-6.0.1",
  "hash_with_dot_access-2.2.0", "inclusive-1.1.0", "dry-inflector-1.3.1",
  "mapping-1.1.3", "console-1.36.0", "fiber-annotation-0.2.0", "fiber-local-1.1.0",
  "faraday-net_http-3.4.4",
  # Real net/http stack — now backed by the _socket + _openssl batteries
  # (ADR 0028/0029), so faraday's net_http adapter loads unshimmed.
  "net-http-0.6.0", "net-protocol-0.2.2", "uri-1.0.2", "ruby2_keywords-0.0.5",
  # rexml — kramdown's HTML parser dep, pulled in during render (CRuby
  # finds it via rubygems; rubyrs only sees the explicit $LOAD_PATH).
  "rexml-3.4.4",
].each { |g| $LP.unshift("#{G}/#{g}/lib") }
# concurrent-ruby's require_path is lib/concurrent-ruby (not lib), so
# `require "concurrent/map"` resolves under that subdir.
$LP.unshift("#{G}/concurrent-ruby-1.3.6/lib/concurrent-ruby")

# Curated pure-Ruby stdlib subset (find, fileutils) that rubyrs doesn't
# vendor. We expose ONLY these, not the whole stdlib dir, so heavyweight
# on-disk files (rubygems, bundler) don't shadow rubyrs' own built-in
# stubs. Built at runtime under /tmp by copying the real 3.4 stdlib files
# with plain File I/O (can't use FileUtils — that's one of the files we're
# bootstrapping) — avoids committing machine-specific absolute symlinks.
STDLIB_SRC = "/Users/linyiru/.rbenv/versions/3.4.1/lib/ruby/3.4.0"
SUBSET = "/tmp/bt-stdlib-subset"
Dir.mkdir(SUBSET) unless Dir.exist?(SUBSET)
%w[find.rb fileutils.rb].each do |f|
  src = File.join(STDLIB_SRC, f)
  File.write(File.join(SUBSET, f), File.read(src)) if File.exist?(src)
end
$LP.push(SUBSET)

$LP.unshift(File.expand_path("shim", __dir__))
require "shims" if File.exist?(File.expand_path("shim/shims.rb", __dir__))

puts "== phase 1: require bridgetown-core"
begin
  require "bridgetown-core"
  puts "OK: Bridgetown #{Bridgetown::VERSION}"
rescue Exception => e
  puts "P1-ERR: #{e.class}: #{e.message}"
  (e.backtrace || []).first(15).each { |f| puts "  #{f}" }
  exit 1
end

# Minimal on-disk site so phases 2-4 exercise a real read/render/write.
SITE = "/tmp/bt-probe-site"
require "fileutils"
FileUtils.rm_rf(SITE)
FileUtils.mkdir_p("#{SITE}/src")
File.write("#{SITE}/src/index.md", "---\nlayout: none\ntitle: Hello\n---\n# Hi\n\nA test page.\n")

cfg = nil
puts "== phase 2: Bridgetown.configuration =="
begin
  cfg = Bridgetown.configuration(
    root_dir: SITE, source: "#{SITE}/src", destination: "#{SITE}/output", quiet: true
  )
  puts "P2 OK: #{cfg.class}"
rescue Exception => e
  puts "P2-ERR: #{e.class}: #{e.message}"
  (e.backtrace || []).first(8).each { |f| puts "  #{f}" }
end

# Boot+configure benchmark scope: phases 1-2 are the slice both CRuby and
# rubyrs complete identically (rubyrs hits a Marshal-cache wall at phase 3).
# `BT_BOOT_ONLY=1` skips phases 3-4 so the benchmark times the same work on
# both (guard rather than `exit` — rubyrs surfaces `exit 0` as a nonzero
# SystemExit, which would falsely flag the run as failed).
unless ENV["BT_BOOT_ONLY"]
  site = nil
  puts "== phase 3: Bridgetown::Site.new(config) =="
  begin
    site = Bridgetown::Site.new(cfg)
    puts "P3 OK: #{site.class}"
  rescue Exception => e
    puts "P3-ERR: #{e.class}: #{e.message}"
    (e.backtrace || []).first(8).each { |f| puts "  #{f}" }
  end

  puts "== phase 4: site.process (read -> render -> write) =="
  begin
    site.process
    out = Dir.glob("#{SITE}/output/**/*").select { |f| File.file?(f) }.sort
    puts "P4 OK: wrote #{out.size} file(s)"
    # Echo the rendered output so its content can be diffed against CRuby
    # (rubyrs's matches byte-for-byte: `<h1 id="hi">Hi</h1>` etc.).
    out.each { |f| puts "  -> #{f.sub("#{SITE}/", "")}: #{File.read(f).gsub("\n", "\\n")}" }
  rescue Exception => e
    puts "P4-ERR: #{e.class}: #{e.message}"
    (e.backtrace || []).first(6).each { |f| puts "  #{f}" }
  end
end
