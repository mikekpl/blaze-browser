#!/usr/bin/env bash
# Automatic per-build semantic version. Stateless: derived from Cargo.toml,
# git and the clock, so nothing tracked changes when you build.
#
#   MAJOR.MINOR.PATCH+TIMESTAMP.gSHA[.dirty]      e.g. 0.7.3+20260919131205.g58b3605
#
#   MAJOR.MINOR  [workspace.package] version in Cargo.toml — bump it by hand
#                for a release line.
#   PATCH        Cargo.toml's patch + commits since that version was set, so
#                every commit is a new, higher version.
#   +metadata    UTC build time, commit, and "dirty" for uncommitted changes:
#                unique for every build, even two from the same tree.
#
#   scripts/version.sh                  print the full semver
#   scripts/version.sh --env            print BLAZE_VERSION / BLAZE_BUILD / BLAZE_SEMVER
#   scripts/version.sh --stamp PLIST    write the version into a built Info.plist
#                                       (the Xcode "Stamp build version" phase)
set -euo pipefail
cd "$(dirname "$0")/.."

cargo_version() { grep -m1 '^version' | sed 's/.*"\(.*\)"/\1/'; }

BASE=$(cargo_version < Cargo.toml)
IFS=. read -r MAJOR MINOR PATCH <<< "$BASE"

COMMITS=0
SHA=""
DIRTY=""
if git rev-parse --git-dir >/dev/null 2>&1; then
  SHA=$(git rev-parse --short=7 HEAD 2>/dev/null || true)
  [[ -n "$(git status --porcelain 2>/dev/null)" ]] && DIRTY=1
  # Oldest commit of the unbroken run (from HEAD back) in which Cargo.toml
  # already carried this base version; an uncommitted bump counts from zero.
  ANCHOR=""
  if [[ "$(git show HEAD:Cargo.toml 2>/dev/null | cargo_version)" == "$BASE" ]]; then
    while read -r commit; do
      [[ "$(git show "$commit:Cargo.toml" | cargo_version)" == "$BASE" ]] || break
      ANCHOR=$commit
    done < <(git log --format=%H -- Cargo.toml)
  fi
  [[ -n "$ANCHOR" ]] && COMMITS=$(git rev-list --count "$ANCHOR..HEAD")
fi

# One timestamp per build: callers that stamp several artifacts export it.
STAMP=${BLAZE_BUILD_TIMESTAMP:-$(date -u +%Y%m%d%H%M%S)}

BLAZE_VERSION="$MAJOR.$MINOR.$((PATCH + COMMITS))"
# CFBundleVersion allows at most three dot-separated integers.
BLAZE_BUILD="${STAMP:0:8}.${STAMP:8}"
BLAZE_SEMVER="$BLAZE_VERSION+$STAMP${SHA:+.g$SHA}${DIRTY:+.dirty}"

case "${1:-}" in
  "") echo "$BLAZE_SEMVER" ;;
  --env)
    echo "BLAZE_VERSION=$BLAZE_VERSION"
    echo "BLAZE_BUILD=$BLAZE_BUILD"
    echo "BLAZE_SEMVER=$BLAZE_SEMVER"
    ;;
  --stamp)
    PLIST=${2:?usage: version.sh --stamp <Info.plist>}
    [[ -f "$PLIST" ]] || { echo "version.sh: no Info.plist at $PLIST" >&2; exit 1; }
    set_key() {
      /usr/libexec/PlistBuddy -c "Set :$1 $2" "$PLIST" 2>/dev/null \
        || /usr/libexec/PlistBuddy -c "Add :$1 string $2" "$PLIST"
    }
    set_key CFBundleShortVersionString "$BLAZE_VERSION"
    set_key CFBundleVersion "$BLAZE_BUILD"
    set_key BlazeSemVer "$BLAZE_SEMVER"
    echo "Blaze $BLAZE_SEMVER"
    ;;
  *) echo "unknown argument: $1" >&2; exit 2 ;;
esac
