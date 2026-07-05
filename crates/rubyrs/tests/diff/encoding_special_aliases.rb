# Encoding's special pseudo-names — CRuby's Encoding.find resolves
# "external"/"filesystem"/"locale"/"internal" beyond the real
# registry (encoding.c enc_find). Motivating consumer: stdlib
# find.rb:43 `Encoding.find("filesystem")`, reached by RuboCop's
# ResultCache via `Find.find` — the ticketed gap this fixture pins.
# The harness pins LC_ALL, so CRuby's locale charmap is UTF-8 on
# both sides and "locale" agrees with rubyrs's UTF-8-fixed Tier-1.

# --- find: values + identity against the default_external singleton ---
p Encoding.find("filesystem")
p Encoding.find("locale")
p Encoding.find("external")
# The one find() shape that returns NIL instead of raising:
# default_internal is nil at process start.
p Encoding.find("internal")
p Encoding.find("filesystem").equal?(Encoding.default_external)
p Encoding.find("external").equal?(Encoding.default_external)
p Encoding.find("filesystem").equal?(Encoding::UTF_8)
p Encoding.find("locale").equal?(Encoding::UTF_8)

# --- case-insensitive, like real encoding names ---
p Encoding.find("FILESYSTEM")
p Encoding.find("Filesystem")
p Encoding.find("LOCALE")
p Encoding.find("External")
p Encoding.find("INTERNAL")

# --- aliases: lowercase-exact keys, values naming default_external;
# --- "internal" ABSENT while default_internal is nil ---
al = Encoding.aliases
p al["filesystem"]
p al["locale"]
p al["external"]
p al.key?("internal")
p al.key?("FILESYSTEM")
p al.select { |k, _| %w[locale external filesystem internal].include?(k) }

# --- name_list: all four present ("internal" INCLUDED even when nil) ---
nl = Encoding.name_list
p %w[filesystem locale external internal].map { |n| nl.include?(n) }
p nl.include?("FILESYSTEM")
p nl.last(3)

# --- pseudo-names do not leak into Encoding.list or the constants ---
p(Encoding.list.map(&:name) &
  %w[filesystem locale external internal FILESYSTEM LOCALE EXTERNAL INTERNAL])
p(Encoding.constants.map(&:to_s) & %w[FILESYSTEM LOCALE EXTERNAL INTERNAL])

# --- unknown-name contrast: nearby spellings still raise ---
["filesystem2", " filesystem", "locale "].each do |n|
  begin
    Encoding.find(n)
    puts "found: #{n.inspect}"
  rescue ArgumentError => e
    puts "#{e.class}: #{e.message}"
  end
end

# --- the exact stdlib call shape (find.rb:43) ---
fs_encoding = Encoding.find("filesystem")
p("x".encoding == Encoding::US_ASCII ? fs_encoding : "x".encoding)

# --- dynamic tracking: external/filesystem follow default_external,
# --- locale does NOT; internal follows default_internal, appearing
# --- in aliases (tail order locale/external/filesystem/internal) ---
Encoding.default_external = Encoding::ASCII_8BIT
p Encoding.find("external")
p Encoding.find("filesystem")
p Encoding.find("locale")
p Encoding.find("external").equal?(Encoding::ASCII_8BIT)
p Encoding.aliases["external"]
p Encoding.aliases["filesystem"]
p Encoding.aliases["locale"]

Encoding.default_internal = Encoding::US_ASCII
p Encoding.find("internal")
p Encoding.find("internal").equal?(Encoding.default_internal)
p Encoding.aliases["internal"]
p Encoding.aliases.select { |k, _| %w[locale external filesystem internal].include?(k) }
p Encoding.name_list.include?("internal")

Encoding.default_internal = nil
p Encoding.find("internal")
p Encoding.aliases.key?("internal")
p Encoding.name_list.include?("internal")
