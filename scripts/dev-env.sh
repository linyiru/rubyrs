#!/usr/bin/env bash
# scripts/dev-env.sh — the environment the repo's pins assume.
#
# Source it (`. scripts/dev-env.sh`); scripts/doctor.sh and the Claude Code
# SessionStart hook in .claude/settings.json source it for you. It only sets
# variables, never fails, and prints nothing unless RUBYRS_DEV_ENV_VERBOSE=1.
#
# Why this exists: three facts about a dev machine silently turn a correct
# build into "tests fail" or "clippy is red", and none of them was visible
# from the repo before:
#
#   1. rust-toolchain.toml pins 1.95, but it is only honoured by rustup's
#      proxy binaries. A Homebrew / distro `cargo` earlier on PATH ignores
#      the pin, and a newer clippy then raises lints in untouched code.
#      -> put the rustup proxies first on PATH when they exist.
#   2. Release builds of this workspace are 5-8 GB per worktree. On a host
#      whose root disk is small, a few parallel agent worktrees hit ENOSPC,
#      which breaks every tool call in every session on that machine.
#      -> when a scratch root is configured, give each worktree its own
#         CARGO_TARGET_DIR under it (per-worktree, not shared: concurrent
#         cargo runs serialise on a shared target dir's build lock).
#   3. TMPDIR follows the same root, for the same reason.
#
# Host-specific values do not belong in the repo. Put them in
#   ${XDG_CONFIG_HOME:-$HOME/.config}/rubyrs/env
# (a plain shell file, sourced below if present). Recognised variables:
#   RUBYRS_TARGET_ROOT   directory holding one target dir per worktree
#   RUBYRS_NO_SCCACHE=1  do not set RUSTC_WRAPPER even if sccache is installed
#
# The CRuby oracle version is pinned separately by .ruby-version (rbenv,
# chruby and asdf read it from the checkout); nothing to do here.

# shellcheck disable=SC2034  # exported variables are the product
_rubyrs_dev_env() {
  local repo_root here cfg rustup_bin tag
  here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
  repo_root="$(cd "$here/.." && pwd)"

  cfg="${XDG_CONFIG_HOME:-$HOME/.config}/rubyrs/env"
  if [[ -r "$cfg" ]]; then
    # shellcheck source=/dev/null
    . "$cfg"
  fi

  # 1. rustup proxies first, so rust-toolchain.toml applies.
  rustup_bin="${CARGO_HOME:-$HOME/.cargo}/bin"
  if [[ -x "$rustup_bin/rustup" && -x "$rustup_bin/cargo" ]]; then
    case ":$PATH:" in
      "$rustup_bin:"*) ;;                         # already first
      *) PATH="$rustup_bin:$PATH"; export PATH ;;
    esac
  fi

  # 2./3. Build artifacts and temp files off the root disk.
  if [[ -n "${RUBYRS_TARGET_ROOT:-}" && -z "${CARGO_TARGET_DIR:-}" ]]; then
    # <basename>-<8 hex of the path>: unique per checkout, still readable.
    # A `.source` file inside names the checkout so stale dirs can be
    # mapped back (scripts/gc-worktrees.sh --target-root uses it).
    tag="$(printf '%s' "$repo_root" | shasum | cut -c1-8)"
    CARGO_TARGET_DIR="$RUBYRS_TARGET_ROOT/$(basename "$repo_root")-$tag"
    export CARGO_TARGET_DIR
    if mkdir -p "$CARGO_TARGET_DIR" 2>/dev/null; then
      [[ -f "$CARGO_TARGET_DIR/.source" ]] || printf '%s\n' "$repo_root" > "$CARGO_TARGET_DIR/.source"
    fi
    if mkdir -p "$RUBYRS_TARGET_ROOT/tmp" 2>/dev/null; then
      TMPDIR="$RUBYRS_TARGET_ROOT/tmp"; export TMPDIR
    fi
  fi

  # Optional compiler cache. Dependencies (vendored Prism C, Cranelift) are
  # otherwise rebuilt once per worktree.
  if [[ -z "${RUBYRS_NO_SCCACHE:-}" && -z "${RUSTC_WRAPPER:-}" ]] && command -v sccache >/dev/null 2>&1; then
    RUSTC_WRAPPER="$(command -v sccache)"; export RUSTC_WRAPPER
  fi

  if [[ -n "${RUBYRS_DEV_ENV_VERBOSE:-}" ]]; then
    echo "dev-env: PATH head        = ${PATH%%:*}"
    echo "dev-env: CARGO_TARGET_DIR = ${CARGO_TARGET_DIR:-<in-tree target/>}"
    echo "dev-env: TMPDIR           = ${TMPDIR:-<default>}"
    echo "dev-env: RUSTC_WRAPPER    = ${RUSTC_WRAPPER:-<none>}"
  fi
}
_rubyrs_dev_env
unset -f _rubyrs_dev_env
