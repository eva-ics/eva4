use crate::common::Pull;
use crate::w1;
use eva_common::events::RawStateEventOwned;
use eva_common::payload::pack;
use eva_common::prelude::*;
use eva_sdk::controller::{format_raw_state_topic, RawStateCache};
use eva_sdk::prelude::err_logger;
use log::{error, trace, warn};
use std::sync::atomic;
use std::sync::Arc;
use std::time::Instant;

err_logger!();

pub async fn launch(
    config: &Pull,
    tx: async_channel::Sender<(String, Vec<u8>)>,
    raw_state_cache: Arc<RawStateCache>,
) {
    let interval = config.interval();
    log::debug!(
        "starting puller for {}, interval: {:?}",
        config.oid(),
        interval
    );
    let mut ticker = tokio::time::interval(config.interval());
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut last_ticked: Option<Instant> = None;
    let oid = config.oid();
    loop {
        let t = ticker.tick().await.into_std();
        if let Some(prev) = last_ticked {
            if t - prev > interval {
                warn!("{} puller timeout", oid);
            }
        }
        let ev_prep = match pull(config).await {
            Ok(Some(ev)) => Some((
                ev,
                if let Some(v) = config.value() {
                    v.value_delta()
                } else {
                    None
                },
            )),
            Ok(None) => {
                warn!("Nothing pulled for {}", oid);
                None
            }
            Err(e) => {
                error!("pull error for {}: {}", oid, e);
                Some((
                    RawStateEventOwned::new0(eva_common::ITEM_STATUS_ERROR),
                    None,
                ))
            }
        };
        if let Some((raw_state, value_delta)) = ev_prep {
            if raw_state_cache.push_check(oid, &raw_state, value_delta) {
                match pack(&raw_state) {
                    Ok(payload) => {
                        if let Err(e) = tx.try_send((format_raw_state_topic(oid), payload)) {
                            error!("state queue error for {}: {}", oid, e);
                        }
                    }
                    Err(e) => error!("state serialization error for {}: {}", oid, e),
                }
            }
        }
        last_ticked.replace(t);
    }
}

#[allow(clippy::cast_possible_truncation)]
async fn pull(config: &Pull) -> EResult<Option<RawStateEventOwned>> {
    trace!("Pulling {}", config.oid());
    let mut raw_state: Option<RawStateEventOwned> = None;
    if let Some(p_status) = config.status() {
        trace!("pulling {} status from {}", config.oid(), p_status.path());
        let st_str = w1::get(
            Arc::new(p_status.path().to_owned()),
            *crate::TIMEOUT.get().unwrap(),
            crate::DEFAULT_RETRIES.load(atomic::Ordering::SeqCst),
        )
        .await?;
        let st_val: Value = st_str.parse().unwrap();
        let mut status: ItemStatus = st_val.try_into()?;
        if p_status.need_transform() {
            let val_f64 = f64::from(status);
            if let Ok(n) = p_status.transform_value(val_f64, config.oid()).log_err() {
                status = n.trunc() as i16;
            }
        }
        raw_state.replace(RawStateEventOwned::new0(status));
    }
    if let Some(p_value) = config.value() {
        trace!("pulling {} value from {}", config.oid(), p_value.path());
        let val_str = w1::get(
            Arc::new(p_value.path().to_owned()),
            *crate::TIMEOUT.get().unwrap(),
            crate::DEFAULT_RETRIES.load(atomic::Ordering::SeqCst),
        )
        .await?;
        let mut value: Value = val_str.parse().unwrap();
        if p_value.need_transform() {
            if let Ok(val_f64) = TryInto::<f64>::try_into(value.clone()).log_err() {
                if let Ok(n) = p_value.transform_value(val_f64, config.oid()).log_err() {
                    value = Value::F64(n);
                }
            }
        }
        if let Some(ref mut event) = raw_state {
            event.value = ValueOptionOwned::Value(value);
        } else {
            raw_state.replace(RawStateEventOwned::new(1, value));
        }
    }
    Ok(raw_state)
}
