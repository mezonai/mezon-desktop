use std::time::Duration;

use anyhow::Result;
use gpui::BackgroundExecutor;
use mezon_client::AppApi;

/// Retry an async fallible operation up to `attempts` times.
/// Sleeps `delay` between attempts. Returns the value from the first success.
pub async fn retry<T, F, Fut>(
    executor: &BackgroundExecutor,
    attempts: usize,
    delay: Duration,
    f: F,
) -> Result<T>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
{
    for i in 0..attempts {
        match f().await {
            Ok(val) => return Ok(val),
            Err(_) => {
                if i + 1 < attempts {
                    executor.timer(delay).await;
                }
            }
        }
    }
    anyhow::bail!("retry: all {attempts} attempts failed")
}

/// Check WebSocket connection with default retry (5 attempts, 1s interval).
pub async fn check_connection(executor: &BackgroundExecutor, api: &AppApi) -> Result<()> {
    let api = api.clone();
    retry(executor, 5, Duration::from_millis(1000), move || {
        let api = api.clone();
        async move {
            anyhow::ensure!(api.is_open().await, "socket not open");
            api.ping_roundtrip().await
        }
    })
    .await
}
