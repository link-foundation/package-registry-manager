//! Shared API pacing and bounded retry delays. No credentials are traced.
use std::time::Duration;
use tokio::{sync::Mutex, time::Instant};

static CRATES_NEXT: Mutex<Option<Instant>> = Mutex::const_new(None);

/// Serialize crates.io request starts across every client in this process.
pub async fn crates_permit() {
    let mut next = CRATES_NEXT.lock().await;
    if let Some(start) = *next {
        tokio::time::sleep_until(start).await;
    }
    *next = Some(Instant::now() + Duration::from_secs(1));
}

/// Honor Retry-After seconds or HTTP dates, with exponential backoff as the floor.
#[must_use]
pub fn retry_delay(value: Option<&str>, attempt: u32) -> Duration {
    let seconds = value
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_secs)
        .or_else(|| {
            value
                .and_then(|value| chrono::DateTime::parse_from_rfc2822(value).ok())
                .and_then(|until| {
                    (until.with_timezone(&chrono::Utc) - chrono::Utc::now())
                        .to_std()
                        .ok()
                })
        })
        .unwrap_or_default();
    Duration::from_secs(1_u64 << attempt.min(10)).max(seconds)
}
