//! Integration tests for the zero-copy Lockfile parser.

use rubund::parser::{parse_lockfile, SourceType};

#[test]
fn test_case_1_standard_gem() {
    let case1 = r#"GEM
  remote: https://rubygems.org/
  specs:
    aasm (5.1.1)
      concurrent-ruby (~> 1.0)
    concurrent-ruby (1.3.4)

PLATFORMS
  ruby

DEPENDENCIES
  aasm (~> 5.1.1)

BUNDLED WITH
   2.5.11
"#;

    let lock = parse_lockfile(case1);
    
    assert_eq!(lock.sources.len(), 1);
    assert_eq!(lock.sources[0].type_, SourceType::Gem);
    assert_eq!(lock.sources[0].remote, "https://rubygems.org/");
    
    assert_eq!(lock.specs.len(), 2);
    assert_eq!(lock.specs[0].name, "aasm");
    assert_eq!(lock.specs[0].version, "5.1.1");
    assert_eq!(lock.specs[0].dependencies.len(), 1);
    assert_eq!(lock.specs[0].dependencies[0], ("concurrent-ruby", Some("~> 1.0")));
    
    assert_eq!(lock.platforms, vec!["ruby"]);
    assert_eq!(lock.dependencies.len(), 1);
    assert_eq!(lock.dependencies[0], ("aasm", Some("~> 5.1.1")));
    assert_eq!(lock.checksums.len(), 0);
    assert_eq!(lock.ruby_version, None);
    assert_eq!(lock.bundled_with, Some("2.5.11"));
}

#[test]
fn test_case_2_git_pinned() {
    let case2 = r#"GIT
  remote: https://github.com/kaikhq/omniauth-line.git
  revision: 9fa44e7c3b88b2b
  branch: master
  specs:
    omniauth-line (1.0.0)
      omniauth (~> 2.1)
      omniauth-oauth2 (~> 1.8)

PLATFORMS
  ruby

DEPENDENCIES
  omniauth-line!
"#;

    let lock = parse_lockfile(case2);
    
    assert_eq!(lock.sources.len(), 1);
    assert_eq!(lock.sources[0].type_, SourceType::Git);
    assert_eq!(lock.sources[0].remote, "https://github.com/kaikhq/omniauth-line.git");
    assert_eq!(lock.sources[0].revision, Some("9fa44e7c3b88b2b"));
    assert_eq!(lock.sources[0].branch, Some("master"));
    
    assert_eq!(lock.specs.len(), 1);
    assert_eq!(lock.specs[0].name, "omniauth-line");
    assert_eq!(lock.specs[0].version, "1.0.0");
    assert_eq!(lock.specs[0].dependencies.len(), 2);
    assert_eq!(lock.specs[0].dependencies[0], ("omniauth", Some("~> 2.1")));
    
    assert_eq!(lock.dependencies.len(), 1);
    assert_eq!(lock.dependencies[0], ("omniauth-line", None));
}

#[test]
fn test_case_3_multi_source() {
    let case3 = r#"GEM
  remote: https://rubygems.org/
  specs:
    activesupport (7.2.0)
      concurrent-ruby (>= 1.0.2)
      i18n (>= 1.6, < 2)

GIT
  remote: https://github.com/banister/binding_of_caller.git
  revision: a7e3b1c2b3d4e5f
  specs:
    binding_of_caller (0.8.0)

PLATFORMS
  arm64-darwin-23
  x86_64-linux

DEPENDENCIES
  activesupport
  binding_of_caller!

BUNDLED WITH
   2.6.2
"#;

    let lock = parse_lockfile(case3);
    
    assert_eq!(lock.sources.len(), 2);
    assert_eq!(lock.sources[0].type_, SourceType::Gem);
    assert_eq!(lock.sources[1].type_, SourceType::Git);
    assert_eq!(lock.sources[1].remote, "https://github.com/banister/binding_of_caller.git");
    assert_eq!(lock.sources[1].revision, Some("a7e3b1c2b3d4e5f"));
    
    assert_eq!(lock.specs.len(), 2);
    assert_eq!(lock.specs[0].name, "activesupport");
    assert_eq!(lock.specs[1].name, "binding_of_caller");
    assert_eq!(lock.specs[0].source_index, 0);
    assert_eq!(lock.specs[1].source_index, 1);
    
    assert_eq!(lock.platforms, vec!["arm64-darwin-23", "x86_64-linux"]);
    assert_eq!(lock.dependencies.len(), 2);
    assert_eq!(lock.dependencies[0], ("activesupport", None));
    assert_eq!(lock.dependencies[1], ("binding_of_caller", None));
    assert_eq!(lock.bundled_with, Some("2.6.2"));
}

