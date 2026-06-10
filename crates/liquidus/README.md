# liquidus

A [Liquid](https://shopify.github.io/liquid/)-compatible template engine
in Rust. *The liquidus is the temperature line above which an alloy is
fully molten.*

liquidus targets **byte-identical output** with the Ruby `liquid` gem
(4.x) plus Jekyll's filter/tag surface, for an explicitly-bounded subset
of the language. Anything outside the implemented subset is a clean
**decline** at template-compile time (`Error::Declined`) — embedders
fall back to the pure-Ruby gem for that template, so output is never
silently wrong.

The design exploits the static nature of site templates: a template
compiles into constant segments plus typed variable slots, and the
variable paths a template needs are known statically
(`Template::variables`). Embedders supply values per render through the
`ValueSource` trait — for the rubyrs runtime that means one batched
value pull per page instead of thousands of dynamic dispatches.

Status: pre-alpha API reservation; the engine is being developed inside
the [rubyrs](https://github.com/linyiru/rubyrs) workspace alongside its
siblings [carmine](https://crates.io/crates/carmine) (rouge-compatible
highlighting) and [rostdown](https://crates.io/crates/rostdown)
(kramdown-compatible Markdown).
