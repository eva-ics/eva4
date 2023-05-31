use crate::common;
use crate::conv::ValueConv;
use eva_common::op::Op;
use eva_common::payload::pack;
use eva_common::prelude::*;
use eva_sdk::controller::{format_action_topic, Action};
use log::error;
use opcua::types::Variant;
use std::sync::atomic;
use std::sync::Arc;

#[allow(clippy::cast_possible_truncation)]
async fn run_action(
    oid: &OID,
    map: &common::ActionMap,
    params: eva_common::actions::UnitParams,
    op: Op,
) -> EResult<()> {
    let retries = crate::DEFAULT_RETRIES.load(atomic::Ordering::SeqCst);
    let t = opcua::client::prelude::DateTime::now();
    if let Some(s) = map.status() {
        let st = if s.need_transform() {
            let val_f64 = f64::from(params.status);
            let val = s.transform_value(val_f64, oid)?;
            val.trunc() as i16
        } else {
            params.status
        };
        let mut val = Value::I16(st);
        let range = s.range();
        if range.is_some() {
            val = Value::Seq(vec![val]);
        }
        crate::comm::write(
            s.node().clone(),
            range,
            Variant::from_eva_value(val, s.tp())?,
            op.timeout()?,
            retries,
            Some(t),
        )
        .await?;
    }
    if let Some(v) = map.value() {
        if let ValueOptionOwned::Value(value) = params.value {
            let val = if v.need_transform() {
                let val_f64: f64 = value.try_into()?;
                Value::F64(v.transform_value(val_f64, oid)?)
            } else {
                value
            };
            crate::comm::write(
                v.node().clone(),
                v.range(),
                Variant::from_eva_value(val, v.tp())?,
                op.timeout()?,
                retries,
                Some(t),
            )
            .await?;
        }
    }
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