#[test]
fn test_case_4_path_source() {
    let case4 = r#"PATH
  remote: ../my_local_gem
  specs:
    my_local_gem (0.1.0)
      activesupport (>= 6.0)

GEM
  remote: https://rubygems.org/
  specs:
    activesupport (7.2.0)

PLATFORMS
  ruby

DEPENDENCIES
  my_local_gem!

BUNDLED WITH
   2.5.11
"#;

    let lock = parse_lockfile(case4);

    assert_eq!(lock.sources.len(), 2);
    assert_eq!(lock.sources[0].type_, SourceType::Path);
    assert_eq!(lock.sources[0].remote, "../my_local_gem");
    assert_eq!(lock.sources[1].type_, SourceType::Gem);
    assert_eq!(lock.sources[1].remote, "https://rubygems.org/");

    assert_eq!(lock.specs.len(), 2);
    assert_eq!(lock.specs[0].name, "my_local_gem");
    assert_eq!(lock.specs[0].version, "0.1.0");
    assert_eq!(lock.specs[0].source_index, 0);
    assert_eq!(lock.specs[0].dependencies.len(), 1);
    assert_eq!(lock.specs[0].dependencies[0], ("activesupport", Some(">= 6.0")));
    assert_eq!(lock.specs[1].name, "activesupport");
    assert_eq!(lock.specs[1].source_index, 1);

    assert_eq!(lock.dependencies.len(), 1);
    assert_eq!(lock.dependencies[0], ("my_local_gem", None));
    assert_eq!(lock.bundled_with, Some("2.5.11"));
}

#[test]
fn test_case_5_official_rspec_vector() {
    let case5 = r#"GIT
  remote: https://github.com/alloy/peiji-san.git
  revision: eca485d8dc95f12aaec1a434b49d295c7e91844b
  specs:
    peiji-san (1.2.0)

GEM
  remote: https://rubygems.org/
  specs:
    rake (10.3.2)

PLATFORMS
  ruby

DEPENDENCIES
  peiji-san!
  rake

CHECKSUMS
  rake (10.3.2) sha256=814828c34f1315d7e7b7e8295184577cc4e969bad6156ac069d02d63f58d82e8

RUBY VERSION
   ruby 2.1.3p242

BUNDLED WITH
   1.12.0.rc.2
"#;

    let lock = parse_lockfile(case5);

    assert_eq!(lock.sources.len(), 2);
    assert_eq!(lock.sources[0].type_, SourceType::Git);
    assert_eq!(lock.sources[0].remote, "https://github.com/alloy/peiji-san.git");
    assert_eq!(lock.sources[0].revision, Some("eca485d8dc95f12aaec1a434b49d295c7e91844b"));

    assert_eq!(lock.sources[1].type_, SourceType::Gem);
    assert_eq!(lock.sources[1].remote, "https://rubygems.org/");

    assert_eq!(lock.specs.len(), 2);
    assert_eq!(lock.specs[0].name, "peiji-san");
    assert_eq!(lock.specs[0].version, "1.2.0");
    assert_eq!(lock.specs[1].name, "rake");
    assert_eq!(lock.specs[1].version, "10.3.2");

    assert_eq!(lock.platforms, vec!["ruby"]);
    
    assert_eq!(lock.dependencies.len(), 2);
    assert_eq!(lock.dependencies[0], ("peiji-san", None));
    assert_eq!(lock.dependencies[1], ("rake", None));

    assert_eq!(lock.checksums.len(), 1);
    assert_eq!(lock.checksums[0], ("rake", "10.3.2", "814828c34f1315d7e7b7e8295184577cc4e969bad6156ac069d02d63f58d82e8"));

    assert_eq!(lock.ruby_version, Some("ruby 2.1.3p242"));
    assert_eq!(lock.bundled_with, Some("1.12.0.rc.2"));
}
