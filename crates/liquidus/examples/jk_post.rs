//! Dev harness: render the liquid-1k bench post-112 through the real
//! bench templates and compare against the CRuby-built page.
//! Usage: jk_post <site_dir> <expected_html> <content_html> [posts_spec]
use liquidus::{LValue, SiteConfig, Values, compile};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let site = &args[1];
    let expected = std::fs::read_to_string(&args[2]).expect("expected html");
    let content = std::fs::read_to_string(&args[3]).expect("content html");

    let include = |name: &str| std::fs::read_to_string(format!("{site}/_includes/{name}")).ok();
    let cfg = SiteConfig {
        baseurl: String::new(),
    };

    let post_src =
        strip_front_matter(&std::fs::read_to_string(format!("{site}/_layouts/post.html")).unwrap());
    let default_src = strip_front_matter(
        &std::fs::read_to_string(format!("{site}/_layouts/default.html")).unwrap(),
    );

    let post_tpl = compile(&post_src, cfg.clone(), &include).expect("compile post");
    let default_tpl = compile(&default_src, cfg, &include).expect("compile default");
    println!("post needs: {:?}", post_tpl.variables());
    println!("default needs: {:?}", default_tpl.variables());

    let mut v = Values::default();
    v.0.insert("page.title".into(), LValue::Str("Post 112".into()));
    v.0.insert(
        "page.url".into(),
        LValue::Str("/2026/06/01/post-112.html".into()),
    );
    v.0.insert(
        "page.date".into(),
        LValue::Time {
            sec: 1_780_315_200,
            local: true,
        },
    );
    v.0.insert(
        "page.tags".into(),
        LValue::Array(vec![LValue::Str("a".into()), LValue::Str("b".into())]),
    );
    v.0.insert("content".into(), LValue::Str(content.clone()));

    let inner = post_tpl.render(&v).expect("render post layout");

    let mut v2 = Values::default();
    v2.0.insert("page.title".into(), LValue::Str("Post 112".into()));
    v2.0.insert("site.title".into(), LValue::Str("Liquid Bench".into()));
    v2.0.insert(
        "site.description".into(),
        LValue::Str("layouts + includes benchmark".into()),
    );
    v2.0.insert("content".into(), LValue::Str(inner));
    let posts: Vec<LValue> = std::env::args()
        .nth(4)
        .map(|spec| {
            spec.split(';')
                .map(|pair| {
                    let (url, title) = pair.split_once('|').unwrap();
                    LValue::Map(vec![
                        ("url".into(), LValue::Str(url.into())),
                        ("title".into(), LValue::Str(title.into())),
                    ])
                })
                .collect()
        })
        .unwrap_or_default();
    v2.0.insert("site.posts".into(), LValue::Array(posts));
    v2.0.insert("site.posts#size".into(), LValue::Int(323));

    let html = default_tpl.render(&v2).expect("render default layout");
    if html == expected {
        println!("BYTE-IDENTICAL");
    } else {
        println!("MISMATCH");
        for (i, (a, b)) in html.lines().zip(expected.lines()).enumerate() {
            if a != b {
                println!("line {}:\n  ours:     {a}\n  expected: {b}", i + 1);
            }
        }
        if html.lines().count() != expected.lines().count() {
            println!(
                "line count: ours {} vs expected {}",
                html.lines().count(),
                expected.lines().count()
            );
        }
        std::process::exit(1);
    }
}

fn strip_front_matter(s: &str) -> String {
    if let Some(rest) = s.strip_prefix("---\n")
        && let Some(end) = rest.find("\n---\n")
    {
        return rest[end + 5..].to_string();
    }
    s.to_string()
}
