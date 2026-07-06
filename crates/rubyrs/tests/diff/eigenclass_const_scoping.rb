## S3 item (a): eigenclass-scoped constants no longer LEAK into the
## flat top-level const table. CRuby scopes a `class << X; CONST = …`
## constant on X's singleton class: lexical reads from the body and
## from methods defined there resolve it, but `X::CONST`, top-level
## `CONST`, and `Object.const_get(:CONST)` all NameError; external
## access goes through the eigenclass value
## (`X.singleton_class::CONST`).
##
## rubyrs mechanism: the eigenclass-body proto's class_path carries a
## synthetic `#<Class:…>` scope segment, so the flat store key is
## unspellable from source; the shell's own `consts` side-table
## (populated by Op::StoreConst's eigenclass arm) serves the
## singleton_class-value read paths (resolve_const_path,
## const_get/const_defined?, constants(false)).

class Widget
  class << self
    EIGC = "eig-const"
    def peek
      EIGC
    end
    def peek_qualified
      Widget::EIGC
    rescue NameError => e
      "NameError: #{e.message}"
    end
    def peek_missing
      NO_SUCH_EIG_CONST
    rescue NameError => e
      "NameError: #{e.message}"
    end
  end
  def inst_peek
    EIGC
  rescue NameError
    "inst NameError"
  end
end

## Lexical read from a singleton method defined in the same body.
puts "peek=#{Widget.peek}"

## The constant is NOT a top-level constant…
begin
  Object.const_get(:EIGC)
  puts "const_get=LEAKED"
rescue NameError
  puts "const_get=NameError"
end
begin
  EIGC
  puts "toplevel=LEAKED"
rescue NameError
  puts "toplevel=NameError"
end

## …and NOT a constant of Widget itself (CRuby: the eigenclass is a
## different scope).
puts "peek_qualified=#{Widget.peek_qualified}"
puts "widget_const_defined=#{Widget.const_defined?(:EIGC, false)}"

## A genuinely-missing bare name inside the eigenclass scope reports
## the eigenclass as the innermost cref (CRuby message shape).
puts "peek_missing=#{Widget.peek_missing}"

## It LIVES on the singleton class: qualified read through the
## eigenclass value, const_get, const_defined?, constants(false).
sc = Widget.singleton_class
puts "sc_colon2=#{sc::EIGC}"
puts "sc_const_get=#{sc.const_get(:EIGC)}"
puts "sc_const_defined=#{sc.const_defined?(:EIGC, false)}"
puts "sc_constants=#{sc.constants(false).inspect}"

## An instance method of Widget does NOT see it (different cref).
puts "inst_peek=#{Widget.new.inst_peek}"

## Same family through the `class << Const` spelling.
class Gadget; end
class << Gadget
  GEIG = 42
  def gpeek
    GEIG
  end
end
puts "gpeek=#{Gadget.gpeek}"
begin
  Object.const_get(:GEIG)
  puts "geig_object=LEAKED"
rescue NameError
  puts "geig_object=NameError"
end
begin
  Gadget::GEIG
  puts "geig_qualified=LEAKED"
rescue NameError
  puts "geig_qualified=NameError"
end
puts "geig_sc=#{Gadget.singleton_class::GEIG}"
puts "geig_sc_list=#{Gadget.singleton_class.constants(false).inspect}"

## The enclosing class's OWN constants stay lexically visible from
## the eigenclass body and its methods (CRuby cref = [#<Class:W>, W]).
class Layered
  OUTER = "outer"
  class << self
    INNER = "inner"
    def both
      "#{OUTER}/#{INNER}"
    end
  end
end
puts "layered=#{Layered.both}"
puts "layered_outer=#{Layered::OUTER}"
begin
  Layered::INNER
  puts "layered_inner=LEAKED"
rescue NameError
  puts "layered_inner=NameError"
end
