# Marshal.load fires a pending autoload for a class name it encounters (CRuby).
# zeitwerk reloads re-arm class autoloads, then Marshal.load re-materialises
# dumped instances. Here we dump, remove the class, register an autoload that
# re-defines it, and load — the load must trigger the autoload.
class MzqD
  def initialize; @v = 42; end
  def v; @v; end
end
str = Marshal.dump(MzqD.new)
Object.send(:remove_const, :MzqD)

target = File.join(__dir__, "marshal_load_autoload_target.rb")
File.write(target, "class MzqD; def initialize; @v = 42; end; def v; @v; end; end")
Object.autoload(:MzqD, target)

obj = Marshal.load(str)
puts obj.class.name
puts obj.v
File.delete(target)
