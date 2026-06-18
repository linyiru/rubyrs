# raise <non-exception> → TypeError "exception class/object expected"
# (a rescuable StandardError), NOT raising the value itself. Sinatra's
# mapped_error specs do `raise 500`.
[500, 3.14, :sym, [1,2], {a:1}, true].each do |v|
  begin
    raise v
  rescue => e
    p [e.class, e.message]
  end
end
