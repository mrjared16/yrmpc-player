use anyhow::Result;
use clap::Parser;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Address to bind to (default: 127.0.0.1:6600)
    #[arg(short, long, default_value = "127.0.0.1:6600")]
    bind: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    // env_logger::init();
    let args = Args::parse();

    println!("Starting rmpcd on {}", args.bind);
    
    // Placeholder for server logic
    // let server = MpdServer::new(&args.bind).await?;
    // server.run().await?;

    Ok(())
}
