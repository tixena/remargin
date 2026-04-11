# Default: list recipes.
default:
    @just --list

# Generate TypeScript types + Zod schemas from Rust models.
generate-types:
    cargo run --bin generate_types
    pnpm -C packages/remargin-obsidian exec biome check --write src/generated/

# Lint Rust (clippy + fmt check) and TypeScript (biome + tsc).
lint: lint-rust lint-ts

lint-rust:
    cargo clippy --all-targets -- -D warnings
    cargo fmt --check

lint-ts:
    pnpm -C packages/remargin-obsidian lint
    pnpm -C packages/remargin-obsidian typecheck

# Build Rust first, then TypeScript.
build: build-rust build-ts

build-rust:
    cargo build

build-ts: generate-types
    pnpm -C packages/remargin-obsidian build

# Build the CLI with the Obsidian plugin feature. The CLI no longer embeds
# the plugin at compile time -- it fetches main.js / manifest.json from the
# matching GitHub release at install time -- so this recipe no longer
# depends on the TypeScript build.
build-cli-obsidian:
    cargo build -p remargin --features obsidian

# Run the Rust test suite.
test:
    cargo test

# Full pipeline: generate types, lint everything, build everything, run tests.
all: generate-types lint build test

# Install the Obsidian plugin into a vault.
# Usage: just install-obsidian /path/to/vault
install-obsidian vault: build-ts
    rm -rf "{{vault}}/.obsidian/plugins/remargin"
    mkdir -p "{{vault}}/.obsidian/plugins/remargin"
    cp packages/remargin-obsidian/main.js "{{vault}}/.obsidian/plugins/remargin/"
    cp packages/remargin-obsidian/manifest.json "{{vault}}/.obsidian/plugins/remargin/"
    @echo "Installed to {{vault}}/.obsidian/plugins/remargin"
