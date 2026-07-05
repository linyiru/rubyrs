# "no implicit conversion" TypeError spellings for the nil/true/false
# singletons — the `Value::conv_type_name` migration battery.
#
# CRuby's conversion TypeErrors spell the three singletons as their
# LITERAL value words ("no implicit conversion of nil into String"),
# never as class names; rubyrs's `Value::type_name` renders
# NilClass / "Boolean", so every inline format site had to migrate to
# `conv_type_name`. On top of that, CRuby uses THREE distinct message
# shapes across these ops (each probed vs CRuby 3.4.1):
#
#   1. "no implicit conversion of X into Y" — the rb_convert_type
#      family (String/Array/Hash targets, and rb_to_int-shaped
#      Integer ops like `1 << nil`, `Integer#digits`).
#   2. "no implicit conversion from nil to integer" — the num2long
#      family's nil pre-check (lowercase "integer", "from..to"):
#      Array.new, byteslice, String#match pos, sprintf %*d/%c width,
#      eval's line arg, Integer("1", nil), Integer#to_s/#ceil, caller.
#      Non-nil args in the same ops use shape 1.
#   3. "can't convert X into Y" — the explicit-conversion family
#      (Kernel#Integer/Float/Rational/Hash, sprintf %d/%f operands).
#      Value words for the singletons EXCEPT Hash(), which prints the
#      real class name ("can't convert TrueClass into Hash").
#
# Surprises pinned here: `Object.const_get(nil)` says "into String"
# (not Symbol); `const_set`/`autoload`/`autoload?` name args use the
# id-or-string guard family ("nil is not a symbol nor a string");
# `65.chr(nil)` says "into String" (not Encoding); a BigInt receiver's
# `chr` is always RangeError; `class_eval("1", nil)` ACCEPTS the nil
# filename while `eval("1", nil, nil)` raises.
def t(label)
  r = yield
  puts "#{label} => OK #{r.inspect[0, 30]}"
rescue Exception => e
  puts "#{label} => #{e.class}: #{e.message}"
end

# --- String targets (rb_convert_type family) ---
t("String.new(nil)")   { String.new(nil) }
t("String.new(true)")  { String.new(true) }
t("String.new(false)") { String.new(false) }
t("String.new(:x)")    { String.new(:x) }
t("'a' << nil")        { +"a" << nil }
t("'a' << true")       { +"a" << true }
t("'a' << false")      { +"a" << false }
t("'a'.concat(true)")  { (+"a").concat(true) }
t("'a'.prepend(nil)")  { (+"a").prepend(nil) }
t("'a'.replace(nil)")  { (+"a").replace(nil) }
t("'a'.replace(false)"){ (+"a").replace(false) }
t("'a'.chomp(true)")   { "a".chomp(true) }
t("'a'.chomp(nil)")    { "a".chomp(nil) }        # nil separator is VALID
t("'a'.chomp!(false)") { (+"a").chomp!(false) }
t("Regexp.new(nil)")   { Regexp.new(nil) }
t("Regexp.new(true)")  { Regexp.new(true) }
t("Regexp.escape(nil)"){ Regexp.escape(nil) }
t("Regexp.union('a', nil)") { Regexp.union("a", nil) }
t("/a/.match(true)")   { /a/.match(true) }
t("/a/.match(false)")  { /a/.match(false) }
t("autoload(:A, nil)")  { autoload(:A, nil) }
t("autoload(:A, true)") { autoload(:A, true) }
t("abort(true)")        { abort(true) }
t("sprintf(nil)")       { sprintf(nil) }
t("sprintf(true)")      { sprintf(true) }
t("require_relative(nil)")  { require_relative(nil) }
t("require_relative(true)") { require_relative(true) }
t("load(nil)")          { load(nil) }
t("eval(nil)")          { eval(nil) }
t("eval(true)")         { eval(true) }
t("eval('1', nil, nil)")  { eval("1", nil, nil) }
t("eval('1', nil, true)") { eval("1", nil, true) }
t("65.chr(nil)")  { 65.chr(nil) }   # into String, NOT Encoding
t("65.chr(true)") { 65.chr(true) }
t("65.chr(:x)")   { 65.chr(:x) }

# --- class_eval: nil filename ACCEPTED, non-String rejected ---
c = Class.new
t("class_eval(nil)")           { c.class_eval(nil) }
t("class_eval(true)")          { c.class_eval(true) }
t("class_eval('1', nil)")      { c.class_eval("1", nil) }
t("class_eval('1', true)")     { c.class_eval("1", true) }
t("class_eval('1', 'f', nil)") { c.class_eval("1", "f", nil) }
t("class_eval('1', 'f', true)"){ c.class_eval("1", "f", true) }

# --- const/autoload NAME args: two different CRuby families ---
t("Object.const_get(nil)")   { Object.const_get(nil) }
t("Object.const_get(true)")  { Object.const_get(true) }
t("Object.const_get(false)") { Object.const_get(false) }
t("Object.const_defined?(nil)")  { Object.const_defined?(nil) }
t("Object.const_defined?(true)") { Object.const_defined?(true) }
t("Object.const_source_location(nil)") { Object.const_source_location(nil) }
t("Object.const_set(nil, 1)")  { Object.const_set(nil, 1) }
t("Object.const_set(true, 1)") { Object.const_set(true, 1) }
t("autoload(nil, 'x')") { autoload(nil, "x") }
t("autoload(1, 'x')")   { autoload(1, "x") }
t("autoload?(nil)")     { autoload?(nil) }

