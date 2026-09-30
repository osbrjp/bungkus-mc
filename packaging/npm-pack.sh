#!/usr/bin/env bash
# Builds the npm packages for one release into <out>: one platform package
# per binary (the binary plus a package.json with os/cpu) and the main
# @osbrjp/bungkus-mc package whose bin wrapper runs the matching binary.
#
#   packaging/npm-pack.sh <version> <dist> <out>
#
# <dist> holds bungkus-mc-{darwin,linux}-{arm64,amd64}; <out> gets one
# folder per package plus the packed .tgz files.
set -euo pipefail

version=$1 dist=$2 out=$3
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/.." && pwd)
rm -rf "$out" && mkdir -p "$out"

for target in darwin-arm64 darwin-amd64 linux-arm64 linux-amd64; do
  os=${target%-*}
  cpu=${target#*-}
  [ "$cpu" = amd64 ] && cpu=x64
  name="bungkus-mc-$os-$cpu"
  dir="$out/$name"
  mkdir -p "$dir/bin"
  install -m 0755 "$dist/bungkus-mc-$target" "$dir/bin/bungkus-mc"
  cp "$root/LICENSE" "$dir/"
  jq -n --arg name "@osbrjp/$name" --arg v "$version" --arg os "$os" --arg cpu "$cpu" '{
    name: $name, version: $v,
    description: "bungkus-mc binary for \($os)/\($cpu)",
    homepage: "https://github.com/osbrjp/bungkus-mc",
    license: "SEE LICENSE IN LICENSE",
    os: [$os], cpu: [$cpu],
    files: ["bin/bungkus-mc", "LICENSE"]
  }' > "$dir/package.json"
done

main="$out/bungkus-mc"
mkdir -p "$main/bin"
cp "$here/npm/bin/bungkus-mc.js" "$main/bin/"
chmod 0755 "$main/bin/bungkus-mc.js"
cp "$root/LICENSE" "$root/README.md" "$main/"
jq --arg v "$version" '.version = $v | .optionalDependencies |= map_values($v)' \
  "$here/npm/package.json" > "$main/package.json"

for dir in "$out"/*/; do
  (cd "$out" && npm pack --silent "./$(basename "$dir")" >/dev/null)
done
ls "$out"/*.tgz
