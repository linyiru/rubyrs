# ERB (vendored erb.rb + erb/compiler.rb): ERB.new(str).result(binding).
# Mirrors rack's ShowExceptions / ShowStatus shape — the template
# reads the handler method's LOCALS through the captured binding
# (exception/path/frames analogue) plus calls a method (`h`) on the
# captured self. Requires the Kernel#binding local-capture layer.
require "erb"

class Page
  def h(s)
    s.to_s.gsub("&", "&amp;").gsub("<", "&lt;").gsub(">", "&gt;")
  end

  TEMPLATE = <<-'HTML'
<html>
  <head><title><%=h title %> (<%= status %>)</title></head>
  <body>
    <h1><%=h title %></h1>
    <% if items.empty? %>
      <p>no items</p>
    <% else %>
      <ul>
      <% items.each do |it| %>
        <li><%=h it %> &amp; co</li>
      <% end %>
      </ul>
    <% end %>
    <p>count: <%= items.size %></p>
    <p>raw: <%= "<b>" + name + "</b>" %></p>
  </body>
</html>
  HTML

  def render(title, status, items, name)
    ERB.new(TEMPLATE).result(binding)
  end
end

page = Page.new
print page.render("Hello & <World>", 500, ["a<b>", "c&d", "e"], "Bob")
print "----\n"
# Empty-collection branch + a different binding.
print page.render("Empty", 404, [], "Ann")
print "----\n"
# A standalone (no-class) binding: locals + a top-level expression.
greeting = "hi"
who = "there"
nums = [1, 2, 3]
tmpl = ERB.new("<%= greeting %>, <%= who %>! sum=<%= nums.sum %>\n<% nums.each { |n| %>* <%= n * n %>\n<% } %>")
print tmpl.result(binding)

# ERB#src is the compiled Ruby (stable shape: buffer var + .freeze
# literals). Pin that it is a String and round-trips the same twice.
e = ERB.new("a<%= 1 + 1 %>b")
puts e.src.class
puts(e.result(binding) == e.result(binding))
puts e.result(binding)
