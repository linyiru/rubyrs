//! Snapshot (VM image) regression: PENDING `autoload` registrations must
//! survive a save/restore cycle.
//!
//! RuboCop registers every formatter + corrector via `autoload` inside a
//! module body (lib/rubocop/formatter.rb, lib/rubocop/cop/correctors.rb),
//! and most never fire during `require "rubocop"` — so at save time those
//! constants exist ONLY as entries in the VM-level autoload tables
//! (`autoloads_scoped` / `autoloads_toplevel`), not in any class's consts
//! table. An image that skips those tables silently drops them:
//! `RuboCop::Formatter.constants` fell 22 → 2 across a restore and a
//! restored rubocop run crashed with "uninitialized constant
//! RuboCop::Formatter::SimpleTextFormatter".
//!
//! Gem-free repro: register a scoped + a toplevel autoload (unfired), save
//! via RUBYRS_SNAPSHOT_SAVE, then in a fresh process RUBYRS_SNAPSHOT_LOAD
//! and (a) list the module's constants — the pending name must still be
//! visible — and (b) reference both constants so the autoloads FIRE
//! post-restore.

#![cfg(feature = "preamble-cache")]

use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn tmp() -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
}

#[test]
fn pending_autoloads_survive_snapshot_restore() {
    let dir = tmp();
    let lib = dir.join("snap_autoload_target.rb");
    let saver = dir.join("snap_autoload_saver.rb");
    let prober = dir.join("snap_autoload_prober.rb");
    let img = dir.join("snap_autoload.img");
    fs::write(
        &lib,
        "module SnapAutoloadNS\n\
         \x20 class Fmt; def self.greet; 'fmt via autoload'; end; end\n\
         end\n\
         class SnapAutoloadTop; def self.greet; 'top via autoload'; end; end\n",
    )
    .unwrap();
    let lib_stem = lib.with_extension("");
    // Saver: register but DO NOT fire — the rubocop shape (a module body
    // full of `autoload :X, path` lines, snapshotted before any fires).
    fs::write(
        &saver,
        format!(
            "module SnapAutoloadNS\n\
             \x20 autoload :Fmt, {stem:?}\n\
             end\n\
             autoload(:SnapAutoloadTop, {stem:?})\n\
             puts SnapAutoloadNS.constants.inspect\n",
            stem = lib_stem.display(),
        ),
    )
    .unwrap();
    // Prober: the same constants view + both references must work from the
    // RESTORED image (listing must show the pending name; referencing must
    // fire the require).
    fs::write(
        &prober,
        "puts SnapAutoloadNS.constants.inspect\n\
         puts SnapAutoloadNS::Fmt.greet\n\
         puts SnapAutoloadTop.greet\n",
    )
    .unwrap();

    let rubyrs = env!("CARGO_BIN_EXE_rubyrs");
    let save = Command::new(rubyrs)
        .env("RUBYRS_SNAPSHOT_SAVE", &img)
        .arg(&saver)
        .output()
        .expect("failed to spawn rubyrs (save)");
    assert!(
        save.status.success(),
        "save run failed.\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&save.stdout),
        String::from_utf8_lossy(&save.stderr),
    );
    assert_eq!(
        String::from_utf8_lossy(&save.stdout).trim(),
        "[:Fmt]",
        "pending scoped autoload not visible in constants at save time"
    );

    let load = Command::new(rubyrs)
        .env("RUBYRS_SNAPSHOT_LOAD", &img)
        .env("RUBYRS_SNAPSHOT_DEBUG", "1")
        .arg(&prober)
        .output()
        .expect("failed to spawn rubyrs (load)");
    let stdout = String::from_utf8_lossy(&load.stdout);
    let stderr = String::from_utf8_lossy(&load.stderr);
    assert!(
        !stderr.contains("snapshot rejected"),
        "image was rejected, not restored:\n{stderr}"
    );
    assert!(
        load.status.success(),
        "restored run failed.\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(
        stdout.trim(),
        "[:Fmt]\nfmt via autoload\ntop via autoload",
        "pending autoloads dropped across snapshot restore"
    );
}
