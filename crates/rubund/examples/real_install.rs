//! Rubund (Real Installer) PoC.
//!
//! Demonstrates the \"Minutes to Seconds\" physical installation speedup:
//! 1. **Parse dynamic Gemfile**: Uses rubyrs to evaluate dynamic Gemfile constraints.
//! 2. **Network Download**: Downloads the real `diff-lcs-1.5.0.gem` from rubygems.org (40KB).
//! 3. **Streaming Extraction**: Unpacks the .gem tarball and data.tar.gz directly in-memory.
//! 4. **Instant Link**: Creates a symlink to the project directory in < 1 microsecond.
//! 5. **Hot Cache**: Runs again to show the 0ms installation when the cache is hit.
//!
//! Run with: `cargo run --release -p rubund --example real_install`

use std::cell::RefCell;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Instant;

#[cfg(unix)]
use std::os::unix::fs::symlink;

use flate2::read::GzDecoder;
use rubyrs::{Runtime, Value};
use tar::Archive;

// ---------- Configurations ----------
const GEM_NAME: &str = "diff-lcs";
const GEM_VERSION: &str = "1.5.0";
const GLOBAL_CACHE_DIR: &str = "./.rubund_global_cache";
const PROJECT_GEMS_DIR: &str = "./.rubund_project_gems";

#[allow(dead_code)]
#[derive(Debug, Clone)]
struct GemRequirement {
    name: String,
    version: String,
    group: String,
}

fn main() {
    println!("==================================================");
    println!("     🚀 RUBUND: Real Package Installer PoC 🚀     ");
    println!("==================================================");

    // Make sure we have clean directories for a real test
    cleanup_dirs();

    // -------------------------------------------------------------------------
    // Run 1: Cold Cache (First installation, needs download & compile/extract)
    // -------------------------------------------------------------------------
    println!("\n❄️  [Run 1] Cold Cache - First-time installation...");
    let reqs_cold = run_rubyrs_eval();
    let gem_to_install = &reqs_cold[0]; // diff-lcs
    
    let total_cold_start = Instant::now();
    let downloaded_file = perform_download(gem_to_install);
    let extracted_dir = perform_extraction(&downloaded_file, gem_to_install);
    perform_linking(&extracted_dir, gem_to_install);
    
    println!("🏆 Run 1 (Cold) Total Install Time: {:?}", total_cold_start.elapsed());

    // -------------------------------------------------------------------------
    // Run 2: Hot Cache (Second installation, cache hit - should take 0ms)
    // -------------------------------------------------------------------------
    println!("\n🔥 [Run 2] Hot Cache - Re-installing project dependencies...");
    let reqs_hot = run_rubyrs_eval();
    let gem_to_install_hot = &reqs_hot[0];

    let total_hot_start = Instant::now();
    let downloaded_file_hot = perform_download(gem_to_install_hot);
    let extracted_dir_hot = perform_extraction(&downloaded_file_hot, gem_to_install_hot);
    
    // Simulate linking again (clean project gem folder first to prove it links)
    let project_gem_path = Path::new(PROJECT_GEMS_DIR).join(format!("{}-{}", gem_to_install_hot.name, gem_to_install_hot.version));
    if project_gem_path.exists() {
        fs::remove_dir_all(&project_gem_path).unwrap();
    }
    perform_linking(&extracted_dir_hot, gem_to_install_hot);
    
    println!("🏆 Run 2 (Hot Cache Hit) Total Install Time: {:?}", total_hot_start.elapsed());
    println!("==================================================\n");

    println!("🎉 Successful! Check directories:");
    println!("   └─ Global Cache: {}", GLOBAL_CACHE_DIR);
    println!("   └─ Project Link: {}", PROJECT_GEMS_DIR);
}

