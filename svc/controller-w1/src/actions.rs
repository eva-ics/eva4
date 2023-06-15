use crate::common;
use crate::w1;
use eva_common::op::Op;
use eva_common::payload::pack;
use eva_common::prelude::*;
use eva_sdk::controller::{format_action_topic, Action};
use log::error;
use std::sync::atomic;
use std::sync::Arc;

#[allow(clippy::cast_possible_truncation)]
async fn run_action(
    oid: &OID,
    map: &common::ActionMap,
    params: eva_common::actions::UnitParams,
    op: Op,
) -> EResult<()> {
    let verify = crate::ACTIONS_VERIFY.load(atomic::Ordering::SeqCst);
    let retries = crate::DEFAULT_RETRIES.load(atomic::Ordering::SeqCst);
    let val = if map.need_transform() {
        let val_f64: f64 = params.value.try_into()?;
        Value::F64(map.transform_value(val_f64, oid)?)
    } else {
        params.value
    };
    w1::set(
        Arc::new(map.path().to_owned()),
        Arc::new(val.to_string()),
        op.timeout()?,
        verify,
        retries,
    )
    .await?;
    Ok(())
}

async fn handle_action(
    mut action: Action,
    map: &common::ActionMap,
    tx: async_channel::Sender<(String, Vec<u8>)>,
) -> EResult<()> {
    let action_topic = format_action_topic(action.oid());
    if !crate::ACTT
        .get()
        .unwrap()
        .remove(action.oid(), action.uuid())?
    {
        let payload_canceled = pack(&action.event_canceled())?;
        tx.send((action_topic.clone(), payload_canceled))
            .await
            .map_err(Error::io)?;
        return Ok(());
    }
    let op = action.op();
    let payload_running = pack(&action.event_running())?;
    tx.send((action_topic.clone(), payload_running))
        .await
        .map_err(Error::io)?;
    let payload = if let Ok(params) = action.take_unit_params() {
        if let Err(e) = run_action(action.oid(), map, params, op).await {
            action.event_failed(1, None, Some(Value::String(e.to_string())))
        } else {
            action.event_completed(None)
        }
    } else {
        action.event_failed(-1, None, Some(to_value("invalid action payload")?))
    };
    tx.send((action_topic, pack(&payload)?))
        .await
        .map_err(Error::io)?;
    Ok(())
}

pub fn start_action_handler(
    map: common::ActionMap,
    receiver: async_channel::Receiver<Action>,
    tx: async_channel::Sender<(String, Vec<u8>)>,
) {
    let action_map = Arc::new(map);
    tokio::spawn(async move {
        while let Ok(action) = receiver.recv().await {
            if let Err(e) = handle_action(action, &action_map, tx.clone()).await {
                error!("action error: {}", e);
            }
        }
    });
}
