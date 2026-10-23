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
}

#[derive(Debug)]
struct Bucket {
    tokens: f64,
    last: Instant,
}

impl RateLimiter {
    pub fn new(rate: u64) -> Self {
        Self { rate: AtomicU64::new(rate), bucket: Mutex::new(Bucket { tokens: 0.0, last: Instant::now() }) }
    }

    pub fn rate(&self) -> u64 {
        self.rate.load(Ordering::Relaxed)
    }

    pub fn set_rate(&self, rate: u64) {
        self.rate.store(rate, Ordering::Relaxed);
        let mut b = self.bucket.lock().unwrap();
        b.tokens = 0.0;
        b.last = Instant::now();
    }

    /// Accounts for `n` bytes, sleeping as long as needed to honour the rate.
    pub async fn acquire(&self, n: usize) {
        let rate = self.rate();
        if rate == 0 {
            return;
        }
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
            tokio::time::sleep(wait).await;
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
