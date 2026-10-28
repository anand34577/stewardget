//! Asynchronous token-bucket rate limiter.
//!
//! Callers take tokens *first* and then sleep off any debt. This keeps the hot
//! path to a single short critical section and naturally queues concurrent
//! connections fairly: whoever drives the bucket further into debt waits longer.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

#[derive(Debug)]
pub(crate) struct RateLimiter {
    /// Bytes per second; `0` = unlimited.
    rate: AtomicU64,
    bucket: Mutex<Bucket>,
    /// Wakes connections sleeping off a debt when the rate changes: lifting a
    /// limit must not leave them asleep for what the old rate asked.
    changed: tokio::sync::Notify,
}

#[derive(Debug)]
struct Bucket {
    tokens: f64,
    last: Instant,
}

impl RateLimiter {
    pub fn new(rate: u64) -> Self {
        Self {
            rate: AtomicU64::new(rate),
            bucket: Mutex::new(Bucket { tokens: 0.0, last: Instant::now() }),
            changed: tokio::sync::Notify::new(),
        }
    }

    pub fn rate(&self) -> u64 {
        self.rate.load(Ordering::Relaxed)
    }

    pub fn set_rate(&self, rate: u64) {
        // Saving unrelated settings must not reset the bucket (and forgive debt).
        if self.rate.swap(rate, Ordering::Relaxed) == rate {
            return;
        }
        let mut b = self.bucket.lock().unwrap();
        b.tokens = 0.0;
        b.last = Instant::now();
        drop(b);
        self.changed.notify_waiters();
    }

    /// Accounts for `n` bytes, sleeping as long as needed to honour the rate.
    pub async fn acquire(&self, n: usize) {
        let rate = self.rate();
        if rate == 0 {
            return;
        }
        // Registered before the bucket is read, so a rate change in between isn't missed.
        let changed = self.changed.notified();
        tokio::pin!(changed);
        changed.as_mut().enable();
        let wait = {
            let mut b = self.bucket.lock().unwrap();
            let now = Instant::now();
            let rate = rate as f64;
            // Allow a burst of ~100 ms worth of data (at least 16 KiB) to keep
            // syscalls efficient without making the cap visibly bursty.
            let burst = (rate / 10.0).max(16.0 * 1024.0);
            b.tokens = (b.tokens + now.duration_since(b.last).as_secs_f64() * rate).min(burst);
            b.last = now;
            b.tokens -= n as f64;
            if b.tokens < 0.0 { Duration::from_secs_f64(-b.tokens / rate) } else { Duration::ZERO }
        };
        if !wait.is_zero() {
            // A new rate starts a fresh bucket; the old debt is forgiven.
            tokio::select! {
                _ = tokio::time::sleep(wait) => {}
                _ = changed => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn unlimited_is_instant() {
        let l = RateLimiter::new(0);
        let t = Instant::now();
        for _ in 0..1000 {
            l.acquire(1 << 20).await;
        }
        assert!(t.elapsed() < Duration::from_millis(50));
    }

    #[tokio::test]
    async fn lifting_the_limit_wakes_sleepers() {
        let l = std::sync::Arc::new(RateLimiter::new(1024)); // 1 KiB/s
        let sleeper = {
            let l = std::sync::Arc::clone(&l);
            tokio::spawn(async move { l.acquire(60 * 1024).await }) // a minute of debt
        };
        tokio::time::sleep(Duration::from_millis(50)).await;
        let t = Instant::now();
        l.set_rate(0);
        tokio::time::timeout(Duration::from_secs(2), sleeper).await.expect("still asleep").unwrap();
        assert!(t.elapsed() < Duration::from_secs(1));
    }

    #[tokio::test]
    async fn limits_throughput() {
        let l = RateLimiter::new(1 << 20); // 1 MiB/s
        let t = Instant::now();
        for _ in 0..8 {
            l.acquire(64 * 1024).await; // 512 KiB total
        }
        let secs = t.elapsed().as_secs_f64();
        assert!((0.35..0.8).contains(&secs), "took {secs}s");
    }
}
