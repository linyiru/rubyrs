//! Rubund C-Extension Caching PoC.
//!
//! Unlocks the ultimate speedup for Ruby package installation:
//! 1. Downloads a real C-extension gem (`msgpack-1.7.2.gem`).
//! 2. Unpacks it to the global cache.
//! 3. Compiles the C-extension using `ruby extconf.rb && make`.
//! 4. Caches the compiled binary artifact (`msgpack.bundle` / `msgpack.so`).
//! 5. Runs again to show the **0ms** Hot-Cache C-extension installation!
//!
//! Run with: `cargo run --release -p rubund --example c_ext_cache`

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

#[cfg(unix)]
use std::os::unix::fs::symlink;

use flate2::read::GzDecoder;
use tar::Archive;

const GEM_NAME: &str = "msgpack";
const GEM_VERSION: &str = "1.7.2";
const GLOBAL_CACHE_DIR: &str = "./.rubund_global_cache";
const BIN_CACHE_DIR: &str = "./.rubund_bin_cache";
const PROJECT_GEMS_DIR: &str = "./.rubund_project_gems";

fn main() {
    println!("==================================================");
    println!("    🚀 RUBUND: C-Extension Compilation Cache 🚀   ");
    println!("==================================================");

    // Clean up directories for a fresh, honest run
    cleanup_dirs();

    // -------------------------------------------------------------------------
    // Run 1: Cold Compile (First time - download, compile from scratch, cache)
    // -------------------------------------------------------------------------
    println!("\n❄️  [Run 1] Cold Cache - Downloading & Compiling C-extension from scratch...");
    let total_cold_start = Instant::now();
    
    let downloaded_file = perform_download();
    let extracted_dir = perform_extraction(&downloaded_file);
    perform_c_extension_compilation(&extracted_dir);
    perform_linking(&extracted_dir);
    
    println!("🏆 Run 1 (Cold) Total Time: {:?}", total_cold_start.elapsed());

    // -------------------------------------------------------------------------
    // Run 2: Hot Compile (Second time - cache hit, skip compilation!)
    // -------------------------------------------------------------------------
    println!("\n🔥 [Run 2] Hot Cache - Installing again (C-extension cache hit!)...");
    let total_hot_start = Instant::now();
    
    let downloaded_file_hot = perform_download();
    let extracted_dir_hot = perform_extraction(&downloaded_file_hot);
    
    // Clean project gem directory first to prove the linking works
    let project_gem_path = Path::new(PROJECT_GEMS_DIR).join(format!("{}-{}", GEM_NAME, GEM_VERSION));
    if project_gem_path.exists() {
        fs::remove_dir_all(&project_gem_path).unwrap();
    }
    
    // Re-install by applying our cached C-extension!
    perform_c_extension_compilation(&extracted_dir_hot);
    perform_linking(&extracted_dir_hot);
    
    println!("🏆 Run 2 (Hot C-ext Cache Hit) Total Time: {:?}", total_hot_start.elapsed());
    println!("==================================================\n");

    println!("🎉 Successful! Checked:");
    println!("   └─ Extracted Gem: {}/extracted/{}-{}", GLOBAL_CACHE_DIR, GEM_NAME, GEM_VERSION);
    println!("   └─ Cached Binary: {}", BIN_CACHE_DIR);
    println!("   └─ Project Link: {}", PROJECT_GEMS_DIR);
}

// -----------------------------------------------------------------------------
// Step 1: Download the real C-extension gem using ureq
// -----------------------------------------------------------------------------
fn perform_download() -> PathBuf {
    let start_time = Instant::now();
    
    let cache_dir = Path::new(GLOBAL_CACHE_DIR);
    fs::create_dir_all(cache_dir).unwrap();
    let gem_filename = format!("{}-{}.gem", GEM_NAME, GEM_VERSION);
    let gem_path = cache_dir.join(&gem_filename);

    if gem_path.exists() {
        println!("  └─ 📥 Downloading {}... [CACHE HIT]", GEM_NAME);
        return gem_path;
    }

    println!("  └─ 📥 Downloading {} from Rubygems.org...", GEM_NAME);
    let url = format!("https://rubygems.org/downloads/{}", gem_filename);
    
    let response = ureq::get(&url).call().expect("Failed to fetch gem from rubygems.org");
    let mut reader = response.into_reader();
    let mut out_file = File::create(&gem_path).unwrap();
    std::io::copy(&mut reader, &mut out_file).unwrap();

    println!("       -> Downloaded {} in {:?}", gem_filename, start_time.elapsed());
    gem_path
}

