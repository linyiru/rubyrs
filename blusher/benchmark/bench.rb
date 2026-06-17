# frozen_string_literal: true
# blusher vs rouge: highlight a corpus of source files to HTML (rouge's
# dominant real-world job). Files are named by lexer tag.
#   ruby -Ilib benchmark/bench.rb <corpus-dir>
require "rouge"
require "blusher"   # installs the drop-in; keeps the original lex as __blusher_rouge_lex

dir = ARGV[0] or abort "usage: bench.rb <corpus-dir of <tag> files>"
fmt = Rouge::Formatters::HTML.new

corpus = Dir[File.join(dir, "*")].select { |f| File.file?(f) }.filter_map do |f|
  lx = Rouge::Lexer.find(File.basename(f))
  next unless lx.is_a?(Class) && lx < Rouge::RegexLexer
  [lx, File.read(f)]
end
total_bytes = corpus.sum { |_, s| s.bytesize }

# files that take the fused (fast) path vs fall back to rouge
fused = corpus.count { |lx, _| Blusher::Shim.routable?(lx.tag) && Blusher::Native.fused_html? }

# `:rouge` forces the original lexer + rouge formatter; `:blusher` uses the
# patched lex (deferred stream) so HTML#format can fuse.
def run(corpus, fmt, mode)
  corpus.each do |lx, src|
    fmt.format(mode == :rouge ? lx.new.__blusher_rouge_lex(src).to_a : lx.new.lex(src))
  end
end

def timed(corpus, fmt, mode)
  3.times { run(corpus, fmt, mode) } # warm (also primes the table cache)
  iters = 0; t0 = Process.clock_gettime(Process::CLOCK_MONOTONIC)
  loop { run(corpus, fmt, mode); iters += 1; break if Process.clock_gettime(Process::CLOCK_MONOTONIC) - t0 >= 2.0 }
  (Process.clock_gettime(Process::CLOCK_MONOTONIC) - t0) / iters
end

rouge_t   = timed(corpus, fmt, :rouge)
blusher_t = timed(corpus, fmt, :blusher)
mbps = ->(t) { total_bytes / t / 1_000_000.0 }

puts "corpus: #{corpus.size} files, #{(total_bytes / 1024.0).round} KiB → HTML"
puts "  fused (fast path): #{fused}/#{corpus.size}  (rest fall back to rouge)"
printf "  rouge   : %6.1f ms/pass   %5.1f MB/s\n", rouge_t * 1000, mbps.call(rouge_t)
printf "  blusher : %6.1f ms/pass   %5.1f MB/s\n", blusher_t * 1000, mbps.call(blusher_t)
printf "  speedup : %.2fx\n", rouge_t / blusher_t
