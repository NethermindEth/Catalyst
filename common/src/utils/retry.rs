use std::time::{Duration, SystemTime};

/// Retries an operation with exponential backoff until a timeout is reached
/// This version allows custom error types that can be converted to anyhow::Error
///
/// # Arguments
/// * `operation` - The async operation to retry
/// * `base_delay_ms` - Base delay in milliseconds for exponential backoff
/// * `max_delay_ms` - Maximum delay between retries in milliseconds
/// * `timeout` - Total timeout duration after which to stop retrying
///
/// # Returns
/// * `Result<T, anyhow::Error>` - The result of the operation or the last error encountered
pub async fn backoff_retry_with_timeout<T, E, F, Fut>(
    operation: F,
    base_delay: Duration,
    max_delay: Duration,
    timeout: Duration,
) -> Result<T, anyhow::Error>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<T, E>>,
    E: std::fmt::Display + Send + Sync + 'static,
{
    let start_time = SystemTime::now();
    let mut current_delay = base_delay;
    let mut last_error: Option<anyhow::Error> = None;

    loop {
        if start_time.elapsed().unwrap_or_else(|_| {
            tracing::error!("backoff_retry_with_timeout: start_time.elapsed() failed, using 0");
            Duration::from_secs(0)
        }) >= timeout
        {
            let error_msg = if let Some(ref err) = last_error {
                format!("Operation timed out after {timeout:?}, last error: {err}")
            } else {
                format!("Operation timed out after {timeout:?}")
            };
            return Err(anyhow::anyhow!(error_msg));
        }

        match operation().await {
            Ok(value) => return Ok(value),
            Err(e) => {
                last_error = Some(anyhow::anyhow!("{}", e));
                tokio::time::sleep(current_delay).await;

                // Calculate next delay with exponential backoff
                current_delay = std::cmp::min(current_delay * 2, max_delay);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn backoff_retry_with_timeout_pass_test() {
        let result = backoff_retry_with_timeout(
            || async { Ok::<(), anyhow::Error>(()) },
            Duration::from_millis(1),
            Duration::from_millis(10),
            Duration::from_millis(100),
        )
        .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn backoff_retry_with_timeout_fail_test() {
        let result: Result<(), anyhow::Error> = backoff_retry_with_timeout(
            || async { Err(anyhow::anyhow!("test error")) },
            Duration::from_millis(1),
            Duration::from_millis(10),
            Duration::from_millis(100),
        )
        .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("test error"));
    }
}
