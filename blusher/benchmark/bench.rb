# frozen_string_literal: true
# blusher vs rouge throughput, over a real corpus of source files.
#   ruby -Ilib benchmark/bench.rb <corpus-dir>   (files named by lexer tag)
require "rouge"
require "blusher"   # installs the drop-in; keeps the original lex as __blusher_rouge_lex

dir = ARGV[0] or abort "usage: bench.rb <corpus-dir of <tag> files>"
corpus = Dir[File.join(dir, "*")].select { |f| File.file?(f) }.filter_map do |f|
  lx = Rouge::Lexer.find(File.basename(f)) or next
  next unless lx.is_a?(Class) && lx < Rouge::RegexLexer
  [lx, File.read(f)]
end
total_bytes = corpus.sum { |_, s| s.bytesize }

# count carmine-routed vs rouge-fallback files (one representative lex each)
routed = corpus.count do |lx, src|
  Blusher::Native.lex(Blusher::Shim.table_for(lx.tag) || "", src)["status"] == "ok"
rescue StandardError
  false
end

def run(corpus, meth)
  corpus.each { |lx, src| lx.new.send(meth, src).each { |_| } }
end

# adaptive timing to a stable ~1s per engine
def timed(corpus, meth)
  3.times { run(corpus, meth) } # warm
  iters = 0; t0 = Process.clock_gettime(Process::CLOCK_MONOTONIC)
  loop { run(corpus, meth); iters += 1; break if Process.clock_gettime(Process::CLOCK_MONOTONIC) - t0 >= 1.0 }
  (Process.clock_gettime(Process::CLOCK_MONOTONIC) - t0) / iters
end

rouge_t   = timed(corpus, :__blusher_rouge_lex)
blusher_t = timed(corpus, :lex)
mbps = ->(t) { total_bytes / t / 1_000_000.0 }

puts "corpus: #{corpus.size} files, #{(total_bytes / 1024.0).round} KiB"
puts "  carmine-routed: #{routed}/#{corpus.size}  (rest fall back to rouge)"
printf "  rouge   : %6.1f ms/pass   %5.1f MB/s\n", rouge_t * 1000, mbps.call(rouge_t)
printf "  blusher : %6.1f ms/pass   %5.1f MB/s\n", blusher_t * 1000, mbps.call(blusher_t)
printf "  speedup : %.2fx\n", rouge_t / blusher_t
