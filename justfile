# Build the release binary. `just build v0.14.0` builds a tag (or any commit) from a
# temporary worktree instead, leaving the working tree alone, and puts the binary in
# target/tags/v0.14.0/annox. Tag builds get their own target directory: cargo decides
# what to rebuild by file times, so sharing target/ makes the next plain build think the
# older files in the working tree are already built.
build ref="":
    #!/usr/bin/env bash
    set -euo pipefail
    ref='{{ ref }}'
    if [ -z "$ref" ]; then
        exec cargo build --release
    fi
    case "$ref" in -*) echo "not a git ref: $ref" >&2; exit 1 ;; esac
    dir="$(mktemp -d)"
    trap 'git worktree remove --force "$dir"' EXIT
    git worktree add --detach "$dir" "$ref"
    cargo build --release --locked --manifest-path "$dir/Cargo.toml" --target-dir "$PWD/target/tag"
    mkdir -p "target/tags/$ref"
    cp target/tag/release/annox "target/tags/$ref/annox"

hub:
    @annox hub --data /tmp/annox-hub/ --listen 127.0.0.1:7878

# Put annox on the PATH, and its man pages where `man` finds them. `just link v0.14.0`
# links the binary from `just build v0.14.0`.
link ref="":
    #!/usr/bin/env bash
    set -euo pipefail
    ref='{{ ref }}'
    bin="$PWD/target/release/annox"
    if [ -n "$ref" ]; then bin="$PWD/target/tags/$ref/annox"; fi
    [ -x "$bin" ] || { echo "no binary at $bin; run just build $ref first" >&2; exit 1; }
    ln -sf "$bin" ~/.local/bin/annox
    man=~/.local/share/man/man1
    mkdir -p "$man"
    rm -f "$man/annox.1" "$man"/annox-*.1
    "$bin" man "$man" || echo "this annox has no man pages" >&2

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

# Render the animated intro (assets/intro/annox-intro.html) to a video. The extension of
# `out` picks the format: .mp4, .webm, or .gif. Needs uv, ffmpeg, and Chromium.
intro-video out="target/annox-intro.mp4" theme="light" fps="30" width="1920":
    uv run assets/intro/render.py "{{ out }}" --theme {{ theme }} --fps {{ fps }} --width {{ width }}

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
