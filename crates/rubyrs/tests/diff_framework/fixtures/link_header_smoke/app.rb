# link_header_smoke app — exercises real
# sinatra-contrib-4.2.1/lib/sinatra/link_header.rb vendored 1:1
# under vendor/sinatra/link_header.rb. The gem's last line is
# `helpers LinkHeader`; modular form needs explicit
# `helpers Sinatra::LinkHeader`.

require_relative "sinatra_compat"

class LinkHeaderSmokeApp < Sinatra::Base
  helpers Sinatra::LinkHeader

  # `link(:rel, *urls)` — Symbol first arg becomes the `rel`
  # value; subsequent args are URLs. Sets the Link header AND
  # returns matching HTML <link> tags as the response body.
  get "/link_simple" do
    link :next, "/page/2"
  end

  # Multiple URLs collapse into a single Link header (comma-
  # joined) and a concatenated HTML string.
  get "/link_multi" do
    link :prefetch, "/asset/a", "/asset/b"
  end

  # Explicit options Hash overrides the rel-from-first-arg
  # convention.
  get "/link_with_opts" do
    link "/feed.xml", rel: :alternate, type: "application/rss+xml"
  end

  # `stylesheet(*urls)` — auto-fills `type=text/css` via
  # `mime_type(:css)` and dispatches into `link`. Exercises the
  # `urls.last[:type] ||= mime_type(:css)` shape against
  # sinatra_lite's :css mime entry.
  get "/stylesheet" do
    stylesheet "/style.css"
  end

  # `prefetch(*urls)` — thin wrapper over `link(:prefetch, ...)`.
  get "/prefetch" do
    prefetch "/big-image.png", "/big-video.mp4"
  end

  # `link_headers` reads the current Link header back and
  # produces HTML for each entry. Exercises the `response.include?`
  # + `String#split` round-trip the gem uses.
  get "/headers_dump" do
    response["Link"] = '</page/1>; rel="prev",</page/3>; rel="next"'
    link_headers
  end

  get "/" do
    "backend: #{SERVER_BACKEND}"
  end
end

HARNESS_RUN_APP.call(LinkHeaderSmokeApp)
