# An autovivified namespace (`Admin = Module.new` — zeitwerk's
# implicit namespaces) has an empty structural name but
# effective_name "Admin", so const_defined? / const_get / autoload?
# / scoped autoload firing must all key under "Admin::User".
Admin = Module.new
module Admin
  class User; end
end
p Admin.const_defined?(:User, false)
p Admin.const_defined?(:User)
p Admin.const_get(:User).name
p defined?(Admin::User)

# A scoped autoload on the anon module fires under the right key.
Shop = Module.new
target = "/tmp/rubyrs_diff_cav_item.rb"
File.write(target, "module Shop; class Item; end; end")
Shop.autoload(:Item, target)
p Shop.autoload?(:Item) == target
p Shop::Item.name
File.delete(target)
