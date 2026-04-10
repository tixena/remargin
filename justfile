# Install the Obsidian plugin into a vault.
# Usage: just install-obsidian /path/to/vault
install-obsidian vault:
    rm -rf "{{vault}}/.obsidian/plugins/remargin"
    pnpm -C packages/remargin-obsidian build
    mkdir -p "{{vault}}/.obsidian/plugins/remargin"
    cp packages/remargin-obsidian/main.js "{{vault}}/.obsidian/plugins/remargin/"
    cp packages/remargin-obsidian/manifest.json "{{vault}}/.obsidian/plugins/remargin/"
    @echo "Installed to {{vault}}/.obsidian/plugins/remargin"
