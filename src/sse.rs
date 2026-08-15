//! Backward-compatible MCP HTTP+SSE transport (protocol 2024-11-05 style).
use axum::{
    Json, Router,
    extract::{Query, State},
    http::StatusCode,
    response::sse::{Event, KeepAlive, Sse},
    routing::{get, post},
};
use futures::{Sink, SinkExt, Stream, StreamExt};
use rmcp::{
    RoleServer,
    model::ClientJsonRpcMessage,
    service::{RxJsonRpcMessage, TxJsonRpcMessage},
};
use serde::Deserialize;
use std::{
    collections::HashMap,
    convert::Infallible,
    io,
    net::SocketAddr,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::PollSender;

#[derive(Clone)]
struct App {
    clients:
        Arc<tokio::sync::RwLock<HashMap<String, tokio::sync::mpsc::Sender<ClientJsonRpcMessage>>>>,
    transports: tokio::sync::mpsc::UnboundedSender<SseTransport>,
    post_path: Arc<str>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PostQuery {
    session_id: String,
}

async fn post_message(
    State(app): State<App>,
    Query(query): Query<PostQuery>,
    Json(message): Json<ClientJsonRpcMessage>,
) -> Result<StatusCode, StatusCode> {
    let sender = app
        .clients
        .read()
        .await
        .get(&query.session_id)
        .cloned()
        .ok_or(StatusCode::NOT_FOUND)?;
    sender.send(message).await.map_err(|_| StatusCode::GONE)?;
    Ok(StatusCode::ACCEPTED)
}

async fn open_sse(
    State(app): State<App>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, StatusCode> {
    let session = uuid::Uuid::new_v4().to_string();
    let (from_client_tx, from_client_rx) = tokio::sync::mpsc::channel(64);
    let (to_client_tx, to_client_rx) = tokio::sync::mpsc::channel(64);
    app.clients
        .write()
        .await
        .insert(session.clone(), from_client_tx);
    let transport = SseTransport {
        stream: ReceiverStream::new(from_client_rx),
        sink: PollSender::new(to_client_tx),
        session: session.clone(),
        clients: app.clients.clone(),
    };
    app.transports
        .send(transport)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let endpoint = Event::default()
        .event("endpoint")
        .data(format!("{}?sessionId={session}", app.post_path));
    let stream = futures::stream::once(async move { Ok(endpoint) }).chain(
        ReceiverStream::new(to_client_rx).map(|message| {
            Ok(Event::default()
                .event("message")
                .data(serde_json::to_string(&message).unwrap_or_else(|_| "{}".into())))
        }),
    );
    Ok(Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15))))
}

pub struct SseTransport {
    stream: ReceiverStream<RxJsonRpcMessage<RoleServer>>,
    sink: PollSender<TxJsonRpcMessage<RoleServer>>,
    session: String,
    clients:
        Arc<tokio::sync::RwLock<HashMap<String, tokio::sync::mpsc::Sender<ClientJsonRpcMessage>>>>,
}

impl Sink<TxJsonRpcMessage<RoleServer>> for SseTransport {
    type Error = io::Error;
    fn poll_ready(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.sink.poll_ready_unpin(cx).map_err(io::Error::other)
    }
    fn start_send(
        mut self: Pin<&mut Self>,
        item: TxJsonRpcMessage<RoleServer>,
    ) -> Result<(), Self::Error> {
        self.sink.start_send_unpin(item).map_err(io::Error::other)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.sink.poll_flush_unpin(cx).map_err(io::Error::other)
    }
    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        let result = self.sink.poll_close_unpin(cx).map_err(io::Error::other);
        if result.is_ready() {
            let session = self.session.clone();
            let clients = self.clients.clone();
            tokio::spawn(async move {
                clients.write().await.remove(&session);
            });
        }
        result
    }
}
impl Stream for SseTransport {
    type Item = RxJsonRpcMessage<RoleServer>;
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.stream.poll_next_unpin(cx)
    }
}

pub async fn serve(
    bind: SocketAddr,
    sse_path: &str,
    factory: impl Fn() -> crate::server::MemoryServer + Send + Sync + 'static,
) -> Result<(), Box<dyn std::error::Error>> {
    let post_path = "/message".to_owned();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let app = App {
        clients: Default::default(),
        transports: tx,
        post_path: post_path.clone().into(),
    };
    let router = Router::new()
        .route(sse_path, get(open_sse))
        .route(&post_path, post(post_message))
        .with_state(app);
    let listener = tokio::net::TcpListener::bind(bind).await?;
    eprintln!(
        "[mcp-memory] Legacy SSE listening on http://{}{}",
        listener.local_addr()?,
        sse_path
    );
    let server = tokio::spawn(async move { axum::serve(listener, router).await });
    let factory = Arc::new(factory);
    loop {
        tokio::select! {
            transport = rx.recv() => match transport {
                Some(transport) => {
                    let service = factory();
                    tokio::spawn(async move {
                        use rmcp::ServiceExt;
                        if let Ok(running) = service.serve(transport).await { let _ = running.waiting().await; }
                    });
                }
                None => break,
            },
            _ = tokio::signal::ctrl_c() => { server.abort(); break; }
        }
    }
    Ok(())
}
