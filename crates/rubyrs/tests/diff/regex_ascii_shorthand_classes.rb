# Ruby/Onigmo's \s \d \w \h shorthand classes are ASCII-ONLY, while
# the Rust regex engines default them to Unicode (\s matches U+00A0,
# \d matches arabic-indic digits, \w matches every Unicode letter).
# rubyrs rewrites them to explicit ASCII classes at pattern-prepare
# time (regex_engine.rs `rewrite_ascii_shorthand_classes`).
#
# Discovered by the front-matter differential: `---\s*\n` with a
# stray NBSP after the fence matched on rubyrs but not CRuby.
#
# \b / \B are the documented Onigmo ASYMMETRY: the word BOUNDARY is
# Unicode-aware even though \w is ASCII — café's é is non-\w yet
# /café\b/ matches. The rewrite must leave \b alone.

NBSP = " "
ARABIC_THREE = "٣" # ٣ ARABIC-INDIC DIGIT THREE

puts "== \\s family =="
[" ", "\t", "\n", "\v", "\f", "\r", NBSP, "x"].each do |c|
  puts "#{c.inspect}: s=#{!!(c =~ /\A\s\z/)} S=#{!!(c =~ /\A\S\z/)} [s]=#{!!(c =~ /\A[\s]\z/)} [S]=#{!!(c =~ /\A[\S]\z/)}"
end

puts "== \\d family =="
["0", "9", ARABIC_THREE, "a"].each do |c|
  puts "#{c.inspect}: d=#{!!(c =~ /\A\d\z/)} D=#{!!(c =~ /\A\D\z/)} [d]=#{!!(c =~ /\A[\d]\z/)} [^d]=#{!!(c =~ /\A[^\d]\z/)}"
end

puts "== \\w family =="
["a", "Z", "5", "_", "é", "日", "-"].each do |c|
  puts "#{c.inspect}: w=#{!!(c =~ /\A\w\z/)} W=#{!!(c =~ /\A\W\z/)} [w]=#{!!(c =~ /\A[\w]\z/)}"
end

puts "== \\h family (Onigmo hex shorthand) =="
["a", "F", "5", "g", ARABIC_THREE].each do |c|
  puts "#{c.inspect}: h=#{!!(c =~ /\A\h\z/)} H=#{!!(c =~ /\A\H\z/)} [h]=#{!!(c =~ /\A[\h]\z/)}"
end

puts "== \\b stays Unicode-aware (Onigmo asymmetry) =="
puts "caf|e-acute boundary: #{!!("café x" =~ /caf\b/)}"
puts "after e-acute:        #{!!("café x" =~ /café\b/)}"
puts "ascii hyphen:         #{!!("caf-x" =~ /caf\b/)}"
puts "cjk word:             #{!!("日本 x" =~ /日本\b/)}"
puts "\\B inside cafe:       #{!!("café" =~ /ca\Bf/)}"

puts "== class composition =="
puts "[\\sa] a:        #{!!("a" =~ /\A[\sa]\z/)}"
puts "[\\sa] nbsp:     #{!!(NBSP =~ /\A[\sa]\z/)}"
puts "[^\\S] tab:      #{!!("\t" =~ /\A[^\S]\z/)}"
puts "[^\\S] nbsp:     #{!!(NBSP =~ /\A[^\S]\z/)}"
puts "[\\d-] dash:     #{!!("-" =~ /\A[\d-]\z/)}"
puts "[\\w.] dot:      #{!!("." =~ /\A[\w.]\z/)}"
puts "[\\d\\s] mix:     #{!!(" " =~ /\A[\d\s]\z/)}"

puts "== inside scan / split / gsub =="
puts "scan:  #{"a1 b2#{NBSP}c3".scan(/\w\d/).inspect}"
puts "split: #{"x y#{NBSP}z".split(/\s/).inspect}"
puts "gsub:  #{"a1#{ARABIC_THREE}".gsub(/\d/, "#").inspect}"

puts "== extended mode (the carmine-discovered trap) =="
# Under /x the Rust engines ignore whitespace INSIDE character
# classes too (Onigmo keeps it) — so the ASCII rewrite must emit
# \x20, not a literal space, or x-mode patterns using \s stop
# matching spaces entirely. Discovered via rouge's ruby lexer
# (its x-mode module rule), pinned here on the rubyrs side.
xre = /
  (module)
  (\s+)
  (\w+)
/x
m = "module Foo".match(xre)
puts "x-mode \\s+: #{m ? [m[1], m[2].length, m[3]].inspect : "NO MATCH"}"
puts "x-mode [\\s]: #{!!(" " =~ /[\s]/x)}"
puts "x-mode [\\sa]: #{!!(" " =~ /[\sa]/x)} #{!!("a" =~ /[\sa]/x)}"

puts "== front-matter shape (the discovering case) =="
fm_ok  = "---\na: 1\n---\nbody\n"
fm_nbsp = "---#{NBSP}\na: 1\n---\nbody\n"
re = /\A(---\s*\n.*?\n?)^((---|\.\.\.)\s*$\n?)/m
puts "plain fence: #{!!(fm_ok =~ re)}"
puts "nbsp fence:  #{!!(fm_nbsp =~ re)}"
