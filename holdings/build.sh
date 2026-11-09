#!/usr/bin/env bash
# Builds `holdings` inside the game's workspace, at the release the network
# runs, from a local checkout of digital-objects-network. The checkout is not
# changed: the build happens in a clone of it.
#
#   holdings/build.sh /path/to/digital-objects-network
#
# COMMIT     the game commit to build against (default 616e0eb, v0.1.0-rc.50,
#            which the hosted synchronizer reports at /info)
# BUILD_DIR  where the clone and the build go (default ~/.cache/bitcraft-holdings)
#
# Prints the binary's path. The first build compiles pod2, plonky2 and the
# game's libraries; later ones only this crate.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
game="${1:?usage: holdings/build.sh /path/to/digital-objects-network}"
commit="${COMMIT:-616e0eb}"
build="${BUILD_DIR:-$HOME/.cache/bitcraft-holdings}"

mkdir -p "$build"
if [ ! -d "$build/src/.git" ]; then
  git clone -q --local --no-checkout "$game" "$build/src"
fi
git -C "$build/src" cat-file -e "$commit^{commit}" 2>/dev/null ||
  git -C "$build/src" fetch -q "$game"
git -C "$build/src" checkout -q --force "$commit"
rm -rf "$build/src/devtools/holdings"
mkdir -p "$build/src/devtools/holdings"
cp -R "$here/Cargo.toml" "$here/src" "$build/src/devtools/holdings/"
perl -pi -e 's|^    "devtools/tests",\n|$&    "devtools/holdings",\n|' "$build/src/Cargo.toml"
grep -q '"devtools/holdings"' "$build/src/Cargo.toml"

(cd "$build/src" && CARGO_TARGET_DIR="$build/target" cargo build -q --release -p holdings)
echo "$build/target/release/holdings"
