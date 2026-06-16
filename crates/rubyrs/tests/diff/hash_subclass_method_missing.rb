# A Hash/Array SUBCLASS instance must dispatch unknown methods to its
# class's `method_missing` (not raise NoMethodError directly off the
# collection primitive path). Surfaced by `HashWithDotAccess::Hash`,
# which exposes hash keys as methods via method_missing — the basis of
# Bridgetown's `Configuration < HashWithDotAccess::Hash` dot-access.
class DotHash < Hash
  def [](k) = super(k.to_s)
  def []=(k, v); super(k.to_s, v); end
  def key?(k) = super(k.to_s)
  def method_missing(name, *args)
    key = name.to_s
    if key.end_with?("=")
      self[key.chop] = args.first
    elsif key?(key)
      self[key]
    else
      super
    end
  end
  def respond_to_missing?(name, *) = key?(name.to_s.chomp("="))
end

h = DotHash.new
h.title = "hello"
puts h.title
puts h.respond_to?(:title)
puts h.respond_to?(:missing_key)
begin
  h.nonexistent
rescue NoMethodError => e
  puts "NoMethodError raised"
end

class TagArray < Array
  def method_missing(name, *) = name == :first_or_nil ? (self[0]) : super
end
a = TagArray.new([10, 20])
puts a.first_or_nil
