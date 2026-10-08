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
    ANNOX_BIN="$PWD/target/debug/annox" editors/nvim/tests/run.sh

# Record the Neovim demo in assets/nvim-demo.gif. Needs asciinema 3 and agg.
nvim-demo:
    cargo build -p annox-lsp --locked
    ANNOX_BIN="$PWD/target/debug/annox" editors/nvim/demo/record.sh assets/nvim-demo.gif

# Type check and test the VS Code extension. The end-to-end test downloads VS Code
# and runs it on a virtual display when xvfb-run is installed; `just test-vscode show`
# shows its window instead. The Wayland variables are unset so VS Code uses X11.
test-vscode mode="":
    cargo build -p annox-lsp --locked
    cd editors/vscode && npm ci && npm run check && npm run test:unit
    cd editors/vscode && export ANNOX_BIN="$PWD/../../target/debug/annox" && \
    if [ "{{ mode }}" != show ] && command -v xvfb-run > /dev/null; then \
        env -u WAYLAND_DISPLAY -u XDG_SESSION_TYPE xvfb-run -a npm run test:e2e; \
    else \
        npm run test:e2e; \
    fi

# Package the VS Code extension as editors/vscode/annox-<version>.vsix.
vsix:
    cd editors/vscode && npm ci && npm run package

# Render the VS Code extension icon from assets/annox.svg (light version; rsvg-convert ignores the dark-mode styles).
vscode-icon:
    rsvg-convert -w 128 -h 128 assets/annox.svg -o editors/vscode/icon.png

# Install the packaged extension for the version in editors/vscode/package.json.
code-install:
    code --install-extension "editors/vscode/annox-$(node -p "require('./editors/vscode/package.json').version").vsix"

# Build the website in site/: annox-core as WebAssembly, the logos, and the install scripts.
site:
    wasm-pack build crates/annox-wasm --target web --no-typescript --no-pack --out-dir ../../site/annox-demo/pkg
    mkdir -p site/assets && cp assets/*.svg assets/*.gif assets/*.png site/assets/
    cp install.sh install.ps1 site/

# Render the site's PNG images from their SVG sources: the social card (og:image),
# the iOS home screen icon, and a PNG favicon for browsers without SVG favicons.
site-images:
    chromium --headless --disable-gpu --hide-scrollbars --virtual-time-budget=8000 --window-size=1200,630 --screenshot="$PWD/assets/og-card.png" "file://$PWD/assets/og-card.svg"
    rsvg-convert -w 136 -h 136 assets/annox.svg | magick -size 180x180 xc:white - -gravity center -composite -depth 8 assets/apple-touch-icon.png
    rsvg-convert -w 32 -h 32 assets/annox.svg -o assets/favicon-32.png

# Build the website and serve it on http://localhost:8000.
serve-site: site
    python3 -m http.server -d site 8000
