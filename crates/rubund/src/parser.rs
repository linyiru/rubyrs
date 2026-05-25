//! Zero-Copy Gemfile.lock Parser Module.

// -----------------------------------------------------------------------------
// Data Structures
// -----------------------------------------------------------------------------

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum SourceType {
    Gem,
    Git,
    Path,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct GemSource<'a> {
    pub type_: SourceType,
    pub remote: &'a str,
    pub revision: Option<&'a str>,
    pub branch: Option<&'a str>,
    pub path: Option<&'a str>,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct GemSpec<'a> {
    pub name: &'a str,
    pub version: &'a str,
    pub dependencies: Vec<(&'a str, Option<&'a str>)>,
    pub source_index: usize, // Pointer to the index in Lockfile::sources
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Lockfile<'a> {
    pub sources: Vec<GemSource<'a>>,
    pub specs: Vec<GemSpec<'a>>,
    pub platforms: Vec<&'a str>,
    pub dependencies: Vec<(&'a str, Option<&'a str>)>,
    pub checksums: Vec<(&'a str, &'a str, &'a str)>, // (name, version, sha256)
    pub ruby_version: Option<&'a str>,
    pub bundled_with: Option<&'a str>,
}

// -----------------------------------------------------------------------------
// The Zero-Copy State-Machine Parser
// -----------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SectionState {
    None,
    GemSection,
    GitSection,
    PathSection,
    PlatformsSection,
    DependenciesSection,
    ChecksumsSection,
    RubyVersionSection,
    BundledWithSection,
}

pub fn parse_lockfile(content: &str) -> Lockfile<'_> {
    let mut sources = Vec::new();
    let mut specs = Vec::new();
    let mut platforms = Vec::new();
    let mut dependencies = Vec::new();
    let mut checksums = Vec::new();
    let mut ruby_version = None;
    let mut bundled_with = None;

    let mut current_section = SectionState::None;
    
    // Auxiliary trackers for current source configuration
    let mut current_remote = "";
    let mut current_revision = None;
    let mut current_branch = None;
    let mut current_path = None;
    
    // Double-check if we are currently parsing specs inside a source block
    let mut parsing_specs = false;

    for line in content.lines() {
        let trimmed = line.trim_start();
        if trimmed.is_empty() {
            continue;
        }
        let indent = line.len() - trimmed.len();

        // 1. Detect and switch top-level section headers (indent == 0)
        if indent == 0 {
            // If we are leaving a GEM/GIT/PATH section, save the accumulated source
            if matches!(current_section, SectionState::GemSection | SectionState::GitSection | SectionState::PathSection) {
                let type_ = match current_section {
                    SectionState::GemSection => SourceType::Gem,
                    SectionState::GitSection => SourceType::Git,
                    _ => SourceType::Path,
                };
                sources.push(GemSource {
                    type_,
                    remote: current_remote,
                    revision: current_revision.take(),
                    branch: current_branch.take(),
                    path: current_path.take(),
                });
                current_remote = "";
                parsing_specs = false;
            }

            current_section = match trimmed {
                "GEM" => SectionState::GemSection,
                "GIT" => SectionState::GitSection,
                "PATH" => SectionState::PathSection,
                "PLATFORMS" => SectionState::PlatformsSection,
                "DEPENDENCIES" => SectionState::DependenciesSection,
                "CHECKSUMS" => SectionState::ChecksumsSection,
                "RUBY VERSION" => SectionState::RubyVersionSection,
                "BUNDLED WITH" => SectionState::BundledWithSection,
                _ => SectionState::None,
            };
            continue;
        }

        // 2. State-Machine Line Processing
        match current_section {
            SectionState::GemSection | SectionState::GitSection | SectionState::PathSection => {
                if indent == 2 {
                    if let Some(rest) = trimmed.strip_prefix("remote:") {
                        current_remote = rest.trim();
                    } else if let Some(rest) = trimmed.strip_prefix("revision:") {
                        current_revision = Some(rest.trim());
                    } else if let Some(rest) = trimmed.strip_prefix("branch:") {
                        current_branch = Some(rest.trim());
                    } else if let Some(rest) = trimmed.strip_prefix("path:") {
                        current_path = Some(rest.trim());
                    } else if trimmed == "specs:" {
                        parsing_specs = true;
                    }
                } else if parsing_specs {
                    if indent == 4 {
                        // A gem spec definition: e.g. "    aasm (5.1.1)"
                        if let Some((name_part, rest)) = trimmed.split_once(' ') {
                            let version_part = rest.trim_matches(|c| c == '(' || c == ')' || c == ' ');
                            
                            // The current source index will be the index of the source we are building
                            let source_index = sources.len();

                            specs.push(GemSpec {
                                name: name_part.trim(),
                                version: version_part,
                                dependencies: Vec::new(),
                                source_index,
                            });
                        }
                    } else if indent == 6 {
                        // A dependency of the last parsed spec: e.g. "      concurrent-ruby (~> 1.0)"
                        if let Some(last_spec) = specs.last_mut() {
                            let (dep_name, dep_constraint) = match trimmed.split_once(' ') {
                                Some((name, rest)) => {
                                    let constraint = rest.trim_matches(|c| c == '(' || c == ')' || c == ' ');
                                    (name.trim(), Some(constraint))
                                }
                                None => (trimmed.trim(), None),
                            };
                            last_spec.dependencies.push((dep_name, dep_constraint));
                        }
                    }
                }
            }
            SectionState::PlatformsSection => {
                if indent == 2 {
                    platforms.push(trimmed.trim());
                }
            }
            SectionState::DependenciesSection => {
                if indent == 2 {
                    // Dependency lines e.g. "  aasm (~> 5.1.1)" or "  omniauth-line!"
                    let cleaned = trimmed.trim_end_matches('!');
                    let (name, constraint) = match cleaned.split_once(' ') {
                        Some((name, rest)) => {
                            let constraint = rest.trim_matches(|c| c == '(' || c == ')' || c == ' ');
                            (name.trim(), Some(constraint))
                        }
                        None => (cleaned.trim(), None),
                    };
                    dependencies.push((name, constraint));
                }
            }
            SectionState::ChecksumsSection => {
                if indent == 2 {
                    // Checksum lines e.g. "  rake (10.3.2) sha256=814828c34f1315d7..."
                    if let Some((name_part, rest)) = trimmed.split_once(' ') {
                        let version_and_sha = rest.trim();
                        if let Some((version, sha_part)) = version_and_sha.split_once(' ') {
                            let clean_version = version.trim_matches(|c| c == '(' || c == ')' || c == ' ');
                            let sha = sha_part.trim_start_matches("sha256=");
                            checksums.push((name_part.trim(), clean_version, sha.trim()));
                        }
                    }
                }
            }
            SectionState::RubyVersionSection => {
                ruby_version = Some(trimmed.trim());
            }
            SectionState::BundledWithSection => {
                bundled_with = Some(trimmed.trim());
            }
            SectionState::None => {}
        }
    }

    // Capture the final source block if we hit EOF while parsing one
    if matches!(current_section, SectionState::GemSection | SectionState::GitSection | SectionState::PathSection) {
        let type_ = match current_section {
            SectionState::GemSection => SourceType::Gem,
            SectionState::GitSection => SourceType::Git,
            _ => SourceType::Path,
        };
        sources.push(GemSource {
            type_,
            remote: current_remote,
            revision: current_revision,
            branch: current_branch,
            path: current_path,
        });
    }

    Lockfile {
        sources,
        specs,
        platforms,
        dependencies,
        checksums,
        ruby_version,
        bundled_with,
    }
}
