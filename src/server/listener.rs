//! Share the desktop's reserved socket without duplicating its OS handle.
//! A duplicated Windows listener can accept connections whose asynchronous
//! reads never complete. Keep one Tokio listener across server restarts.

use std::{io, net::SocketAddr, sync::Arc};
use tokio::{net::TcpListener, sync::Mutex};

#[derive(Clone)]
pub(crate) struct SharedTcpListener {
    socket: Arc<Mutex<TcpListener>>,
    address: SocketAddr,
}

impl SharedTcpListener {
    pub fn new(socket: TcpListener) -> io::Result<Self> {
        Ok(Self {
            address: socket.local_addr()?,
            socket: Arc::new(Mutex::new(socket)),
        })
    }

    #[cfg(any(feature = "tauri", test))]
    pub fn from_std(socket: std::net::TcpListener) -> io::Result<Self> {
        Self::new(TcpListener::from_std(socket)?)
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        Ok(self.address)
    }
}

impl axum::serve::Listener for SharedTcpListener {
    type Io = tokio::net::TcpStream;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        // Reuse Axum's accept-error handling. Only the running server accepts;
        // the desktop's other reference holds the port during a restart.
        axum::serve::Listener::accept(&mut *self.socket.lock().await).await
    }

    fn local_addr(&self) -> io::Result<Self::Addr> {
        self.local_addr()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn a_reserved_listener_serves_http_before_and_after_restart() {
        let socket = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        socket.set_nonblocking(true).unwrap();
        let retained = SharedTcpListener::from_std(socket).unwrap();
        let address = retained.local_addr().unwrap();
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap();

        for _ in 0..2 {
            let listener = retained.clone();
            let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
            let task = tokio::spawn(async move {
                let router = axum::Router::new()
                    .route("/", axum::routing::get(|| async { "desktop ready" }));
                axum::serve(listener, router)
                    .with_graceful_shutdown(async {
                        stopped.await.ok();
                    })
                    .await
                    .unwrap();
            });
            // Let accept wait before connecting: a socket with a connection
            // already queued can hide the Windows duplicate-handle failure.
            tokio::time::sleep(Duration::from_millis(25)).await;
            assert_eq!(
                client
                    .get(format!("http://{address}/"))
                    .send()
                    .await
                    .unwrap()
                    .error_for_status()
                    .unwrap()
                    .text()
                    .await
                    .unwrap(),
                "desktop ready"
            );
            stop.send(()).unwrap();
            tokio::time::timeout(Duration::from_secs(5), task)
                .await
                .unwrap()
                .unwrap();
            assert!(
                std::net::TcpListener::bind(address).is_err(),
                "the port stays reserved between servers"
            );
        }
        drop(retained);
        assert!(std::net::TcpListener::bind(address).is_ok());
    }
}
