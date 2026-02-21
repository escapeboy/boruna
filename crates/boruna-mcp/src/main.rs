use clap::Parser;
use rmcp::ServiceExt;

mod server;
mod tools;

#[derive(Parser)]
#[command(name = "boruna-mcp", about = "Boruna MCP server for AI coding agents")]
struct Args {
    /// Path to templates directory.
    #[arg(long, default_value = "templates")]
    templates_dir: String,

    /// Path to standard libraries directory.
    #[arg(long, default_value = "libs")]
    libs_dir: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    let server = server::BorunaMcpServer::new(args.templates_dir, args.libs_dir);
    let service = server.serve(rmcp::transport::stdio()).await?;
    service.waiting().await?;
    Ok(())
}
