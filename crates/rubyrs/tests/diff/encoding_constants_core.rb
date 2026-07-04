# The common encoding-constant family is name-registered in EVERY
# build (activesupport's inflector/transliterate.rb references
# Encoding::GB18030 in ALLOWED_ENCODINGS_FOR_TRANSLITERATE at class-
# body load time, so `require "active_model"` needs it). Constants,
# find identity, aliases, reflection inclusion, dummy?/
# ascii_compatible? and inspect are pinned against CRuby here;
# TRANSCODING to these encodings stays behind `_encoding_full` (the
# decline shape is pinned in encoding_converter_not_found.rb).

FAMILY = %w[ISO-8859-1 Windows-1252 ISO-8859-15 KOI8-R Windows-31J
            EUC-JP GBK GB18030 Big5 UTF-16LE UTF-16BE UTF-16
            UTF-32LE UTF-32BE UTF-32 Shift_JIS ISO-2022-JP].freeze

# --- constant existence + name/to_s/inspect + the property table ---
CONSTS = {
  "ISO-8859-1" => Encoding::ISO_8859_1,
  "Windows-1252" => Encoding::Windows_1252,
  "ISO-8859-15" => Encoding::ISO_8859_15,
  "KOI8-R" => Encoding::KOI8_R,
  "Windows-31J" => Encoding::Windows_31J,
  "EUC-JP" => Encoding::EUC_JP,
  "GBK" => Encoding::GBK,
  "GB18030" => Encoding::GB18030,
  "Big5" => Encoding::Big5,
  "UTF-16LE" => Encoding::UTF_16LE,
  "UTF-16BE" => Encoding::UTF_16BE,
  "UTF-16" => Encoding::UTF_16,
  "UTF-32LE" => Encoding::UTF_32LE,
  "UTF-32BE" => Encoding::UTF_32BE,
  "UTF-32" => Encoding::UTF_32,
  "Shift_JIS" => Encoding::Shift_JIS,
  "ISO-2022-JP" => Encoding::ISO_2022_JP,
}.freeze
CONSTS.each do |want, enc|
  puts "#{want}: name=#{enc.name} to_s=#{enc} " \
       "dummy=#{enc.dummy?} ascii=#{enc.ascii_compatible?} " \
       "find_identity=#{Encoding.find(want).equal?(enc)}"
end

# Dummy encodings render their stable "(dummy)" form loaded or not.
p [Encoding::UTF_16.inspect, Encoding::UTF_32.inspect, Encoding::ISO_2022_JP.inspect]

# Non-dummy inspect, pinned AFTER the Encoding.find calls above: CRuby
# 3.4 renders a not-yet-LOADED registry encoding as
# "#<Encoding:GB18030 (autoload)>" (a lazy-registry artifact rubyrs
# deliberately doesn't model — its constants are always loaded), and
# find() is what loads one; by here every family member has been
# found, so both engines render the plain "#<Encoding:NAME>" form.
p [Encoding::GB18030.inspect, Encoding::Big5.inspect,
   Encoding::EUC_JP.inspect, Encoding::UTF_16LE.inspect,
   Encoding::UTF_32BE.inspect, Encoding::Shift_JIS.inspect,
   Encoding::ISO_8859_1.inspect, Encoding::Windows_31J.inspect]

# --- Encoding.constants inclusion (subset check — CRuby has ~180) ---
want_syms = %i[ISO_8859_1 Windows_1252 ISO_8859_15 KOI8_R Windows_31J
               EUC_JP GBK GB18030 Big5 UTF_16LE UTF_16BE UTF_16
               UTF_32LE UTF_32BE UTF_32 Shift_JIS ISO_2022_JP]
p want_syms - Encoding.constants

# --- find: case-insensitive + CRuby's alias fold set ---
p Encoding.find("gb18030").equal?(Encoding::GB18030)
p Encoding.find("big5").equal?(Encoding::Big5)
p Encoding.find("utf-16le").equal?(Encoding::UTF_16LE)
p Encoding.find("CP932").equal?(Encoding::Windows_31J)
p Encoding.find("SJIS").equal?(Encoding::Windows_31J)
p Encoding.find("CP936").equal?(Encoding::GBK)
p Encoding.find("CP1252").equal?(Encoding::Windows_1252)
p Encoding.find("ISO8859-1").equal?(Encoding::ISO_8859_1)
p Encoding.find("ISO8859-15").equal?(Encoding::ISO_8859_15)
p Encoding.find("EUCJP").equal?(Encoding::EUC_JP)
p Encoding.find("ISO2022-JP").equal?(Encoding::ISO_2022_JP)
p Encoding.find("shift_jis").equal?(Encoding::Shift_JIS)

# --- spellings CRuby REJECTS must stay rejected (verified 3.4.1:
# no un-hyphenated UTF forms, no KOI8R, no SHIFT-JIS) ---
%w[UTF16 UTF16LE UTF32 KOI8R SHIFT-JIS UTF8].each do |bad|
  begin
    Encoding.find(bad)
    puts "#{bad}: FOUND (bug)"
  rescue ArgumentError => e
    puts "#{bad}: #{e.message}"
  end
end

# --- comparisons with a live string's encoding (the activesupport
# transliterate idiom: ALLOWED.include?(string.encoding)) ---
s = "abc"
p s.encoding == Encoding::GB18030
p [Encoding::UTF_8, Encoding::US_ASCII, Encoding::GB18030].include?(s.encoding)
p [Encoding::UTF_8, Encoding::US_ASCII, Encoding::GB18030].map(&:name)

# --- usable as Hash keys with find-identity ---
p({ Encoding::GB18030 => :cn, Encoding::Big5 => :tw }[Encoding.find("GB18030")])

# --- reflection: list / name_list / aliases (intersection — ours is
# a documented subset of CRuby's registry) ---
p (FAMILY - Encoding.list.map(&:name))
p (FAMILY - Encoding.name_list)
p Encoding.aliases.select { |k, _| %w[CP932 SJIS CP936 CP1252 ISO8859-1 ISO8859-15].include?(k) }.sort.to_h
