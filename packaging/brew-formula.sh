#!/usr/bin/env bash
# Prints the Homebrew formula for one release. The bottles are the npm
# platform tarballs (see npm-pack.sh), so brew needs no GitHub access
# while the repo is private.
#
#   packaging/brew-formula.sh <version> <dir with the packed .tgz files>
set -euo pipefail

version=$1 packs=$2

# Prints the url and sha256 lines for one platform package.
source_for() {
  local name="bungkus-mc-$1"
  local tgz="$packs/osbrjp-$name-$version.tgz"
  local sha
  sha=$(shasum -a 256 "$tgz" | cut -d' ' -f1)
  printf '      url "https://registry.npmjs.org/@osbrjp/%s/-/%s-%s.tgz"\n' "$name" "$name" "$version"
  printf '      sha256 "%s"\n' "$sha"
}

cat <<EOF
class BungkusMc < Formula
  desc "Mission control for AI coding agents in the terminal"
  homepage "https://github.com/osbrjp/bungkus-mc"
  version "$version"
  license :cannot_represent

  on_macos do
    on_arm do
$(source_for darwin-arm64)
    end
    on_intel do
$(source_for darwin-x64)
    end
  end

  on_linux do
    on_arm do
$(source_for linux-arm64)
    end
    on_intel do
$(source_for linux-x64)
    end
  end

  def install
    bin.install "bin/bungkus-mc"
    bin.install_symlink "bungkus-mc" => "bkmc"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/bungkus-mc --version")
  end
end
EOF
