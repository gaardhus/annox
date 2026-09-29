use lsp_server::Connection;

fn main() -> anyhow::Result<()> {
    match std::env::args().nth(1).as_deref() {
        Some("lsp") => {
            let (connection, io_threads) = Connection::stdio();
            annox_lsp::run(&connection)?;
            drop(connection);
            io_threads.join()?;
            Ok(())
        }
        _ => {
            eprintln!("usage: annox lsp    run the annox language server over stdio (spec §6)");
            std::process::exit(2);
        }
    }
}
