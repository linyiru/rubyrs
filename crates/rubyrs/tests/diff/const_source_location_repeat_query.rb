# Repeat-query correctness for the lazy const_source_location stamp:
# the offset→line resolution is memoized on the FIRST query (the raw
# stamp stores file + source + byte offset only), so a loop of
# repeated queries must (a) keep returning the identical [file, line]
# answer and (b) stay O(1) after the first scan — a fixture with 1000
# queries of constants defined after multi-KB padding completes in
# well under any test timeout on the memoized path, while still
# exercising the first-scan arithmetic far from byte 0. This is a
# correctness fixture (stable repeated answers), not a timing assert.

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
# magnam aliquam quaerat voluptatem. Ut enim ad minima veniam, quis
# nostrum exercitationem ullam corporis suscipit laboriosam, nisi ut
# aliquid ex ea commodi consequatur? Quis autem vel eum iure
# reprehenderit qui in ea voluptate velit esse quam nihil molestiae
# consequatur, vel illum qui dolorem eum fugiat quo voluptas nulla
# pariatur? At vero eos et accusamus et iusto odio dignissimos
# ducimus qui blanditiis praesentium voluptatum deleniti atque
# corrupti quos dolores et quas molestias excepturi sint occaecati
# cupiditate non provident, similique sunt in culpa qui officia
# deserunt mollitia animi, id est laborum et dolorum fuga. Et harum
# quidem rerum facilis est et expedita distinctio. Nam libero
# tempore, cum soluta nobis est eligendi optio cumque nihil impedit
# quo minus id quod maxime placeat facere possimus, omnis voluptas
# assumenda est, omnis dolor repellendus. Temporibus autem quibusdam
# et aut officiis debitis aut rerum necessitatibus saepe eveniet ut
# et voluptates repudiandae sint et molestiae non recusandae. Itaque
# earum rerum hic tenetur a sapiente delectus, ut aut reiciendis
# voluptatibus maiores alias consequatur aut perferendis doloribus
# asperiores repellat.

# ---- padding block 2: multibyte BEFORE the defines -------------------
# 多字节文字填充 — spans are BYTE offsets while lines walk chars, so
# UTF-8 here keeps the byte-vs-char arithmetic honest on every scan:
# ラドクリフ、マラソン五輪代表に1万m出場にも含み。火の国、山の国。
# Ünïcödé pâddîng: αβγδεζηθικλμνξοπρστυφχψω ÀÈÌÒÙ àèìòù ãõñ ç.
# Еще немного кириллицы для разнообразия смеси байтов и символов.
# 🎯🚀🔬🧪🧬 (4-byte emoji) — offsets past here are byte >> char.
# 日本語のテキストで数キロバイトのパディングを続けます。続く。

module RepeatDeep
  ANSWER = 42
end

class RepeatDeepClass
  NESTED_VAL = "n"
end

REPEAT_TOP = :top

# First queries — the one-and-only offset→line scan per constant.
first = [
  Object.const_source_location(:RepeatDeep),
  RepeatDeep.const_source_location(:ANSWER),
  Object.const_source_location(:RepeatDeepClass),
  RepeatDeepClass.const_source_location(:NESTED_VAL),
  Object.const_source_location(:REPEAT_TOP),
]
first.each { |f| p [File.basename(f[0]), f[1]] }

# 1000 repeat queries of each shape: every answer must be identical
# to the first (memoized answers can never drift; a mismatch prints
# the iteration and diverges from CRuby).
stable = true
1000.times do |i|
  again = [
    Object.const_source_location(:RepeatDeep),
    RepeatDeep.const_source_location(:ANSWER),
    Object.const_source_location(:RepeatDeepClass),
    RepeatDeepClass.const_source_location(:NESTED_VAL),
    Object.const_source_location(:REPEAT_TOP),
  ]
  if again != first
    stable = false
    p [i, again]
    break
  end
end
p stable

# remove_const + redefine must re-stamp (fresh location, fresh memo):
# the new line is the redefining write's line, and repeat queries of
# the NEW location are stable too.
Object.send(:remove_const, :REPEAT_TOP)
REPEAT_TOP = :moved
moved = Object.const_source_location(:REPEAT_TOP)
p [File.basename(moved[0]), moved[1]]
p moved[1] != first[4][1]
p 100.times.all? { Object.const_source_location(:REPEAT_TOP) == moved }