// -----------------------------------------------------------------------------
// Step 2: Extract Gzip/Tarball
// -----------------------------------------------------------------------------
fn perform_extraction(gem_file_path: &Path) -> PathBuf {
    let start_time = Instant::now();
    
    let extracted_dir = Path::new(GLOBAL_CACHE_DIR)
        .join("extracted")
        .join(format!("{}-{}", GEM_NAME, GEM_VERSION));

    if extracted_dir.exists() {
        println!("  └─ 📦 Decompressing and Unpacking {}... [CACHE HIT]", GEM_NAME);
        return extracted_dir;
    }

    println!("  └─ 📦 Decompressing and Unpacking {} tarball...", GEM_NAME);
    fs::create_dir_all(&extracted_dir).unwrap();

    let file = File::open(gem_file_path).unwrap();
    let mut gem_archive = Archive::new(file);

    for entry_result in gem_archive.entries().unwrap() {
        let entry = entry_result.unwrap();
        let path = entry.path().unwrap();
        if path.to_str() == Some("data.tar.gz") {
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
// Step 3: Compile C-extension and cache/apply binary artifacts
// -----------------------------------------------------------------------------
fn perform_c_extension_compilation(extracted_dir: &Path) {
    let start_time = Instant::now();
    
    let ext_dir = extracted_dir.join("ext").join(GEM_NAME);
    if !ext_dir.exists() {
        println!("  └─ No C-extension folder found in gem.");
        return;
    }

    // Binary cache paths
    let bin_cache_path = Path::new(BIN_CACHE_DIR).join(format!("{}-{}", GEM_NAME, GEM_VERSION));
    fs::create_dir_all(&bin_cache_path).unwrap();
    
    // Check if we have the compiled binary in our cache
    let compiled_file_name = if cfg!(target_os = "linux") {
        "msgpack.so"
    } else if cfg!(target_os = "windows") {
        "msgpack.dll"
    } else {
        "msgpack.bundle" // default macOS
    };

    let cached_binary = bin_cache_path.join(compiled_file_name);
    let target_binary_in_gem = ext_dir.join(compiled_file_name);

    if cached_binary.exists() {
        println!("  └─ 🛠️  Compiling C-extension... [CACHE HIT]");
        println!("       -> Reusing cached binary: {}", compiled_file_name);
        
        // Copy the cached binary back into the gem's build directory!
        fs::copy(&cached_binary, &target_binary_in_gem).unwrap();
        println!("       -> Applied cached binary in {:?}", start_time.elapsed());
        return;
    }

    // Cache miss: We must compile!
    println!("  └─ 🛠️  Compiling C-extension using ruby extconf.rb & make...");
    
    // 1. Run ruby extconf.rb
    let extconf_status = Command::new("ruby")
        .arg("extconf.rb")
        .current_dir(&ext_dir)
        .status()
        .expect("Failed to execute ruby extconf.rb");
    
    if !extconf_status.success() {
        panic!("ruby extconf.rb failed");
    }

    // 2. Run make
    let make_status = Command::new("make")
        .current_dir(&ext_dir)
        .status()
        .expect("Failed to execute make");
    
    if !make_status.success() {
        panic!("make failed");
    }

    // 3. Find the compiled binary and save it to our binary cache!
    if target_binary_in_gem.exists() {
        fs::copy(&target_binary_in_gem, &cached_binary).unwrap();
        println!("       -> Saved compiled binary to cache: {}", compiled_file_name);
    } else {
        panic!("Could not find compiled binary in {}", target_binary_in_gem.display());
    }

    println!("       -> Compiled and cached in {:?}", start_time.elapsed());
}

// -----------------------------------------------------------------------------
// Step 4: Instant Symlinking
// -----------------------------------------------------------------------------
fn perform_linking(extracted_dir: &Path) {
    let start_time = Instant::now();
    
    let project_gems_dir = Path::new(PROJECT_GEMS_DIR);
    fs::create_dir_all(project_gems_dir).unwrap();
    
    let dest_link = project_gems_dir.join(format!("{}-{}", GEM_NAME, GEM_VERSION));

    println!("  └─ 🔗 Linking cache files into project folder...");
    
    #[cfg(unix)]
    {
        let abs_extracted = fs::canonicalize(extracted_dir).unwrap();
        symlink(&abs_extracted, &dest_link).expect("Failed to create symlink");
    }
    
    #[cfg(windows)]
    {
        let abs_extracted = fs::canonicalize(extracted_dir).unwrap();
        std::os::windows::fs::symlink_dir(&abs_extracted, &dest_link).expect("Failed to create symlink");
    }

    println!("       -> Linked instantly in {:?}", start_time.elapsed());
}

// -----------------------------------------------------------------------------
// Utilities
// -----------------------------------------------------------------------------
fn cleanup_dirs() {
    let global = Path::new(GLOBAL_CACHE_DIR);
    let bin = Path::new(BIN_CACHE_DIR);
    let project = Path::new(PROJECT_GEMS_DIR);
    if global.exists() {
        let _ = fs::remove_dir_all(global);
    }
    if bin.exists() {
        let _ = fs::remove_dir_all(bin);
    }
    if project.exists() {
        let _ = fs::remove_dir_all(project);
    }
}
