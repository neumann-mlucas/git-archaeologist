# Local test and lint entry points. CI runs only `fmt`, `clippy`, `test`.

default:
    @just --list

# rustfmt, clippy, dprint
lint:
    cargo fmt --all --check
    cargo clippy --all-targets -- -D warnings
    dprint check

# Tier 0 unit + Tier 1 golden + Tier 1.x scenarios
test:
    cargo test

# Tier 2: ratatui smoke vs git, blame, tokei (clones once, then cached)
e2e:
    cargo test --release --features e2e --test tier2_smoke -- --nocapture

# Tier 3: ratatui, uv, godot perf ceilings (SKIP_LARGE=1 skips godot)
bench:
    TIER3_SKIP_LARGE="${SKIP_LARGE:-}" cargo test --release --features bench-large --test tier3_bench -- --nocapture

# Rewrite tests/goldens after an intentional output change
regen-goldens:
    REGEN_GOLDENS=1 cargo test --test golden tier1_golden_integration

# Everything short of the bench
all: lint test e2e

# Release build copied to ~/.local/bin
install:
    cargo build --release
    cp target/release/git-archaeologist ~/.local/bin/
