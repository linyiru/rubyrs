# Error / backtrace line numbers for failures deep in a file (multi-KB
# byte offsets, multibyte padding in between) — guards the offset→line
# arithmetic shared by backtrace formatting and the (now lazy)
# const_source_location stamping. Every error is rescued and printed so
# both interpreters exit 0 and the harness diffs the rendered locations.

def deep_boom
  raise ArgumentError, "deep-boom"
end

# ---- padding: push everything below to deep byte offsets ------------
# Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do
# eiusmod tempor incididunt ut labore et dolore magna aliqua. Ut enim
# ad minim veniam, quis nostrud exercitation ullamco laboris nisi ut
# aliquip ex ea commodo consequat. Duis aute irure dolor in
# reprehenderit in voluptate velit esse cillum dolore eu fugiat nulla
# pariatur. Excepteur sint occaecat cupidatat non proident, sunt in
# culpa qui officia deserunt mollit anim id est laborum.
# 多字节のパディング: ラドクリフ、マラソン五輪代表に1万m出場にも含み。
# αβγδεζηθικλμνξοπρστυφχψω ÀÈÌÒÙ àèìòù ãõñ Ünïcödé pâddîng 🎯🚀🔬.
# Еще немного кириллицы для разнообразия смеси байтов и символов.
# Sed ut perspiciatis unde omnis iste natus error sit voluptatem
# accusantium doloremque laudantium, totam rem aperiam, eaque ipsa
# quae ab illo inventore veritatis et quasi architecto beatae vitae
# dicta sunt explicabo. Nemo enim ipsam voluptatem quia voluptas sit
# aspernatur aut odit aut fugit, sed quia consequuntur magni dolores
# eos qui ratione voluptatem sequi nesciunt.

begin
  deep_boom
rescue ArgumentError => e
  p e.message
  # First backtrace entry: the raise site inside deep_boom.
  file, line = e.backtrace.first.split(":")[0, 2]
  p [File.basename(file), line]
end

begin
  nil.definitely_missing_method_xz
rescue NoMethodError => e
  file, line = e.backtrace.first.split(":")[0, 2]
  p [File.basename(file), line]
end

expected = __LINE__ + 1
raise "tail-boom" rescue err = $!
p err.message
p err.backtrace.first.split(":")[1] == expected.to_s
