# String#split (regex AND literal-string separator) preserves the
# receiver's bytes and encoding for BINARY / invalid-UTF-8 receivers,
# instead of U+FFFD-mangling high bytes and re-tagging UTF-8. rack's
# QueryParser splits a BINARY query string by `/& */n` then each pair
# by `"="`; `_method=\xBF` must keep its invalid byte so the later
# `.upcase` raises (spec_method_override).

show = ->(a) { a.map { |s| [s.bytes, s.encoding.to_s] } }

# Regex split of a BINARY string with a raw high byte.
p show.call("_method=\xBF".b.split(/[&;] */n))     # [[..., 191], "ASCII-8BIT"]
p show.call("a\xFF&b\xFE".b.split(/&/n))           # two BINARY chunks, bytes kept

# Literal-string split of a BINARY string.
p show.call("_method=\xBF".b.split("=", 2))        # ["_method", "\xBF"] BINARY
p show.call("\xC0=\xC1=\xC2".b.split("="))         # 3 BINARY chunks
p show.call("a=b=c=".b.split("=", -1))             # trailing empty kept
p show.call("a=b=".b.split("="))                   # trailing empty dropped (limit 0)

# The QueryParser-shape round trip: invalid byte survives → upcase raises.
val = "_method=\xBF".b.split(/[&;] */n).first.split("=", 2).last
p val.bytes
p val.valid_encoding?
begin
  val.dup.force_encoding("UTF-8").upcase
  puts "no raise"
rescue ArgumentError
  puts "ArgumentError"
end

# Valid UTF-8 split is unchanged (chunks tagged UTF-8).
p show.call("a,café,c".split(","))
p show.call("x=y=z".split("=", 2))
