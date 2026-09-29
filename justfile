build:
    cargo build --release

hub:
    annox hub --data /tmp/annox-hub/ --listen 127.0.0.1:7878
