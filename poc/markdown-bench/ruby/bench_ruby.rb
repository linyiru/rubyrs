# frozen_string_literal: true

# Self-timed markdown render benchmark.
# Usage: ruby bench_ruby.rb <kramdown|rostdown-gem|commonmarker> <file> <iters>
# Emits: "<engine>\t<ns_per_op>\t<mb_per_s>\t<out_bytes>" on stdout.

engine = ARGV[0]
file   = ARGV[1]
iters  = Integer(ARGV[2] || "200")
src    = File.read(file)

GFM = { input: "GFM", auto_ids: true, hard_wrap: false }.freeze
# No-highlight: match the other engines, which don't syntax-highlight by
# default (apples-to-apples on raw parse + render).
GFM_NOHL = GFM.merge(syntax_highlighter: nil).freeze
JEKYLL = GFM.merge(syntax_highlighter_opts: { default_lang: "plaintext", guess_lang: true }).freeze

render =
  case engine
  when "kramdown"
    require "kramdown"
    require "kramdown-parser-gfm"
    ->(s) { Kramdown::Document.new(s, GFM_NOHL).to_html.bytesize }
  when "rostdown-gem"
    $LOAD_PATH.unshift File.expand_path("../../kramdown-rostdown/lib", __dir__)
    require "kramdown"
    require "kramdown-parser-gfm"
    require "rouge"
    require "kramdown/rostdown"
    ->(s) { Kramdown::Document.new(s, JEKYLL).to_html.bytesize }
  when "commonmarker"
    # commonmarker 2.x runs syntect syntax-highlighting by default;
    # disable it so the code-block path matches the other engines
    # (plain <pre><code>) — this measures comrak's parse, not syntect.
    require "commonmarker"
    ->(s) { Commonmarker.to_html(s, plugins: { syntax_highlighter: nil }).bytesize }
  else
    abort "unknown engine: #{engine}"
  end

# Rouge's HTMLLegacy deprecation chatter would flood the timed loop.
orig_stderr = $stderr
$stderr = File.open(File::NULL, "w")

out_bytes = 0
[iters / 5, 3].max.times { out_bytes = render.call(src) }

t0 = Process.clock_gettime(Process::CLOCK_MONOTONIC, :nanosecond)
sink = 0
iters.times { sink += render.call(src) }
t1 = Process.clock_gettime(Process::CLOCK_MONOTONIC, :nanosecond)

$stderr = orig_stderr
ns = t1 - t0
ns_per_op = ns.to_f / iters
mb_per_s = src.bytesize.to_f * iters / (ns / 1.0e9) / 1.0e6
puts "#{engine}\t#{format('%.0f', ns_per_op)}\t#{format('%.1f', mb_per_s)}\t#{out_bytes}"
