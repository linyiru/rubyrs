#!/usr/bin/env ruby
# frozen_string_literal: true

# Differential conformance harness: render every case in kramdown's own
# test corpora twice — once through PRISTINE kramdown, once through the
# rostdown accelerator — and assert byte-identical HTML.
#
# This is the strongest form of the "zero code change drop-in" claim: if
# the accelerated output equals pure kramdown's for every corpus input,
# then the accelerator passes exactly the spec kramdown passes (rostdown
# renders the subset it can reproduce byte-for-byte and declines the rest
# to the pure-Ruby path). We also report how much of the corpus rostdown
# served natively, which is the coverage dashboard.
#
# Usage: ruby bin/spec_diff.rb [-v]

require "yaml"

VERBOSE = ARGV.include?("-v")

# ---- locate the installed kramdown corpora -------------------------
require "kramdown"
require "kramdown-parser-gfm"

def gem_test_dir(spec_name)
  spec = Gem::Specification.find_by_name(spec_name)
  File.join(spec.gem_dir, "test", "testcases")
end

CORPORA = [
  { name: "kramdown-core", root: gem_test_dir("kramdown"),             extra: {} },
  { name: "gfm",           root: gem_test_dir("kramdown-parser-gfm"),  extra: { input: "GFM" } },
].freeze

DEFAULT_OPTS = { auto_ids: false, footnote_nr: 1 }.freeze

def options_for(text_file, extra)
  opts_file = text_file.sub(/\.text\z/, ".options")
  opts_file = File.join(File.dirname(text_file), "options") unless File.exist?(opts_file)
  base = File.exist?(opts_file) ? YAML.unsafe_load(File.read(opts_file)) : DEFAULT_OPTS.dup
  base.merge(extra)
end

def collect_cases(corpus)
  Dir.glob(File.join(corpus[:root], "**", "*.text")).sort.filter_map do |text_file|
    html_file = text_file.sub(/\.text\z/, ".html")
    next unless File.exist?(html_file) # to_html cases only

    { path: text_file, src: File.read(text_file), opts: options_for(text_file, corpus[:extra]) }
  end
end

# [:ok, html] | [:err, exception_class_name]. ScriptError catches the
# LoadError kramdown raises when an optional dep (e.g. stringex, for
# transliterated header ids) is absent — pristine and accelerated both
# hit it identically, so it compares equal.
def render(src, opts)
  [:ok, Kramdown::Document.new(src, opts).to_html]
rescue StandardError, ScriptError => e
  [:err, e.class.name]
end

ALL = CORPORA.map { |c| [c, collect_cases(c)] }.freeze

# ---- pass 1: pristine kramdown (BEFORE the accelerator is loaded) ---
baseline = {}
ALL.each do |corpus, cases|
  cases.each { |c| baseline[c[:path]] = render(c[:src], c[:opts]) }
end

# ---- load the accelerator, then pass 2 -----------------------------
require_relative "../lib/kramdown/rostdown"
Kramdown::Rostdown.stats.clear

mismatches = []
totals = Hash.new(0)
ALL.each do |corpus, cases|
  cases.each do |c|
    totals["#{corpus[:name]}.total"] += 1
    got = render(c[:src], c[:opts])
    next if got == baseline[c[:path]]

    mismatches << { path: c[:path], opts: c[:opts], expected: baseline[c[:path]], got: got }
  end
end

# ---- report --------------------------------------------------------
total_cases = ALL.sum { |_, cases| cases.size }
st = Kramdown::Rostdown.stats

puts "=" * 64
puts "kramdown-rostdown — differential conformance (accelerated vs pure)"
puts "=" * 64
ALL.each do |corpus, cases|
  puts format("  %-14s %4d cases", corpus[:name], cases.size)
end
puts "  #{"-" * 30}"
puts format("  %-14s %4d cases", "TOTAL", total_cases)
puts
puts "  rostdown native hits : #{st[:native]}"
puts "  rostdown declined    : #{st[:decline]}    (fell back, still identical)"
puts "  options ineligible   : #{st[:ineligible]} (fell back, still identical)"
cov = total_cases.zero? ? 0 : (100.0 * st[:native] / total_cases)
puts format("  native coverage      : %.1f%% of all corpus cases", cov)
puts
if mismatches.empty?
  puts "  RESULT: PASS — accelerated output is byte-identical to pure"
  puts "          kramdown on all #{total_cases} corpus cases."
else
  puts "  RESULT: FAIL — #{mismatches.size} case(s) diverged:"
  mismatches.first(VERBOSE ? mismatches.size : 12).each do |m|
    puts "   • #{m[:path].sub(%r{.*/testcases/}, "")}  opts=#{m[:opts].inspect}"
    if VERBOSE
      puts "     expected: #{m[:expected].inspect}"
      puts "     got:      #{m[:got].inspect}"
    end
  end
end
puts "=" * 64

exit(mismatches.empty? ? 0 : 1)
