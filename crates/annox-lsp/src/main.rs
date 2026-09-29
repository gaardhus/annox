use std::net::TcpListener;
use std::path::PathBuf;

use annox_sync::hub::{serve, HubConfig};
use lsp_server::Connection;

const USAGE: &str = "usage:
  annox lsp                                   run the language server over stdio (spec §6)
  annox hub --data DIR [--listen ADDR] [--token-file FILE]
                                              run a sync hub (spec §7); ADDR defaults to 127.0.0.1:7878
  annox mcp [--root DIR] [--author ID] [--name NAME]
                                              run an MCP server over stdio, for agents; DIR defaults to
                                              the current directory
";

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
        Some("mcp") => mcp(&args[1..]),
        Some("help" | "--help" | "-h") => {
            println!("{USAGE}{}", annox_lsp::cli::USAGE);
            Ok(())
        }
        Some(_) => match annox_lsp::cli::run(&args, &std::env::current_dir()?) {
            Ok(output) => {
                println!("{}", serde_json::to_string_pretty(&output)?);
                Ok(())
            }
            Err(e) => {
                eprintln!("annox: {e:#}");
                std::process::exit(1);
            }
        },
        None => {
            eprintln!("{USAGE}{}", annox_lsp::cli::USAGE);
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

fn mcp(args: &[String]) -> anyhow::Result<()> {
    let mut config = annox_lsp::mcp::Config { root: std::env::current_dir()?, author: None, name: None };
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        let value = it.next().ok_or_else(|| anyhow::anyhow!("{flag} needs a value\n{USAGE}"))?;
        match flag.as_str() {
            "--root" => config.root = PathBuf::from(value),
            "--author" => config.author = Some(value.clone()),
            "--name" => config.name = Some(value.clone()),
            _ => anyhow::bail!("unknown option {flag}\n{USAGE}"),
        }
    }
    annox_lsp::mcp::serve(std::io::stdin().lock(), std::io::stdout().lock(), &config)?;
    Ok(())
}
