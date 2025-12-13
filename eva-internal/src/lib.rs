use std::time::Duration;

use eva_common::time::{now, Time};
use tokio::time::Interval;

pub struct RtcSyncedInterval {
    interval: Interval,
    d: Duration,
    tick_prev: u64,
}

impl RtcSyncedInterval {
    pub fn new(d: Duration) -> Self {
        let mut interval = tokio::time::interval(if d >= Duration::from_secs(1) {
            Duration::from_millis(200)
        } else {
            d
        });
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        Self {
            interval,
            d,
            tick_prev: 0,
        }
    }
    pub async fn tick(&mut self) -> Time {
        if self.d >= Duration::from_secs(1) {
            loop {
                self.interval.tick().await;
                let t_secs = now();
                if t_secs % self.d.as_secs() == 0 && t_secs != self.tick_prev {
                    self.tick_prev = t_secs;
                    break Time::from_timestamp(t_secs as f64);
                }
            }
        } else {
            self.interval.tick().await;
            Time::now()
        }
    }
}
