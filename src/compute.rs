use crate::error::Failure;

pub async fn off_the_runtime<T: Send + 'static>(
    doing: &'static str,
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, Failure> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|stopped| match stopped.is_panic() {
            true => Failure::Panicked { doing },
            false => Failure::Internal { doing },
        })
}
