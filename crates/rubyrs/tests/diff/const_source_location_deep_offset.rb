# Lazy const_source_location stamping (perf: the eager define-time
# offset→line scan was quadratic per file) must resolve the SAME
# [file, line] values as CRuby for constants defined deep in a file,
# after multibyte text, and via every defining-op shape (DefClass /
# DefModule / StoreConst). The padding below pushes the later
# definitions to multi-KB byte offsets so the lazy offset→line
# resolution is exercised well away from byte 0.

class DeepA; end

# ---- padding block 1 ------------------------------------------------
# Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do
# eiusmod tempor incididunt ut labore et dolore magna aliqua. Ut enim
# ad minim veniam, quis nostrud exercitation ullamco laboris nisi ut
# aliquip ex ea commodo consequat. Duis aute irure dolor in
# reprehenderit in voluptate velit esse cillum dolore eu fugiat nulla
# pariatur. Excepteur sint occaecat cupidatat non proident, sunt in
# culpa qui officia deserunt mollit anim id est laborum. Sed ut
# perspiciatis unde omnis iste natus error sit voluptatem accusantium
# doloremque laudantium, totam rem aperiam, eaque ipsa quae ab illo
# inventore veritatis et quasi architecto beatae vitae dicta sunt
# explicabo. Nemo enim ipsam voluptatem quia voluptas sit aspernatur
# aut odit aut fugit, sed quia consequuntur magni dolores eos qui
# ratione voluptatem sequi nesciunt. Neque porro quisquam est, qui
# dolorem ipsum quia dolor sit amet, consectetur, adipisci velit, sed
# quia non numquam eius modi tempora incidunt ut labore et dolore
# magnam aliquam quaerat voluptatem.

module DeepM
  DEEP_VAL = 41
end

# ---- padding block 2: multibyte chars BEFORE later defines ----------
# 多字节字符串填充 — line/col resolution walks chars while spans are
# BYTE offsets, so several KB of UTF-8 here pins the byte-vs-char
# arithmetic: ラドクリフ、マラソン五輪代表に1万m出場にも含み。
# Ünïcödé pâddîng: αβγδεζηθικλμνξοπρστυφχψω ÀÈÌÒÙ àèìòù ãõñ.
# 火の国、山の国、水の国。日本語のテキストで数キロバイトのパディング。
# Еще немного кириллицы для разнообразия смеси байтов и символов.
# 🎯🚀🔬🧪🧬 (4-byte emoji) — offsets past here are byte >> char.

class DeepB
  class Nested; end
end

DEEP_TOP = "deep"

fa = Object.const_source_location(:DeepA)
p [File.basename(fa[0]), fa[1]]
fv = DeepM.const_source_location(:DEEP_VAL)
p [File.basename(fv[0]), fv[1]]
fb = Object.const_source_location(:DeepB)
p [File.basename(fb[0]), fb[1]]
fn = Object.const_source_location("DeepB::Nested")
p [File.basename(fn[0]), fn[1]]
ft = Object.const_source_location(:DEEP_TOP)
p [File.basename(ft[0]), ft[1]]

# Reopen deep in the file: location must NOT move (first-define wins).
class DeepA; def poke; end; end
p Object.const_source_location(:DeepA)[1]

# Same answers when queried a second time (lazy resolution is pure).
p Object.const_source_location(:DeepB)[1] == fb[1]
p DeepM.const_source_location(:DEEP_VAL)[1] == fv[1]
