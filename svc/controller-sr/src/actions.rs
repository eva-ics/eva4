use crate::common::init_cmd_options;
use eva_common::prelude::*;
use eva_sdk::controller::{format_action_topic, Action};
use eva_sdk::prelude::*;
use serde::Deserialize;
use std::path::Path;
use std::time::Duration;

async fn run_action(
    command: &str,
    args: Vec<String>,
    cmd_options: &bmart::process::Options<'_>,
    timeout: Duration,
) -> Result<Option<String>, (i32, Option<String>, String)> {
    trace!("executing {}", command);
    match bmart::process::command(command, args, timeout, cmd_options.clone()).await {
        Ok(res) => {
            let code = res.code.unwrap_or(-1);
            let output = res.out.join("\n");
            for out in res.out {
                info!("{}: {}", command, out);
            }
            for err in &res.err {
                error!("{}: {}", command, err);
            }
            if code == 0 {
                Ok(Some(output))
            } else {
                error!("{}: exit code {}", command, code);
                Err((code, Some(output), res.err.join("\n")))
            }
        }
        Err(e) => {
            error!("{}: {}", command, e);
            Err((-2, None, e.to_string()))
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionMap {
    command: String,
    #[serde(
        default,
        deserialize_with = "eva_common::tools::de_opt_float_as_duration"
    )]
    timeout: Option<Duration>,
    #[serde(default)]
    update_after: bool,
}

async fn handle_action(
    mut action: Action,
    tx: async_channel::Sender<(String, Vec<u8>)>,
    command: &str,
    action_topic: &str,
    cmd_options: &bmart::process::Options<'_>,
    timeout: Duration,
) -> EResult<()> {
    if !crate::ACTT
        .get()
        .unwrap()
        .remove(action.oid(), action.uuid())?
    {
        let payload_canceled = pack(&action.event_canceled())?;
        tx.send((action_topic.to_owned(), payload_canceled))
            .await
            .map_err(Error::io)?;
        return Ok(());
    }
    let payload_running = pack(&action.event_running())?;
    tx.send((action_topic.to_owned(), payload_running))
        .await
        .map_err(Error::io)?;
    let payload = if let Ok(params) = action.take_unit_params() {
        let mut args = vec![params.status.to_string()];
        if let ValueOptionOwned::Value(value) = params.value {
            args.push(value.to_string());
        }
        match run_action(command, args, cmd_options, timeout).await {
            Ok(v) => action.event_completed(v.map(Value::String)),
            Err(e) => {
                // allow truncation as shell exit codes always fit i16
                #[allow(clippy::cast_possible_truncation)]
                action.event_failed(e.0 as i16, e.1.map(Value::String), Some(Value::String(e.2)))
            }
        }
    } else {
        action.event_failed(-1, None, Some(to_value("invalid action payload")?))
    };
    tx.send((action_topic.to_owned(), pack(&payload)?))
        .await
        .map_err(Error::io)?;
    Ok(())
}

pub fn start_action_handler(
    oid: &OID,
    map: ActionMap,
    receiver: async_channel::Receiver<Action>,
    tx: async_channel::Sender<(String, Vec<u8>)>,
) {
    let action_topic = format_action_topic(oid);
    let command = if map.command.starts_with('/') {
        Path::new(&map.command).to_path_buf()
    } else {
        let mut cmd = Path::new(crate::EVA_DIR.get().unwrap()).to_path_buf();
        cmd.push(map.command);
        cmd
    };
    let timeout = map
        .timeout
        .unwrap_or_else(|| *crate::TIMEOUT.get().unwrap());
    let oid = oid.clone();
    tokio::spawn(async move {
        let kind_str = oid.kind().to_string();
        let cmd_options = init_cmd_options(&oid, &kind_str);
        let command_str = command.to_string_lossy();
        while let Ok(action) = receiver.recv().await {
            if let Err(e) = handle_action(
                action,
                tx.clone(),
                &command_str,
                &action_topic,
                &cmd_options,
                timeout,
            )
            .await
            {
                error!("action error: {}", e);
            }
            if map.update_after {
                if let Some(trx) = crate::UPDATE_TRIGGERS.lock().unwrap().get(&oid) {
                    let _r = trx.try_send(());
                }
            }
        }
    });
}
