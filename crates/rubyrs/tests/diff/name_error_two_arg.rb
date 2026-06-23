# NameError.new(msg, name) — the second positional is the offending name,
# read via #name (CRuby). zeitwerk raises NameError.new(msg, cref.cname).
e = NameError.new("oops", :Foo)
p e.message
p e.name
p NameError.new("m").name
p NameError.new.name
class MyNE < NameError; end
p MyNE.new("x", :Bar).name
begin
  raise NameError.new("boom", :Baz)
rescue NameError => err
  p [err.message, err.name]
end
