use std::cmp;
use std::time::{Duration, Instant};

#[derive(Debug)]
pub struct ActivitySystem {
    last_heard: Instant,
    last_sent: Instant,
    idle_timeout: Duration,
}

impl ActivitySystem {
    pub fn new(idle_timeout: Duration, time: Instant) -> Self {
        Self {
            last_heard: time,
            last_sent: time,
            idle_timeout,
        }
    }

    pub fn received(&mut self, time: Instant) {
        self.last_heard = time;
    }

    pub fn sent(&mut self, time: Instant) {
        self.last_sent = time;
    }

    pub fn is_active(&self, time: Instant) -> bool {
        let last = cmp::max(self.last_heard, self.last_sent);
        time.duration_since(last) < self.idle_timeout
    }
}
