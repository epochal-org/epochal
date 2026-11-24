use axum::Router;
use tokio::net::TcpListener;

/// Serves `app` on a free local port and returns its base URL.
pub async fn spawn(app: Router) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    url
}
