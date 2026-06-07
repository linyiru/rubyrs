# Bare (implicit-self) is_a? / kind_of? / instance_of? inside an
# instance method route to the universal Object predicates.
module Conv
  def what
    return :page if is_a?(Page)
    return :post if kind_of?(Post)
    :other
  end
  def exact?; instance_of?(Page); end
end
class Page; include Conv; end
class Post; include Conv; end
class Sub < Page; include Conv; end
p Page.new.what
p Post.new.what
p Sub.new.what
p [Page.new.exact?, Sub.new.exact?]
