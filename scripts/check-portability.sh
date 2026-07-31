#!/usr/bin/env bash
# Portability gate (FR-031, SC-011): every core crate must compile for all
# future platform targets on every commit. No shipped builds in v1 — cargo
# check only (research.md R9).
#
# Two tiers:
#   * Pure-Rust crates cross-check on every target unconditionally.
#   * Crates with C dependencies (blaze-storage → bundled SQLite, and
#     blaze-core which depends on it) additionally need a C cross-compiler.
#     Targets without one available on this host are marked "skip" — the C
#     code (SQLite) is upstream-portable; our gate is about *our* Rust code.
set -euo pipefail

TARGETS=(
  x86_64-unknown-linux-gnu
  x86_64-pc-windows-msvc
  aarch64-linux-android
  aarch64-apple-ios
)

# UI-free core crates; blaze-ffi is checked on host only (bindings are per-shell).
PURE_CRATES=(blaze-engine blaze-adblock blaze-net blaze-media)
C_DEP_CRATES=(blaze-storage blaze-core)

for target in "${TARGETS[@]}"; do
  rustup target add "$target" >/dev/null 2>&1 || true
done

# Does this host have a C cross-compiler for the target?
has_c_toolchain() {
  case "$1" in
    *-apple-*) command -v clang >/dev/null ;;                    # Apple clang multi-targets
    x86_64-unknown-linux-gnu)
      command -v x86_64-linux-gnu-gcc >/dev/null \
        || command -v x86_64-unknown-linux-gnu-gcc >/dev/null \
        || { [[ "$(uname -s)" == "Linux" ]] && command -v cc >/dev/null; } ;;
    x86_64-pc-windows-msvc) [[ "${OS:-}" == "Windows_NT" ]] ;;   # needs MSVC host
    aarch64-linux-android) [[ -n "${ANDROID_NDK_HOME:-}" ]] ;;
    *) return 1 ;;
  esac
}

fail=0
printf '%-16s' ""
for target in "${TARGETS[@]}"; do printf '%-10s' "${target%%-*}"; done
echo

check_crate() {
  local crate=$1 need_cc=$2
  printf '%-16s' "$crate"
  for target in "${TARGETS[@]}"; do
    if [[ "$need_cc" == "yes" ]] && ! has_c_toolchain "$target"; then
      printf '%-10s' "skip"
      continue
    fi
    if cargo check -p "$crate" --target "$target" --quiet >/dev/null 2>&1; then
      printf '%-10s' "ok"
    else
      printf '%-10s' "FAIL"
      fail=1
    fi
  done
  echo
}

for crate in "${PURE_CRATES[@]}"; do check_crate "$crate" no; done
for crate in "${C_DEP_CRATES[@]}"; do check_crate "$crate" yes; done

if [[ $fail -ne 0 ]]; then
  echo "Portability matrix has failures (FR-031 regression)." >&2
  exit 1
fi
echo "Portability matrix green."
