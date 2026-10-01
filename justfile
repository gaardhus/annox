build:
    cargo build --release

hub:
    @annox hub --data /tmp/annox-hub/ --listen 127.0.0.1:7878

link:
    ln -sf "$PWD/target/release/annox" ~/.local/bin/annox

# Install the git hooks (formatting, linting, commit messages).
hooks:
    prek install

# Format everything.
fmt:
    cargo fmt --all
    prek run --all-files stylua-github rumdl-check end-of-file-fixer trailing-whitespace

# Run every check CI runs, except the other operating systems.
check: lint test test-nvim test-vscode

lint:
    prek run --all-files
    cargo clippy --workspace --all-targets --locked -- -D warnings

test:
    cargo test --workspace --locked

test-nvim:
    cargo build -p annox-lsp --locked
    for t in editors/nvim/tests/*.lua; do ANNOX_BIN="$PWD/target/debug/annox" nvim --headless --clean -l "$t" || exit 1; done

# Type check and test the VS Code extension. The end-to-end test downloads VS Code.
test-vscode:
    cargo build -p annox-lsp --locked
    cd editors/vscode && npm ci && npm run check && npm run test:unit
    cd editors/vscode && ANNOX_BIN="$PWD/../../target/debug/annox" npm run test:e2e

# Package the VS Code extension as editors/vscode/annox-<version>.vsix.
vsix:
    cd editors/vscode && npm ci && npm run package

# Install the packaged extension for the version in editors/vscode/package.json.
code-install:
    code --install-extension "editors/vscode/annox-$(node -p "require('./editors/vscode/package.json').version").vsix"

# Build the website in site/: annox-core as WebAssembly, the logos, and the install scripts.
site:
    wasm-pack build crates/annox-wasm --target web --no-typescript --no-pack --out-dir ../../site/annox-demo/pkg
    mkdir -p site/assets && cp assets/*.svg site/assets/
    cp install.sh install.ps1 site/

# Build the website and serve it on http://localhost:8000.
serve-site: site
    python3 -m http.server -d site 8000
