default:
    @just --list

# clippy with ci's denials, after every change
check: clippy

fmt:
    cargo fmt --all

fmt-check:
    cargo fmt --all -- --check

clippy:
    cargo clippy --all-targets --all-features -- -D warnings

# apply clippy's and rustfmt's fixes
fix:
    cargo clippy --all-targets --all-features --fix --allow-dirty --allow-staged
    cargo fmt --all

test:
    cargo test --all-features --all-targets

# the tests that meet through the real public brokers
test-online:
    cargo test --all-features -- --ignored brokers

# play every sound on the speakers, to listen to them
sounds:
    cargo test --release --lib every_cue_plays_on_the_speakers -- --ignored

# how long frames take at 1600 by 1000
frame-times:
    cargo test --release --lib frame_times -- --ignored --nocapture

# licenses, advisories and sources, from deny.toml
deny:
    cargo deny check

typos:
    typos

# copy-paste duplication, from .jscpd.json
dupes:
    npx --yes jscpd@4 src/

# cyclomatic complexity, length and parameter count of every function
complexity:
    uvx --from lizard==1.26.0 lizard src -l rust -C 15 -L 100 -a 8 -w

# the code-quality gates: complexity and duplication
quality: complexity dupes

# unused dependencies
machete:
    cargo machete

# install the git hooks, once per clone
hooks:
    prek install --hook-type pre-commit --hook-type commit-msg

# rustdoc with warnings as errors
docs:
    RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --document-private-items

# every gate, before every commit
ci: fmt-check clippy test docs typos quality

# render the boards and pieces again with blender, headless (slow, needs a gpu for speed)
art samples="96" styles="amber,glass,metal":
    blender --background --factory-startup --python assets/render.py -- --out assets --samples {{samples}} --styles {{styles}}

# the screenshots in docs/screenshots, rendered headless
screenshots:
    KESS_SHOTS=docs/screenshots cargo test --release --lib screenshot -- --ignored

# both sides on one keyboard
play *args:
    cargo run --release -- {{args}}

# host an online game, like `just host white --clock 5+3`
host *args:
    cargo run --release -- host {{args}}

# join an online game with the host's code
join code *args:
    cargo run --release -- join {{code}} {{args}}

# the version Cargo.toml names
version:
    @sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1

# fail unless `tag` names the crate version, like v0.1.0
release-check tag:
    test {{quote(tag)}} = "v$(just version)"

# bumps the version, passes the gate, commits, tags and pushes; the release
# workflow then builds the binaries and publishes the crate.
# release, like `just release patch` (or minor, major)
release level:
    #!/usr/bin/env bash
    set -euo pipefail
    test -z "$(git status --porcelain)" || { echo "commit or stash your changes first"; exit 1; }
    test "$(git branch --show-current)" = main || { echo "release from main"; exit 1; }
    IFS=. read -r major minor patch <<< "$(just version)"
    case {{quote(level)}} in
      major) next="$((major + 1)).0.0" ;;
      minor) next="$major.$((minor + 1)).0" ;;
      patch) next="$major.$minor.$((patch + 1))" ;;
      *) echo "say major, minor or patch"; exit 1 ;;
    esac
    current=$(just version)
    perl -pi -e "s/^version = \"\Q$current\E\"/version = \"$next\"/" Cargo.toml
    cargo update --workspace --quiet
    just release-check "v$next"
    git add -A
    just ci
    git commit -m "chore: release $next"
    git tag "v$next"
    git push origin main "v$next"