# --- File/Dir path args ---
t("File.expand_path(nil)")      { File.expand_path(nil) }
t("File.expand_path(true)")     { File.expand_path(true) }
t("File.basename('a.rb', nil)") { File.basename("a.rb", nil) }
t("File.basename('a.rb', true)"){ File.basename("a.rb", true) }
t("File.join(nil)")             { File.join(nil) }
t("File.join(true)")            { File.join(true) }
t("File.join('a', nil)")        { File.join("a", nil) }
t("Dir.glob(nil)")              { Dir.glob(nil) }
t("Dir.glob([nil])")            { Dir.glob([nil]) }
t("Dir.glob(true)")             { Dir.glob(true) }

# --- Array/Hash targets ---
t("Array.new(nil)")   { Array.new(nil) }     # num2long: "from nil to integer"
t("Array.new(true)")  { Array.new(true) }
t("Array.new(false)") { Array.new(false) }
t("Array.new(:x)")    { Array.new(:x) }
t("[1,2][true..]")    { [1,2][true..] }      # a range BOUND can be a singleton
t("[].replace(nil)")  { [].replace(nil) }
t("[].replace(true)") { [].replace(true) }
t("[[1],nil].transpose")  { [[1],nil].transpose }
t("[[1],true].transpose") { [[1],true].transpose }
t("[1].first(nil)")   { [1].first(nil) }
t("[1].first(true)")  { [1].first(true) }
t("[1].pop(false)")   { [1].pop(false) }
t("[1].each_slice(nil)") { [1].each_slice(nil) }
t("{}.merge(nil)")    { {}.merge(nil) }
t("{}.merge(true)")   { {}.merge(true) }
t("{}.merge(false)")  { {}.merge(false) }

# --- num2long-shaped Integer args ("from nil to integer") ---
t("'abc'.byteslice(0, nil)")  { "abc".byteslice(0, nil) }
t("'abc'.byteslice(0, true)") { "abc".byteslice(0, true) }
t("'abc'.match(/b/, nil)")    { "abc".match(/b/, nil) }
t("'abc'.match(/b/, true)")   { "abc".match(/b/, true) }
t("caller(nil)")      { caller(nil) }
t("caller(true)")     { caller(true) }
t("caller(false)")    { caller(false) }
t("Integer('1', nil)")  { Integer("1", nil) }
t("Integer('1', true)") { Integer("1", true) }
t("1.to_s(nil)")      { 1.to_s(nil) }
t("1.to_s(true)")     { 1.to_s(true) }
t("1.to_s(false)")    { 1.to_s(false) }
t("1.ceil(nil)")      { 1.ceil(nil) }
t("1.round(true)")    { 1.round(true) }
t("1.floor(false)")   { 1.floor(false) }
t("(2**100).to_s(nil)")  { (2**100).to_s(nil) }
t("(2**100).to_s(true)") { (2**100).to_s(true) }
t("(2**100).ceil(nil)")  { (2**100).ceil(nil) }
t("sprintf('%*d', nil, 5)")   { sprintf("%*d", nil, 5) }
t("sprintf('%*d', true, 5)")  { sprintf("%*d", true, 5) }
t("sprintf('%.*f', nil, 1.0)"){ sprintf("%.*f", nil, 1.0) }
t("sprintf('%c', nil)")  { sprintf("%c", nil) }
t("sprintf('%c', true)") { sprintf("%c", true) }
t("exit(:x)")            { exit(:x) }
t("exit('1')")           { exit("1") }

# --- rb_to_int-shaped Integer args ("of nil into Integer") ---
t("srand(nil)")       { srand(nil) }
t("srand(true)")      { srand(true) }
t("Random.new(true)") { Random.new(true) }
t("1 << nil")         { 1 << nil }
t("1 << true")        { 1 << true }
t("1 >> nil")         { 1 >> nil }
t("1.allbits?(nil)")  { 1.allbits?(nil) }
t("123.digits(nil)")  { 123.digits(nil) }
t("123.digits(true)") { 123.digits(true) }

# --- "can't convert" family (explicit conversions) ---
t("Integer(nil)")     { Integer(nil) }
t("Integer(true)")    { Integer(true) }
t("Integer(false)")   { Integer(false) }
t("Float(nil)")       { Float(nil) }
t("Float(true)")      { Float(true) }
t("Rational(nil)")    { Rational(nil) }
t("Rational(true)")   { Rational(true) }
t("Hash(true)")       { Hash(true) }     # class name: "TrueClass"
t("Hash(1)")          { Hash(1) }
t("sprintf('%d', nil)")  { sprintf("%d", nil) }
t("sprintf('%d', true)") { sprintf("%d", true) }
t("sprintf('%d', false)"){ sprintf("%d", false) }
t("sprintf('%f', nil)")  { sprintf("%f", nil) }
t("sprintf('%f', true)") { sprintf("%f", true) }
t("sprintf('%f', :x)")   { sprintf("%f", :x) }

# --- BigInt#chr: RangeError for EVERY arg (receiver checked first) ---
t("(2**100).chr(:x)")  { (2**100).chr(:x) }
t("(2**100).chr(nil)") { (2**100).chr(nil) }
