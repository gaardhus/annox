build:
    cargo build --release

hub:
    annox hub --data /tmp/annox-hub/ --listen 127.0.0.1:7878

# Install the git hooks (formatting, linting, commit messages).
hooks:
    prek install

# Format everything.
fmt:
    cargo fmt --all
    prek run --all-files stylua-github rumdl-check end-of-file-fixer trailing-whitespace

# Run every check CI runs, except the other operating systems.
check: lint test test-nvim

lint:
    prek run --all-files
    cargo clippy --workspace --all-targets --locked -- -D warnings

test:
    cargo test --workspace --locked

test-nvim:
    cargo build -p annox-lsp --locked
    for t in editors/nvim/tests/*.lua; do ANNOX_BIN="$PWD/target/debug/annox" nvim --headless --clean -l "$t" || exit 1; done
