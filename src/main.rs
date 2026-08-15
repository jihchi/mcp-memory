use mcp_memory::server::MemoryServer;
use rmcp::{
    ServiceExt,
    transport::{
        StreamableHttpServerConfig, StreamableHttpService,
        streamable_http_server::session::local::LocalSessionManager,
    },
};
use std::{env, net::SocketAddr, sync::Arc};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Transport {
    Stdio,
    Http,
    Sse,
}

fn arguments() -> Result<(Transport, SocketAddr, String), String> {
    let mut transport = Transport::Stdio;
    let mut bind = "127.0.0.1:8080".parse().unwrap();
    let mut path = None;
    let args = env::args().skip(1).collect::<Vec<_>>();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--transport" => {
                i += 1;
                transport = match args.get(i).map(String::as_str) {
                    Some("stdio") => Transport::Stdio,
                    Some("http") | Some("streamable-http") => Transport::Http,
                    Some("sse") => Transport::Sse,
                    Some(value) => return Err(format!("unsupported transport '{value}'")),
                    None => return Err("--transport requires stdio, http, or sse".into()),
                };
            }
            "--bind" => {
                i += 1;
                bind = args
                    .get(i)
                    .ok_or("--bind requires an address")?
                    .parse()
                    .map_err(|e| format!("invalid bind address: {e}"))?;
            }
            "--path" => {
                i += 1;
                path = Some(args.get(i).ok_or("--path requires a value")?.clone());
            }
            "-h" | "--help" => {
                println!(
                    "mcp-memory {}\n\nUsage: mcp-memory [--transport stdio|http|sse] [--bind 127.0.0.1:8080] [--path /mcp]\n\nDefaults to MCP over stdio.",
                    env!("CARGO_PKG_VERSION")
                );
                std::process::exit(0);
            }
            "-V" | "--version" => {
                println!("mcp-memory {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            value => return Err(format!("unknown argument '{value}'")),
        }
        i += 1;
    }
    let path = path.unwrap_or_else(|| {
        if transport == Transport::Sse {
            "/sse".into()
        } else {
            "/mcp".into()
        }
    });
    if !path.starts_with('/') {
        return Err("--path must begin with '/'".into());
    }
    Ok((transport, bind, path))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (transport, bind, path) = arguments().map_err(std::io::Error::other)?;
    match transport {
        Transport::Stdio => {
            let service = MemoryServer::default()
                .serve(rmcp::transport::stdio())
                .await?;
            service.waiting().await?;
        }
        Transport::Http => serve_http(bind, &path).await?,
        Transport::Sse => mcp_memory::sse::serve(bind, &path, MemoryServer::default).await?,
    }
    Ok(())
}

async fn serve_http(bind: SocketAddr, path: &str) -> Result<(), Box<dyn std::error::Error>> {
    let cancellation = CancellationToken::new();
    let service: StreamableHttpService<MemoryServer, LocalSessionManager> =
        StreamableHttpService::new(
            || Ok(MemoryServer::default()),
            Arc::new(LocalSessionManager::default()),
            StreamableHttpServerConfig::default()
                .with_cancellation_token(cancellation.child_token()),
        );
    let router = axum::Router::new().nest_service(path, service);
    let listener = tokio::net::TcpListener::bind(bind).await?;
    eprintln!(
        "[mcp-memory] Streamable HTTP listening on http://{}{}",
        listener.local_addr()?,
        path
    );
    axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            let _ = tokio::signal::ctrl_c().await;
            cancellation.cancel();
        })
        .await?;
    Ok(())
}
