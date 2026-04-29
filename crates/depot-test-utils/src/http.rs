use anyhow::Result;
use axum::Router;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

pub struct TestServer {
    pub base_url: String,
    handle: JoinHandle<()>,
}

impl TestServer {
    pub async fn spawn(app: Router) -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;

        let handle = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("test server should stay alive");
        });

        Ok(Self {
            base_url: format!("http://{address}"),
            handle,
        })
    }

    pub async fn spawn_with_known_url<F>(build_app: F) -> Result<Self>
    where
        F: FnOnce(String) -> Router,
    {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let base_url = format!("http://{address}");
        let app = build_app(base_url.clone());
        let handle = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("test server should stay alive");
        });
        Ok(Self { base_url, handle })
    }

    pub fn url(&self, path: impl AsRef<str>) -> String {
        format!(
            "{}/{}",
            self.base_url.trim_end_matches('/'),
            path.as_ref().trim_start_matches('/')
        )
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.handle.abort();
    }
}
