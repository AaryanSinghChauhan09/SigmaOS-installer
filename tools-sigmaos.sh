#!/bin/sh
# SigmaOS component interconnect helper v2.1.
# Usage: ./tools-sigmaos.sh <command>
# Commands: info, test, build, sync, deps, peers, link, status, check
set -eu
cmd="${1:-info}"
here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
root=$(dirname "$here")
manifest="$here/COMPONENT.toml"

have_cargo() { [ -f "$here/Cargo.toml" ]; }

# Normalize a component name to a repo dir name.
repo_of() {
  # strip sigmaos- prefix variants, keep raw name
  raw=$(printf '%s' "$1" | tr 'A-Z' 'a-z')
  raw=${raw#sigmaos-}
  raw=${raw#sigmaos}
  want="sigmaos-$raw"
  for d in "$root"/SigmaOS-*/; do
    [ -d "$d" ] || continue
    b=$(basename "$d" | tr 'A-Z' 'a-z')
    [ "$b" = "$want" ] && { basename "$d"; return 0; }
  done
  return 1
}
# Extract declared deps from either manifest style.
deps_list() {
  [ -f "$manifest" ] || return 0
  # style 1: [dependencies] section entries like `name = "..."` or bare names
  sed -n '/^\[dependencies\]/,/^\[/p' "$manifest" | sed -n 's/^[[:space:]]*\([A-Za-z0-9_.-]*\)[[:space:]]*=.*/\1/p'
  # style 2: depends_on = ["a", "b"] inside [contract]
  sed -n '/depends_on/,/]/p' "$manifest" | grep -o '"[^"]*"' | tr -d '"'
}

case "$cmd" in
  info)
    if have_cargo; then
      cargo metadata --no-deps --format-version 1 --manifest-path "$here/Cargo.toml" | head -c 2000; echo
    else
      grep -E '^(name|description|version)' "$manifest" 2>/dev/null || basename "$here"
    fi
    ;;
  test)  have_cargo && cargo test --manifest-path "$here/Cargo.toml" || echo "no cargo targets" ;;
  build) have_cargo && cargo build --release --manifest-path "$here/Cargo.toml" || echo "no cargo targets" ;;
  deps)  deps_list | sort -u ;;
  check)
    # Verify every declared dep resolves to a sibling repo.
    missing=0
    for dep in $(deps_list | sort -u); do
      if repo_of "$dep" >/dev/null; then
        echo "  ok    $dep -> $(repo_of "$dep")"
      else
        echo "  MISS  $dep"
        missing=$((missing+1))
      fi
    done
    [ "$missing" -eq 0 ] && echo "all deps resolved" || { echo "$missing unresolved"; exit 1; }
    ;;
  peers)
    for d in "$root"/SigmaOS-*/; do
      [ "$d" = "$here/" ] && continue
      [ -d "$d/.git" ] && basename "$d"
    done | sort
    ;;
  link)
    mkdir -p "$HOME/.config/sigmaos/links"
    ln -sfn "$here" "$HOME/.config/sigmaos/links/$(basename "$here")"
    echo "linked $(basename "$here")"
    ;;
  status)
    cd "$here"
    echo "component: $(basename "$here")"
    git status --short | head -5
    git log --oneline -1
    ;;
  sync)  cd "$here" && git pull --rebase origin main && git push origin main ;;
  *) echo "usage: $0 [info|test|build|sync|deps|peers|link|status|check]" >&2; exit 2 ;;
esac