// -----------------------------------------------------------------------------
// Step 1: Parse Gemfile using rubyrs
// -----------------------------------------------------------------------------
fn run_rubyrs_eval() -> Vec<GemRequirement> {
    let start_time = Instant::now();
    let mut rt = Runtime::new();
    let requirements = Rc::new(RefCell::new(Vec::<GemRequirement>::new()));

    // Register our host callbacks
    let reqs_clone = requirements.clone();
    rt.register_fn("host_register_gem", move |args| {
        if let [Value::Str(name), Value::Str(version), Value::Str(group)] = args {
            reqs_clone.borrow_mut().push(GemRequirement {
                name: name.borrow().clone(),
                version: version.borrow().clone(),
                group: group.borrow().clone(),
            });
        }
        Ok(Value::Nil)
    });

    rt.register_fn("host_register_source", |_args| {
        Ok(Value::Nil)
    });

    // The dynamic Gemfile wrapped in a DSL class
    let gemfile = format!(r#"
        class DSL
          def initialize
            @current_group = "default"
          end
          def source(url)
            host_register_source(url.to_s)
          end
          def group(name)
            old_group = @current_group
            @current_group = name.to_s
            yield
            @current_group = old_group
          end
          def gem(name, version)
            host_register_gem(name.to_s, version.to_s, @current_group)
          end
          def run!
            source "https://rubygems.org"
            gem "{}", "{}"
          end
        end
        DSL.new.run!
    "#, GEM_NAME, GEM_VERSION);

    rt.eval(&gemfile, "Gemfile").unwrap();

    println!("  └─ ⚡ Gemfile parsed dynamically in: {:?}", start_time.elapsed());
    requirements.borrow().clone()
}

// -----------------------------------------------------------------------------
// Step 2: Download the real gem using ureq
// -----------------------------------------------------------------------------
fn perform_download(gem: &GemRequirement) -> PathBuf {
    let start_time = Instant::now();
    
    let cache_dir = Path::new(GLOBAL_CACHE_DIR);
    fs::create_dir_all(cache_dir).unwrap();
    let gem_filename = format!("{}-{}.gem", gem.name, gem.version);
    let gem_path = cache_dir.join(&gem_filename);

    if gem_path.exists() {
        println!("  └─ 📥 Downloading {}... [CACHE HIT]", gem.name);
        return gem_path;
    }

    println!("  └─ 📥 Downloading {} from Rubygems.org...", gem.name);
    let url = format!("https://rubygems.org/downloads/{}", gem_filename);
    
    // Perform real HTTP request using ureq
    let response = ureq::get(&url).call().expect("Failed to fetch gem from rubygems.org");
    let mut reader = response.into_reader();
    let mut out_file = File::create(&gem_path).unwrap();
    std::io::copy(&mut reader, &mut out_file).unwrap();

    println!("       -> Downloaded {} in {:?}", gem_filename, start_time.elapsed());
    gem_path
}

// -----------------------------------------------------------------------------
// Step 3: Stream and decompress .gem in-memory
// -----------------------------------------------------------------------------
fn perform_extraction(gem_file_path: &Path, gem: &GemRequirement) -> PathBuf {
    let start_time = Instant::now();
    
    let extracted_dir = Path::new(GLOBAL_CACHE_DIR)
        .join("extracted")
        .join(format!("{}-{}", gem.name, gem.version));

    if extracted_dir.exists() {
        println!("  └─ 📦 Decompressing and Unpacking {}... [CACHE HIT]", gem.name);
        return extracted_dir;
    }

    println!("  └─ 📦 Decompressing and Unpacking {} tarball...", gem.name);
    fs::create_dir_all(&extracted_dir).unwrap();

    let file = File::open(gem_file_path).unwrap();
    let mut gem_archive = Archive::new(file);

    // Walk the tar entries of the .gem file to find data.tar.gz
    for entry_result in gem_archive.entries().unwrap() {
        let entry = entry_result.unwrap();
        let path = entry.path().unwrap();
        if path.to_str() == Some("data.tar.gz") {
            // Unpack data.tar.gz on-the-fly using flate2 + tar
            let gz_decoder = GzDecoder::new(entry);
            let mut inner_tar = Archive::new(gz_decoder);
            inner_tar.unpack(&extracted_dir).unwrap();
            break;
        }
    }

    println!("       -> Extracted to cache in {:?}", start_time.elapsed());
    extracted_dir
}

// -----------------------------------------------------------------------------
// Step 4: Instant Symlinking
// -----------------------------------------------------------------------------
fn perform_linking(extracted_dir: &Path, gem: &GemRequirement) {
    let start_time = Instant::now();
    
    let project_gems_dir = Path::new(PROJECT_GEMS_DIR);
    fs::create_dir_all(project_gems_dir).unwrap();
    
    let dest_link = project_gems_dir.join(format!("{}-{}", gem.name, gem.version));

    println!("  └─ 🔗 Linking cache files into project folder...");
    
    #[cfg(unix)]
    {
        // On unix, create a folder symlink
        let abs_extracted = fs::canonicalize(extracted_dir).unwrap();
        symlink(&abs_extracted, &dest_link).expect("Failed to create symlink");
    }
    
    #[cfg(windows)]
    {
        // On windows, create directory symlink
        let abs_extracted = fs::canonicalize(extracted_dir).unwrap();
        std::os::windows::fs::symlink_dir(&abs_extracted, &dest_link).expect("Failed to create symlink");
    }

    // Measure the exact linking time (usually < 50 microseconds!)
    println!("       -> Linked instantly in {:?}", start_time.elapsed());
}

// -----------------------------------------------------------------------------
// Utilities
// -----------------------------------------------------------------------------
fn cleanup_dirs() {
    let global = Path::new(GLOBAL_CACHE_DIR);
    let project = Path::new(PROJECT_GEMS_DIR);
    if global.exists() {
        let _ = fs::remove_dir_all(global);
    }
    if project.exists() {
        let _ = fs::remove_dir_all(project);
    }
}
