#!/bin/sh
# Builds the npm package `roomy-cli` for a published release, from its universal binary.
#   packaging/npm/build.sh v0.1.0 out/
# The package ships the binary itself: no install scripts, no downloads at install or run time.
set -eu
tag=$1
out=$2
version=${tag#v}
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

name="roomy-${tag}-universal-apple-darwin"
gh release download "$tag" --repo masa-tichita/roomy --pattern "$name.tar.gz" --pattern "$name.tar.gz.sha256" --dir "$work"
(cd "$work" && shasum -a 256 -c "$name.tar.gz.sha256")
mkdir "$work/x" && tar -xzf "$work/$name.tar.gz" -C "$work/x"

rm -rf "$out" && mkdir -p "$out/bin"
cp "$work/x/roomy" "$out/bin/roomy" && chmod 755 "$out/bin/roomy"
cp "$root/README.md" "$root/LICENSE-MIT" "$root/LICENSE-APACHE" "$out/"
sed "s/@VERSION@/$version/" "$here/package.json.in" > "$out/package.json"
echo "built $out for $tag"
