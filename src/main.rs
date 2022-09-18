use axum::extract::ws::*;
use axum::extract::*;
use axum::response::*;
use axum::*;
use chrono::Utc;
use request_repository::{Request, RequestRepository};
use std::sync::{Arc, Mutex as BlockingMutex};
use uuid::Uuid;
mod request_repository;

type SharedRequestRepository = Arc<BlockingMutex<RequestRepository>>;

const WEB_SOCKET_PING_INTERVAL_SECONDS: u64 = 30;
const WEB_SOCKET_PONG_TIMEOUT_SECONDS: u64 = 10;
const REQUEST_REPOSITORY_CLEANUP_INTERVAL_SECONDS: u64 = 60;
const HTTP_SERVER_SOCKET_ADDRESS: &str = "0.0.0.0:8000";

fn main() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("Failed to create tokio runtime");

    runtime.block_on(async {
        let request_repository = Arc::new(BlockingMutex::new(RequestRepository::new()));

        let server_future = run_server(request_repository.clone());
        let cleanup_future = run_cleanup(request_repository.clone());
        let interrupt_future = run_interrupt_listener();

        tokio::select! {
            _ = server_future => {
                eprintln!("Web server exited.");
            }
            _ = cleanup_future => {
                eprintln!("Cleanup loop exited.");
            },
            _ = interrupt_future => {
                println!("Interrupt received.");
            }
        };
    });
}

async fn run_server(request_repository: SharedRequestRepository) {
    let app = Router::new()
        .route("/webhook/:uuid", routing::any(handle_webhook))
        .route("/ws/:uuid", routing::get(handle_websocket))
        .route("/:uuid", routing::get(handle_webhook_page))
        .route("/", routing::get(handle_index_page))
        .layer(Extension(request_repository));

    axum::Server::bind(&HTTP_SERVER_SOCKET_ADDRESS.parse().unwrap())
        .serve(app.into_make_service())
        .await
        .expect("Web server stopped")
}

async fn run_cleanup(request_repository: SharedRequestRepository) {
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(
            REQUEST_REPOSITORY_CLEANUP_INTERVAL_SECONDS,
        ))
        .await;
        println!("Running request repository cleanup.");
        let mut request_repository_lock = request_repository
            .lock()
            .expect("Request repository mutex poisoned");
        request_repository_lock.cleanup();
        drop(request_repository_lock);
    }
}

async fn run_interrupt_listener() {
    tokio::signal::ctrl_c()
        .await
        .expect("Failed to listen for interrupt");
}

async fn handle_webhook(
    Path(uuid): Path<Uuid>,
    Extension(request_repository): Extension<SharedRequestRepository>,
    request: axum::http::Request<body::Body>, // must be last
) {
    let request = Request {
        id: Uuid::new_v4(),
        received_time: Utc::now(),
        method: request.method().to_string(),
        uri: request.uri().to_string(),
        headers: request
            .headers()
            .iter()
            .filter_map(|(name, value)| {
                value
                    .to_str()
                    .ok()
                    .map(|value| (name.to_string(), value.to_string()))
            })
            .collect(),
        body: hyper::body::to_bytes(request.into_body())
            .await
            .map(|bytes| String::from_utf8_lossy(bytes.as_ref()).to_string())
            .unwrap_or_default(),
    };
    println!("Received request: {request:?}");

    let mut request_repository_lock = request_repository
        .lock()
        .expect("Request repository mutex poisoned");

    request_repository_lock.insert(uuid, request);
}

async fn handle_websocket(
    Path(uuid): Path<Uuid>,
    Extension(request_repository): Extension<SharedRequestRepository>,
    ws: WebSocketUpgrade,
) -> impl response::IntoResponse {
    ws.on_upgrade(move |mut socket| async move {
        let (old_requests, mut new_requests_receiver) = {
            let mut request_repository_lock = request_repository
                .lock()
                .expect("Request repository mutex poisoned");

            request_repository_lock.get_requests_and_receiver(uuid)
        };

        // Send old requests (those from requests repository) to the client.
        // While this is happening, new requests might be piling up in the
        // receiver's buffer. If too many new requests come before we send old
        // request to the client, the receiver will lag.
        for request in old_requests {
            let message = serde_json::to_string(&request).expect("Could not serialize request");
            let res = socket.send(extract::ws::Message::Text(message)).await;
            if res.is_err() {
                // Failed to send message to client. Drop socket.
                return;
            }
        }

        async fn timeout_at(time: Option<tokio::time::Instant>) {
            if let Some(time) = time {
                tokio::time::sleep_until(time).await;
            } else {
                std::future::pending().await
            }
        }

        let mut ping_interval = tokio::time::interval(std::time::Duration::from_secs(WEB_SOCKET_PING_INTERVAL_SECONDS));
        let mut pong_timeout_at = None;

        loop {
            tokio::select! {
                new_request_result = new_requests_receiver.recv() => {
                    match new_request_result {
                        Ok(request) => {
                            let text = serde_json::to_string(&request).expect("Could not serialize request");
                            let send_result = socket.send(extract::ws::Message::Text(text)).await;
                            if let Err(error) = send_result {
                                eprintln!("Failed to send request to client: {error}. Closing connection.");
                                return;
                            }
                        },
                        Err(error) => {
                            eprintln!("Failed to receive request: {error}. Closing connection.");
                            return;
                        }
                    }
                }
                socket_receive_result = socket.recv() => {
                    match socket_receive_result {
                        Some(Ok(Message::Pong(_))) => {
                            if pong_timeout_at.is_some() {
                                println!("Received pong. Keeping connection.");
                                pong_timeout_at = None;
                            } else {
                                println!("Received unexpected pong. Closing connection.");
                                return;
                            }
                        }
                        Some(Ok(Message::Close(_close_frame))) => {
                            println!("Connection closed.");
                            return;
                        }
                        Some(Ok(_other_message)) => {
                            println!("Ignoring message from client.");
                        }
                        Some(Err(error)) => {
                            eprintln!("Failed to receive message: {error}. Closing connection.");
                            return;
                        }
                        None => {
                            println!("Connection closed.");
                            return;
                        }
                    }
                }
                _ = ping_interval.tick() => {
                    println!("Sending ping to client.");
                    let send_result = socket.send(extract::ws::Message::Ping(Vec::new())).await;
                    if send_result.is_err() {
                        println!("Failed to send ping to client. Closing connection");
                        return;
                    }
                    pong_timeout_at = Some(tokio::time::Instant::now() + std::time::Duration::from_secs(WEB_SOCKET_PONG_TIMEOUT_SECONDS));
                }
                _ = timeout_at(pong_timeout_at) => {
                    println!("Pong not received in time. Closing connection.");
                }
            }
        }
    })
}

async fn handle_index_page() -> Html<&'static str> {
    Html(include_str!("../pages/index.html"))
}

async fn handle_webhook_page(Path(_uuid): Path<Uuid>) -> Html<&'static str> {
    Html(include_str!("../pages/webhook.html"))
}
