use std::net::TcpListener;
use std::path::PathBuf;

use annox_sync::hub::{serve, HubConfig};
use lsp_server::Connection;

const USAGE: &str = "usage:
  annox lsp                                   run the language server over stdio (spec §6)
  annox hub --data DIR [--listen ADDR] [--token-file FILE]
                                              run a sync hub (spec §7); ADDR defaults to 127.0.0.1:7878";

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("lsp") => {
            let (connection, io_threads) = Connection::stdio();
            annox_lsp::run(&connection)?;
            drop(connection);
            io_threads.join()?;
            Ok(())
        }
        Some("hub") => hub(&args[1..]),
        _ => {
            eprintln!("{USAGE}");
            std::process::exit(2);
        }
    }
}

fn hub(args: &[String]) -> anyhow::Result<()> {
    let mut listen = "127.0.0.1:7878".to_owned();
    let mut data = None;
    let mut token = None;
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        let value = it.next().ok_or_else(|| anyhow::anyhow!("{flag} needs a value\n{USAGE}"))?;
        match flag.as_str() {
            "--listen" => listen = value.clone(),
            "--data" => data = Some(PathBuf::from(value)),
            // A file, not a flag value, so the token doesn't show up in `ps`.
            "--token-file" => token = Some(std::fs::read_to_string(value)?.trim().to_owned()),
            _ => anyhow::bail!("unknown option {flag}\n{USAGE}"),
        }
    }
    let data = data.ok_or_else(|| anyhow::anyhow!("--data is required\n{USAGE}"))?;
    let listener = TcpListener::bind(&listen)?;
    eprintln!("annox hub listening on ws://{} (workspaces at /w/<name>)", listener.local_addr()?);
    serve(listener, HubConfig { data, token })?;
    Ok(())
}
