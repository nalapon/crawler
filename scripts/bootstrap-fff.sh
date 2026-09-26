#!/bin/zsh
set -euo pipefail
cd "${0:A:h:h}"
if ! command -v rustup >/dev/null; then
  print -u2 "Install rustup first: https://rust-lang.org/tools/install/"
  exit 1
fi
if ! python3 -c 'import tomllib' >/dev/null 2>&1; then
  print -u2 "Python 3.11 or later is required. Install it and make sure python3 is on PATH."
  exit 1
fi
FFF_REVISION=c188e7a8f90e4ee4aabe18f93d5869dfb4289f12
FFF_ROOT="$PWD/.build/fff/upstream"
RUST_VERSION=1.97.1
if [[ ! -d "$FFF_ROOT/.git" ]]; then
  git clone --no-checkout https://github.com/dmtrKovalenko/fff.git "$FFF_ROOT"
fi
if ! git -C "$FFF_ROOT" cat-file -e "$FFF_REVISION^{commit}" 2>/dev/null; then
  git -C "$FFF_ROOT" fetch origin "$FFF_REVISION"
fi
# Only generated dependency sources live here. Never touch the project's worktree.
if [[ ! -f "$FFF_ROOT/.crawler-revision" ]] || [[ "$(cat "$FFF_ROOT/.crawler-revision")" != "$FFF_REVISION" ]]; then
  git -C "$FFF_ROOT" archive "$FFF_REVISION" | tar -x -C "$FFF_ROOT"
  printf '\nmod crawler_ffi;\n' >> "$FFF_ROOT/crates/fff-core/src/lib.rs"
  printf '%s\n' "$FFF_REVISION" > "$FFF_ROOT/.crawler-revision"
fi
if ! cmp -s Sources/FFFBridge/engine.rs "$FFF_ROOT/crates/fff-core/src/crawler_ffi.rs"; then
  cp Sources/FFFBridge/engine.rs "$FFF_ROOT/crates/fff-core/src/crawler_ffi.rs"
fi
python3 - "$FFF_ROOT/crates/fff-core/Cargo.toml" <<'PY'
import pathlib, sys
p = pathlib.Path(sys.argv[1])
original = p.read_text()
updated = original.replace('crate-type = ["rlib"]', 'crate-type = ["rlib", "staticlib"]')
if updated != original:
    p.write_text(updated)
PY
if ! rustup run "$RUST_VERSION" rustc --version >/dev/null 2>&1; then
  rustup toolchain install "$RUST_VERSION" --profile minimal
fi
# Keep Rust proc-macro dylibs intact: LLVM stripping can produce a LINKEDIT
# string pool rejected by macOS dyld. The packaged executable is stripped later.
CARGO_PROFILE_RELEASE_STRIP=none MACOSX_DEPLOYMENT_TARGET=14.0 rustup run "$RUST_VERSION" cargo build \
  --manifest-path "$FFF_ROOT/Cargo.toml" --locked --release -p fff-search
mkdir -p .build/fff/lib
cp "$FFF_ROOT/target/release/libfff_search.a" .build/fff/lib/
python3 scripts/fff-notices.py "$FFF_ROOT" "$RUST_VERSION"
