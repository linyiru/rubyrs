# Bare (implicit-self) is_a? / kind_of? / instance_of? inside an
# instance method route to the universal Object predicates.
module Conv
  def what
    return :page if is_a?(Page)
    return :post if kind_of?(Post)
    :other
  end
  def exact?; instance_of?(Page); end
  # Bare (implicit-self) dup/clone route to the universal shallow
  # copy — rack's Recursive#call does `dup._call(env)`.
  def via_dup; dup; end
  def via_clone; clone; end
end
class Page; include Conv; attr_accessor :n; end
class Post; include Conv; end
class Sub < Page; include Conv; end
p Page.new.what
p Post.new.what
p Sub.new.what
p [Page.new.exact?, Sub.new.exact?]

orig = Page.new; orig.n = 7
copy = orig.via_dup
p [copy.class.name, copy.n, copy.equal?(orig)]   # ["Page", 7, false]
p orig.via_clone.class.name                        # "Page"
