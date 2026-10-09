# Task runner for the workspace: the quality gate, builds, tests and the Obsidian plugin release.

# Cargo features the quality gate turns on beyond crate defaults: `session` is off by default,
# so the gate enables it to compile, lint and test the session code of both crates.
gate_features := "remargin/session"

# Default: list recipes.
default:
    @just --list

# Generate TypeScript types + Zod schemas from Rust models.
generate-types:
    cargo xtask generate-types
    pnpm -C packages/remargin-obsidian exec biome check --write src/generated/

# Lint Rust (clippy + fmt check) and TypeScript (biome + tsc).
lint: lint-rust lint-ts

# `--workspace` because `default-members` excludes `crates/xtask`: without
# it the type generator is neither linted nor tested by the gate.
lint-rust:
    cargo clippy --workspace --all-targets --features {{gate_features}} -- -D warnings
    cargo fmt --all --check

lint-ts:
    pnpm -C packages/remargin-obsidian lint
    pnpm -C packages/remargin-obsidian typecheck
    pnpm -C packages/remargin-website lint
    pnpm -C packages/remargin-website check

# Build Rust first, then TypeScript.
build: build-rust build-ts

build-rust:
    cargo build --features {{gate_features}}

build-ts: generate-types
    pnpm -C packages/remargin-obsidian build

# Build the CLI with the Obsidian plugin feature. The CLI fetches main.js /
# manifest.json from the matching GitHub release at install time, so nothing here
# depends on the TypeScript build.
build-cli-obsidian:
    cargo build -p remargin --features obsidian

# Run the Rust and TypeScript test suites.
test: test-rust test-ts

test-rust:
    cargo test --workspace --features {{gate_features}}

# The TypeScript tests validate against `src/generated/`, which is
# gitignored, so a fresh checkout must generate before testing.
test-ts: generate-types
    pnpm -C packages/remargin-obsidian test

# Generate code coverage report (requires cargo-llvm-cov)
test-coverage:
    @echo "Generating code coverage report..."
    cargo llvm-cov --workspace --all-features

# Generate code coverage HTML report (requires cargo-llvm-cov)
test-coverage-html:
    @echo "Generating code coverage HTML report..."
    cargo llvm-cov --workspace --all-features --html
    @echo "Coverage report generated in target/llvm-cov/html/"

# Audit dependencies for security advisories.
audit:
    cargo audit

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

# Propagate Cargo's workspace version into the Obsidian plugin's
# manifest.json and package.json AND the Claude Code plugin's manifest +
# marketplace entry using jq. Cargo.toml is the single source of truth for
# the plugin version; these JSON files are derived. Idempotent: when the
# files already match Cargo, this recipe is a no-op.
sync-versions:
    #!/usr/bin/env bash
    set -euo pipefail
    VERSION=$(cargo metadata --format-version 1 --no-deps | jq -r '.packages[] | select(.name=="remargin") | .version')
    if [ -z "${VERSION}" ] || [ "${VERSION}" = "null" ]; then
        echo "error: failed to extract remargin version from cargo metadata" >&2
        exit 1
    fi
    for file in packages/remargin-obsidian/manifest.json packages/remargin-obsidian/package.json claude-plugin/.claude-plugin/plugin.json; do
        current=$(jq -r '.version' "${file}")
        if [ "${current}" = "${VERSION}" ]; then
            echo "${file}: ${VERSION} (already in sync)"
        else
            tmp=$(mktemp)
            jq --indent 2 --arg v "${VERSION}" '.version = $v' "${file}" > "${tmp}"
            mv "${tmp}" "${file}"
            echo "${file}: ${current} -> ${VERSION}"
        fi
    done
    # Marketplace entry: version lives under .plugins[].version, matched by name.
    market=.claude-plugin/marketplace.json
    current=$(jq -r '.plugins[] | select(.name=="remargin") | .version' "${market}")
    if [ "${current}" = "${VERSION}" ]; then
        echo "${market}: ${VERSION} (already in sync)"
    else
        tmp=$(mktemp)
        jq --indent 2 --arg v "${VERSION}" '.plugins |= map(if .name == "remargin" then .version = $v else . end)' "${market}" > "${tmp}"
        mv "${tmp}" "${market}"
        echo "${market}: ${current} -> ${VERSION}"
    fi

# Publish the Obsidian plugin as a GitHub release tagged obsidian-v<version>.
# Runs sync-versions first so manifest.json and package.json always reflect
# the current Cargo workspace version. The dirty-tree guard excludes those
# two files since they are derived from Cargo and may legitimately change
# as part of the publish run.
publish-obsidian: sync-versions
    #!/usr/bin/env bash
    set -euo pipefail
    if ! git diff --quiet HEAD -- \
        packages/remargin-obsidian crates/remargin \
        ':(exclude)packages/remargin-obsidian/manifest.json' \
        ':(exclude)packages/remargin-obsidian/package.json'; then
        echo "error: uncommitted changes in packages/remargin-obsidian or crates/remargin -- commit before publishing" >&2
        exit 1
    fi
    if ! gh auth status >/dev/null 2>&1; then
        echo "error: gh is not authenticated -- run 'gh auth login' before publishing" >&2
        exit 1
    fi
    VERSION=$(cargo metadata --format-version 1 --no-deps | jq -r '.packages[] | select(.name=="remargin") | .version')
    if [ -z "${VERSION}" ] || [ "${VERSION}" = "null" ]; then
        echo "error: failed to extract remargin version from cargo metadata" >&2
        exit 1
    fi
    TAG="obsidian-v${VERSION}"
    echo "Publishing ${TAG}"
    pnpm -C packages/remargin-obsidian install --frozen-lockfile
    pnpm -C packages/remargin-obsidian build
    if gh release view "${TAG}" >/dev/null 2>&1; then
        echo "Release ${TAG} already exists -- overwriting assets with --clobber"
        gh release upload "${TAG}" \
            packages/remargin-obsidian/main.js \
            packages/remargin-obsidian/manifest.json \
            --clobber
    else
        gh release create "${TAG}" \
            packages/remargin-obsidian/main.js \
            packages/remargin-obsidian/manifest.json \
            --title "Obsidian plugin v${VERSION}" \
            --generate-notes
    fi
