#!/bin/sh
# SigmaOS component interconnect helper v2.
# Usage: ./tools-sigmaos.sh <command>
# Commands: info, test, build, sync, deps, peers, link, status
set -eu
cmd="${1:-info}"
here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
root=$(dirname "$here")

have_cargo() { [ -f "$here/Cargo.toml" ]; }

case "$cmd" in
  info)
    if have_cargo; then
      cargo metadata --no-deps --format-version 1 --manifest-path "$here/Cargo.toml" | head -c 2000; echo
    else
      grep -E '^(name|description|version)' "$here/COMPONENT.toml" 2>/dev/null || basename "$here"
    fi
    ;;
  test)  have_cargo && cargo test --manifest-path "$here/Cargo.toml" || echo "no cargo targets" ;;
  build) have_cargo && cargo build --release --manifest-path "$here/Cargo.toml" || echo "no cargo targets" ;;
  deps)
    # Print declared component dependencies from COMPONENT.toml
    sed -n '/\[dependencies\]/,/^\[/p' "$here/COMPONENT.toml" 2>/dev/null | grep -v '^\[' || true
    ;;
  peers)
    # List sibling SigmaOS component repos available for interconnect
    for d in "$root"/SigmaOS-*/; do
      [ "$d" = "$here/" ] && continue
      [ -d "$d/.git" ] && basename "$d"
    done | sort
    ;;
  link)
    # Register this component's config/state dirs under ~/.config/sigmaos/links
    mkdir -p "$HOME/.config/sigmaos/links"
    ln -sfn "$here" "$HOME/.config/sigmaos/links/$(basename "$here")"
    echo "linked $(basename "$here")"
    ;;
  status)
    # Health summary: git state + manifest presence
    cd "$here"
    echo "component: $(basename "$here")"
    git status --short | head -5
    git log --oneline -1
    ;;
  sync)  cd "$here" && git pull --rebase origin main && git push origin main ;;
  *) echo "usage: $0 [info|test|build|sync|deps|peers|link|status]" >&2; exit 2 ;;
esac
