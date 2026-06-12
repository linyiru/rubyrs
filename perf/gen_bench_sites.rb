# Regenerates the two Jekyll byte-identity test sites under /tmp
# (jk-site-real-1k: 1000 posts; jk-real: 6 posts) — kramdown(GFM) +
# rouge fences (ruby/python) + liquid tags, the "real" render load.
#
# WHY IN-REPO: the sites, boot scripts, and even gem data files live
# in /tmp and the macOS tmp cleaner ate them mid-session (2026-06-12
# — including addressable's unicode.data, which produced a VERY
# confusing ENOENT inside the gate). Run this with CRuby, then
# rebuild baselines:
#   ruby perf/gen_bench_sites.rb
#   TZ=UTC ruby /tmp/jk-boot-real-1k-cruby.rb   # baseline
# Boot scripts are written alongside; gems vendor at /tmp/jk-gems
# (copy from rbenv if missing, incl. addressable/data/unicode.data).
require "fileutils"
def build_site(dir, n_posts)
  FileUtils.rm_rf(Dir["#{dir}/_site*"]) # stale outputs from the old corpus
  FileUtils.mkdir_p("#{dir}/_layouts")
  FileUtils.mkdir_p("#{dir}/_posts")
  File.write("#{dir}/_config.yml", <<~YAML)
    title: bench-site
    markdown: kramdown
    kramdown:
      input: GFM
      syntax_highlighter: rouge
    permalink: /:year/:title/
  YAML
  File.write("#{dir}/_layouts/default.html", <<~HTML)
    <!DOCTYPE html>
    <html><head><title>{{ page.title }} - {{ site.title }}</title></head>
    <body><main>{{ content }}</main></body></html>
  HTML
  File.write("#{dir}/_layouts/post.html", <<~HTML)
    ---
    layout: default
    ---
    <article><h1>{{ page.title }}</h1>{{ content }}</article>
  HTML
  File.write("#{dir}/index.md", <<~MD)
    ---
    layout: default
    title: Home
    ---
    # Posts
    {% for post in site.posts limit: 10 %}
    - [{{ post.title }}]({{ post.url }})
    {% endfor %}
  MD
  n_posts.times do |i|
    day = (i % 27) + 1
    month = (i % 12) + 1
    body = <<~MD
      ---
      layout: post
      title: "Post number #{i}"
      tags: [bench, t#{i % 7}]
      ---
      ## Section #{i}

      Some *emphasis* and **strong** text with `inline code` and a [link](/x/#{i}).

      - item one #{i}
      - item two
        - nested

      ```ruby
      class Demo#{i % 13}
        def run(n = #{i})
          (1..n).map { |x| x * 2 }.select(&:even?).sum
        end
      end
      ```

      ```python
      def fib(n=#{i % 20}):
          a, b = 0, 1
          for _ in range(n):
              a, b = b, a + b
          return a
      ```

      > blockquote line #{i}

      Final paragraph {{ page.title | downcase }}.
    MD
    File.write(format("#{dir}/_posts/2026-%02d-%02d-post-%04d.md", month, day, i), body)
  end
end
build_site("/tmp/jk-site-real-1k", 1000)
build_site("/tmp/jk-real", 6)
puts "sites rebuilt"

# Boot scripts (idempotent rewrite).
%w[/tmp/jk-boot-real-1k.rb /tmp/jk-boot-real-1k-cruby.rb].each do |f|
  File.write(f, <<~RB)
    $LOAD_PATH.unshift(*Dir["/tmp/jk-gems/gems/*/lib"])
    $LOAD_PATH.unshift(*Dir["/tmp/jk-gems/gems/concurrent-ruby-*/lib/concurrent-ruby"])
    $LOAD_PATH.unshift("/tmp/jk-shim-real")
    Dir["/tmp/jk-shim-real/shim_*.rb"].sort.each { |x| require x }
    require "jekyll"
    config = Jekyll.configuration({"source"=>"/tmp/jk-site-real-1k","destination"=>"/tmp/jk-site-real-1k/_site","disable_disk_cache"=>true,"quiet"=>true})
    Jekyll::Site.new(config).process
  RB
end
%w[/tmp/jk-boot-real.rb /tmp/jk-boot-real-cruby.rb].each do |f|
  File.write(f, <<~RB)
    $LOAD_PATH.unshift(*Dir["/tmp/jk-gems/gems/*/lib"])
    $LOAD_PATH.unshift(*Dir["/tmp/jk-gems/gems/concurrent-ruby-*/lib/concurrent-ruby"])
    $LOAD_PATH.unshift("/tmp/jk-shim-real")
    Dir["/tmp/jk-shim-real/shim_*.rb"].sort.each { |x| require x }
    require "jekyll"
    config = Jekyll.configuration({"source"=>"/tmp/jk-real","destination"=>"/tmp/jk-real/_site","disable_disk_cache"=>true,"quiet"=>true})
    Jekyll::Site.new(config).process
  RB
end
puts "boot scripts rewritten"
