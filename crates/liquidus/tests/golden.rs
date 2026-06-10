//! Self-contained golden tests: the liquid-1k bench templates (the
//! shapes a typical Jekyll blog uses) rendered against values captured
//! from the real site, with expectations captured from CRuby
//! liquid 4.0.4 + Jekyll 4.4 filters under TZ=UTC. The full-site
//! byte-diff lives in the rubyrs `_liquid_native` validation; these
//! pin the engine semantics crate-locally.

use liquidus::{Error, LValue, SiteConfig, Values, compile};

fn no_includes(_: &str) -> Option<String> {
    None
}

fn s(v: &str) -> LValue {
    LValue::Str(v.to_string())
}

#[test]
fn output_variables_and_filters() {
    let tpl = compile(
        "<h1>{{ page.title | escape }}</h1>\n<p>{{ content | number_of_words }} words · {{ page.url }}</p>\n",
        SiteConfig::default(),
        &no_includes,
    )
    .unwrap();
    let mut v = Values::default();
    v.0.insert("page.title".into(), s("Q&A <guide> 'n stuff"));
    v.0.insert("page.url".into(), s("/2026/06/01/post-1.html"));
    v.0.insert("content".into(), s("one two  three\nfour"));
    assert_eq!(
        tpl.render(&v).unwrap(),
        "<h1>Q&amp;A &lt;guide&gt; &#39;n stuff</h1>\n<p>4 words · /2026/06/01/post-1.html</p>\n"
    );
}

#[test]
fn date_filters() {
    let tpl = compile(
        r#"<time datetime="{{ page.date | date_to_xmlschema }}">{{ page.date | date: "%B %-d, %Y" }}</time>"#,
        SiteConfig::default(),
        &no_includes,
    )
    .unwrap();
    let mut v = Values::default();
    // 2026-06-01 12:00:00 UTC — expectation from TZ=UTC CRuby:
    // xmlschema renders the LOCAL flavour as +00:00.
    v.0.insert(
        "page.date".into(),
        LValue::Time {
            sec: 1_780_315_200,
            local: true,
        },
    );
    assert_eq!(
        tpl.render(&v).unwrap(),
        r#"<time datetime="2026-06-01T12:00:00+00:00">June 1, 2026</time>"#
    );
}

#[test]
fn if_and_for_with_loop_filters() {
    let tpl = compile(
        "{% if page.tags.size > 0 %}<ul>{% for tag in page.tags %}<li class=\"tag-{{ tag | slugify }}\">{{ tag | upcase }}</li>{% endfor %}</ul>{% endif %}",
        SiteConfig::default(),
        &no_includes,
    )
    .unwrap();
    let mut v = Values::default();
    v.0.insert(
        "page.tags".into(),
        LValue::Array(vec![s("Hello World"), s("b")]),
    );
    assert_eq!(
        tpl.render(&v).unwrap(),
        "<ul><li class=\"tag-hello-world\">HELLO WORLD</li><li class=\"tag-b\">B</li></ul>"
    );
    // empty tags: the if-guard suppresses everything
    let mut v2 = Values::default();
    v2.0.insert("page.tags".into(), LValue::Array(vec![]));
    assert_eq!(tpl.render(&v2).unwrap(), "");
}

#[test]
fn for_limit_slice_and_size_companion() {
    let tpl = compile(
        "{% for post in site.posts limit: 2 %}[{{ post.title | truncate: 10 }}]({{ post.url | relative_url }}){% endfor %} of {{ site.posts | size }}",
        SiteConfig { baseurl: "/blog".into() },
        &no_includes,
    )
    .unwrap();
    // variables() reports the slice so the embedder can supply just 2
    let need = tpl
        .variables()
        .iter()
        .find(|n| n.path == "site.posts")
        .unwrap();
    assert_eq!(need.slice, Some(2));
    assert!(need.need_size);

    let mut v = Values::default();
    v.0.insert(
        "site.posts".into(),
        LValue::Array(vec![
            LValue::Map(vec![
                ("title".into(), s("A genuinely long post title")),
                ("url".into(), s("/2026/06/09/post-988.html")),
            ]),
            LValue::Map(vec![
                ("title".into(), s("Short")),
                ("url".into(), s("/2026/06/09/post-960.html")),
            ]),
        ]),
    );
    v.0.insert("site.posts#size".into(), LValue::Int(323));
    assert_eq!(
        tpl.render(&v).unwrap(),
        "[A genui...](/blog/2026/06/09/post-988.html)[Short](/blog/2026/06/09/post-960.html) of 323"
    );
}

#[test]
fn include_expansion() {
    let include = |name: &str| {
        (name == "header.html").then(|| "<nav>{{ site.title | escape }}</nav>".to_string())
    };
    let tpl = compile(
        "{% include header.html %}<main>{{ content }}</main>",
        SiteConfig::default(),
        &include,
    )
    .unwrap();
    let mut v = Values::default();
    v.0.insert("site.title".into(), s("T&C"));
    v.0.insert("content".into(), s("body"));
    assert_eq!(
        tpl.render(&v).unwrap(),
        "<nav>T&amp;C</nav><main>body</main>"
    );
}

#[test]
fn string_literal_through_filter() {
    let tpl = compile(
        r#"<a href="{{ "/" | relative_url }}">home</a>"#,
        SiteConfig {
            baseurl: String::new(),
        },
        &no_includes,
    )
    .unwrap();
    assert_eq!(
        tpl.render(&Values::default()).unwrap(),
        r#"<a href="/">home</a>"#
    );
}

#[test]
fn declines() {
    let cfg = SiteConfig::default;
    for (src, what) in [
        ("{% assign x = 1 %}", "unsupported-tag"),
        ("{% if a and b %}x{% endif %}", "compound-condition"),
        ("{{ x | money }}", "unsupported-filter"),
        ("{{ x[0] }}", "index-access"),
        ("{{- x -}}", "whitespace-control"),
        (
            "{% for p in site.posts reversed %}{% endfor %}",
            "for-modifiers",
        ),
        ("{{ forloop.index }}", "forloop-variable"),
        ("{% include a.html param=1 %}", "include-params"),
        ("{% if x %}unclosed", "unclosed-block-tag"),
    ] {
        match compile(src, cfg(), &no_includes) {
            Err(Error::Declined(got)) => assert_eq!(got, what, "for {src:?}"),
            other => panic!("expected decline for {src:?}, got {other:?}"),
        }
    }
}

#[test]
fn render_declines_on_unreproducible_values() {
    // non-ASCII through the ASCII-exact filters
    let tpl = compile("{{ t | slugify }}", SiteConfig::default(), &no_includes).unwrap();
    let mut v = Values::default();
    v.0.insert("t".into(), s("中文標籤"));
    assert!(tpl.render(&v).is_err());
}
