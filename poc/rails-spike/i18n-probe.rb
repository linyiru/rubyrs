G = "/Users/linyiru/.rbenv/versions/3.4.1/lib/ruby/gems/3.4.0/gems"
%w[i18n-1.14.7 concurrent-ruby-1.3.5].each { |g| p = Dir.exist?("#{G}/#{g}/lib/concurrent-ruby") ? "#{G}/#{g}/lib/concurrent-ruby" : "#{G}/#{g}/lib"; $LOAD_PATH.unshift(p) }
require "i18n"
puts "OK: I18n loaded"
I18n.backend = I18n::Backend::Simple.new
I18n.available_locales = [:en]
I18n.default_locale = :en
I18n.backend.store_translations(:en, { greeting: "Hello %{name}", errors: { format: "%{attribute} %{message}" } })
begin
  puts I18n.t(:greeting, name: "Ada")
  puts I18n.t("errors.format", attribute: "Name", message: "can't be blank")
  puts I18n.t(:missing, default: "fallback default")
rescue Exception => e
  puts "ERR: #{e.class}: #{e.message}"
  (e.backtrace || []).first(6).each { |f| puts "  #{f}" }
end
