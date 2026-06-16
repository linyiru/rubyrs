#!/usr/bin/env ruby
# frozen_string_literal: true

# Benchmark: pure kramdown vs the rostdown accelerator on the same
# `Kramdown::Document.new(src, opts).to_html` call, across realistic
# workloads. Because the accelerator and pristine kramdown live in the
# same process once the patch is installed, each workload is timed by
# toggling the accelerator on/off per Profile lookup.

require "benchmark/ips"

$LOAD_PATH.unshift File.expand_path("../lib", __dir__)
require "kramdown"
require "kramdown-parser-gfm"
require "rouge"
require "kramdown/rostdown"

# A kill switch so we can time the *pristine* kramdown path in the same
# process: when off, profile_for returns nil → every doc falls back.
module Kramdown
  module Rostdown
    @enabled = true
    class << self
      attr_accessor :enabled
      alias_method :__profile_for_real, :profile_for
      def profile_for(opts)
        return nil unless @enabled

        __profile_for_real(opts)
      end
    end
  end
end

JEKYLL_OPTS = {
  input: "GFM", hard_wrap: false, auto_ids: true,
  syntax_highlighter_opts: { default_lang: "plaintext", guess_lang: true }
}.freeze

PROSE_OPTS = { auto_ids: true, input: "GFM", hard_wrap: false }.freeze

# A realistic blog-post section (prose, headings, lists, links, quotes,
# emphasis) — the bread and butter of a static site.
PROSE = <<~MD
  ## Shipping a Rust renderer to the Ruby world

  We've been running [rostdown](https://example.com/rostdown) in
  production for a while now, and it's *fast* — "byte-identical" fast,
  if you'll pardon the pun. Here's what we learned along the way.

  ### Why bother?

  kramdown is wonderful, but pure-Ruby parsing dominates build time on
  large sites. A few hundred posts and you're waiting seconds per
  build. The fix isn't to replace kramdown --- it's to *accelerate* the
  common case and fall back for everything else.

  - Prose renders natively.
  - Anything exotic declines to Ruby.
  - The output is identical, so nobody notices... except the clock.

  > Don't optimize what you can't measure. We measured.

  See the `README` for the full story, and don't forget the trailing
  ellipsis...
MD

CODE = <<~MD
  ### A worked example

  Here's the hot loop, in Ruby:

  ```ruby
  def render(source, profile)
    sid = Native.rd_scan(source, profile)
    return nil if sid.negative?
    splice_highlighted_blocks(sid)
    Native.rd_render(sid)
  end
  ```

  And the equivalent shape in Rust:

  ```rust
  pub extern "C" fn rd_scan(src: *const u8, len: usize) -> i64 {
      let src = unsafe { slice::from_raw_parts(src, len) };
      parse(src).map_or(-1, store_session)
  }
  ```

  That's the whole trick.
MD

POST = (PROSE + "\n" + CODE + "\n") * 3

def assert_native(label, src, opts)
  Kramdown::Rostdown.enabled = true
  Kramdown::Rostdown.stats.clear
  Kramdown::Document.new(src, opts).to_html
  st = Kramdown::Rostdown.stats
  warn "  ! #{label}: NOT served natively (#{st.inspect}) — bench is unfair" if st[:native].zero?
end

def verify_identical(label, src, opts)
  Kramdown::Rostdown.enabled = false
  pure = Kramdown::Document.new(src, opts).to_html
  Kramdown::Rostdown.enabled = true
  acc = Kramdown::Document.new(src, opts).to_html
  raise "#{label}: NOT identical!" unless pure == acc
end

WORKLOADS = {
  "prose post (GFM, no highlight)" => [PROSE, PROSE_OPTS],
  "post w/ code (Jekyll+rouge)"    => [POST,  JEKYLL_OPTS],
}.freeze

# Rouge's HTMLLegacy formatter warns once per instantiation; kramdown
# builds one per code block, so a few thousand bench iterations would
# bury the report. Silence stderr only around the timed loop.
def without_stderr_noise
  orig = $stderr
  $stderr = File.open(File::NULL, "w")
  yield
ensure
  $stderr.close
  $stderr = orig
end

rouge_v = (Rouge.version rescue "?")
puts "Ruby #{RUBY_VERSION}  kramdown #{Kramdown::VERSION}  rouge #{rouge_v}"
puts "doc sizes: " + WORKLOADS.map { |k, (s, _)| "#{k}=#{s.bytesize}B" }.join("  ")
puts

WORKLOADS.each do |label, (src, opts)|
  verify_identical(label, src, opts)
  assert_native(label, src, opts)

  puts "── #{label} ─────────────────────────────────────────"
  without_stderr_noise do
    Benchmark.ips do |x|
      x.config(time: 3, warmup: 1)
      x.report("pure kramdown") do
        Kramdown::Rostdown.enabled = false
        Kramdown::Document.new(src, opts).to_html
      end
      x.report("rostdown accel") do
        Kramdown::Rostdown.enabled = true
        Kramdown::Document.new(src, opts).to_html
      end
      x.compare!
    end
  end
  puts
end
