#!/usr/bin/env bash
set -euo pipefail

ZMX_URL=https://github.com/neurosnap/zmx
ZMX_REV=8bab1f0173b07e79835ea372d749af3dbf0d0842
GHOSTTY_URL=https://github.com/ghostty-org/ghostty
GHOSTTY_REV=8af6897c0afc63037a8a3efee4162a380e3a4572

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ZMX_DIR="$ROOT/zmx"
BUILD="$ZMX_DIR/.build"
OUT="$ZMX_DIR/out"

PATCHES=(
  zmx/patches/0001-explicit-leadership.patch
  zmx/patches/0002-private-ghostty-dependency.patch
  zmx/patches/0003-atomic-socket-publish.patch
  zmx/patches/ghostty/0001-formatter-unstyled-blanks.patch
)

patches_hash() {
  local file
  for file in "${PATCHES[@]}"; do
    cat "$ROOT/$file"
  done | shasum -a 256 | cut -d' ' -f1
}

fetch() {
  local url=$1 rev=$2 dir=$3
  mkdir -p "$dir"
  git -C "$dir" init -q
  git -C "$dir" remote add origin "$url"
  git -C "$dir" fetch -q --depth 1 origin "$rev"
  git -C "$dir" checkout -q FETCH_HEAD
}

apply() {
  local dir=$1 file=$2
  if ! git -C "$dir" apply --whitespace=nowarn "$ROOT/$file"; then
    echo "failed to apply $file" >&2
    exit 1
  fi
}

guard_float_h() {
  [ "$(uname -s)" = Darwin ] || return 0
  local lib_dir float_h
  lib_dir="$(zig env | sed -n 's/^ *\.lib_dir = "\(.*\)",$/\1/p')"
  float_h="$lib_dir/include/float.h"
  if [ -z "$lib_dir" ] || [ ! -f "$float_h" ]; then
    echo "zig float.h not found at '$float_h'" >&2
    exit 1
  fi
  if grep -q __need_infinity_nan "$float_h"; then
    return 0
  fi
  cp "$float_h" "$float_h.orig"
  {
    echo '/* foster: need_infinity_nan guard */'
    cat <<'EOF'
#if defined(__need_infinity_nan)
#  undef INFINITY
#  undef NAN
#  define INFINITY (__builtin_inff())
#  define NAN (__builtin_nanf(""))
#  undef __need_infinity_nan
#else
EOF
    cat "$float_h.orig"
    echo '#endif /* __need_infinity_nan */'
  } >"$float_h"
}

zig_target() {
  [ "$(uname -s)" = Linux ] || return 0
  echo "-Dtarget=$(uname -m)-linux-musl"
}

stamp="$ZMX_REV $GHOSTTY_REV $(patches_hash) $(zig_target)"
if [ -x "$OUT/foster-zmx" ] && [ "$(cat "$OUT/.stamp" 2>/dev/null || true)" = "$stamp" ]; then
  echo "foster-zmx is up to date"
  exit 0
fi

rm -rf "$BUILD" "$OUT"
mkdir -p "$BUILD" "$OUT"

fetch "$ZMX_URL" "$ZMX_REV" "$BUILD/zmx"
fetch "$GHOSTTY_URL" "$GHOSTTY_REV" "$BUILD/zmx-ghostty"

apply "$BUILD/zmx-ghostty" "${PATCHES[3]}"
apply "$BUILD/zmx" "${PATCHES[0]}"
apply "$BUILD/zmx" "${PATCHES[1]}"
apply "$BUILD/zmx" "${PATCHES[2]}"

guard_float_h

(cd "$BUILD/zmx" && zig build -Doptimize=ReleaseSafe $(zig_target))

install -m 0755 "$BUILD/zmx/zig-out/bin/zmx" "$OUT/foster-zmx"
cp "$BUILD/zmx/LICENSE" "$OUT/LICENSE"
echo "$stamp" >"$OUT/.stamp"
echo "built zmx/out/foster-zmx"
