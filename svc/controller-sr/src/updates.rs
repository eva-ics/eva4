use crate::common::{self, init_cmd_options, init_cmd_options_basic, safe_run_macro};
use eva_common::common_payloads::ValueOrList;
use eva_common::events::{Force, RAW_STATE_TOPIC, RawStateEventOwned};
use eva_common::prelude::*;
use eva_sdk::prelude::*;
use serde::Deserialize;
use std::collections::hash_map::Entry;
use std::path::Path;
use std::time::Duration;

err_logger!();

const RESTART_DELAY: u64 = 2;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Update {
    command: String,
    oid: ValueOrList<OID>,
    #[serde(deserialize_with = "eva_common::tools::de_float_as_duration")]
    interval: Duration,
    #[serde(
        default,
        deserialize_with = "eva_common::tools::de_opt_float_as_duration"
    )]
    timeout: Option<Duration>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdatePipe {
    command: String,
    process: OID,
}

fn register_update_trigger(oid: &OID, tx: async_channel::Sender<()>) {
    if let Entry::Vacant(v) = crate::UPDATE_TRIGGERS.lock().unwrap().entry(oid.clone()) {
        v.insert(tx);
    } else {
        warn!("{} is in more than one updates", oid);
    }
}

pub fn trigger_update(oid: &OID) -> EResult<()> {
    if let Some(ch) = crate::UPDATE_TRIGGERS.lock().unwrap().get(oid) {
        let _r = ch.try_send(());
        Ok(())
    } else {
        Err(Error::not_found(format!("no update trigger for {}", oid)))
    }
}

pub async fn update_handler(update: Update) -> EResult<()> {
    let command = if update.command.starts_with('/') {
        Path::new(&update.command).to_path_buf()
    } else {
        let mut cmd = Path::new(crate::EVA_DIR.get().unwrap()).to_path_buf();
        cmd.push(update.command);
        cmd
    };
    let timeout = update
        .timeout
        .unwrap_or_else(|| *crate::TIMEOUT.get().unwrap());
    let kind_str;
    let cmd_options = if let ValueOrList::Single(ref oid) = update.oid {
        kind_str = oid.kind().to_string();
        init_cmd_options(oid, &kind_str)
    } else {
        init_cmd_options_basic()
    };
    let mut inter = tokio::time::interval(update.interval);
    inter.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let args: &[&str] = &[];
    let command_str = command.to_string_lossy();
    macro_rules! mark_err_all {
        () => {
            for oid in update.oid.iter() {
                mark_error(oid).await.log_ef();
            }
        };
    }
    let (tx, rx) = async_channel::bounded(1);
    for oid in update.oid.iter() {
        register_update_trigger(oid, tx.clone());
    }
    tokio::spawn(async move {
        loop {
            inter.tick().await;
            let _r = tx.try_send(());
        }
    });
    while let Ok(()) = rx.recv().await {
        trace!("executing {}", command_str);
        match bmart::process::command(command_str.as_ref(), args, timeout, cmd_options.clone())
            .await
        {
            Ok(res) => {
                let code = res.code.unwrap_or(-1);
                for err in res.err {
                    error!("{}: {}", command_str, err);
                }
                if code == 0 {
                    for (i, oid) in update.oid.iter().enumerate() {
                        if let Some(data) = res.out.get(i) {
                            if let Err(e) = process_data(oid, data).await {
                                error!("{}: {}", command_str, e);
                                mark_error(oid).await.log_ef();
                            }
                        } else {
                            error!("{}: no data for {}", command_str, oid);
                            mark_error(oid).await.log_ef();
                        }
                    }
                } else {
                    error!("{}: exit code {}", command_str, code);
                    mark_err_all!();
                }
            }
            Err(e) => {
                error!("{}: {}", command_str, e);
                mark_err_all!();
            }
        }
    }
    Ok(())
}

async fn mark_error(oid: &OID) -> EResult<()> {
    let ev = RawStateEventOwned {
        status: eva_common::ITEM_STATUS_ERROR,
        value: ValueOptionOwned::No,
        force: Force::Full,
        ..RawStateEventOwned::default()
    };
    let raw_topic = format!("{}{}", RAW_STATE_TOPIC, oid.as_path());
    eapi_bus::client()
        .lock()
        .await
        .publish(&raw_topic, pack(&ev)?.into(), QoS::No)
        .await?;
    Ok(())
}

async fn process_data(oid: &OID, data: &str) -> EResult<()> {
    let mut sp = data.splitn(2, ' ');
    let status: ItemStatus = sp.next().unwrap().parse()?;
    let value: ValueOptionOwned = if let Some(value) = sp.next() {
        ValueOptionOwned::Value(value.parse().unwrap())
    } else {
        ValueOptionOwned::No
    };
    debug!("{}: {} / {:?}", oid, status, value);
    let ev = RawStateEventOwned {
        status,
        value,
        ..RawStateEventOwned::default()
    };
    let raw_topic = format!("{}{}", RAW_STATE_TOPIC, oid.as_path());
    eapi_bus::client()
        .lock()
        .await
        .publish(&raw_topic, pack(&ev)?.into(), QoS::No)
        .await?;
    Ok(())
}

async fn launch_update_pipe(config: &UpdatePipe) -> EResult<()> {
    trace!("executing {}", config.command);
    let rx = bmart::process::command_pipe(
        "sh",
        ["-c", &config.command],
        bmart::process::Options::default(),
    )?;
    while let Ok(v) = rx.recv().await {
        match v {
            bmart::process::CommandPipeOutput::Stdout(line) => {
                let kwargs = common::LineData { line: Some(line) };
                let params = common::ParamsRun {
                    i: &config.process,
                    params: common::LParams { kwargs },
                    wait: eapi_bus::timeout(),
                };
                safe_run_macro(params).await.log_ef();
            }
            bmart::process::CommandPipeOutput::Stderr(e) => {
                error!("{}", e);
            }
            bmart::process::CommandPipeOutput::Terminated(_) => {
                break;
            }
        }
    }
    Ok(())
}

pub async fn run_update_pipe(config: UpdatePipe) {
    loop {
        let res = launch_update_pipe(&config).await;
        if eva_sdk::service::svc_is_terminating() {
            break;
        }
        if let Err(e) = res {
            warn!("Error running command \"{}\": {}", config.command, e);
        } else {
            warn!("Command \"{}\" exited", config.command);
        }
        let kwargs = common::LineData { line: None };
        let params = common::ParamsRun {
            i: &config.process,
            params: common::LParams { kwargs },
            wait: eapi_bus::timeout(),
        };
        safe_run_macro(params).await.log_ef();
        warn!("Restarting the command in {} seconds", RESTART_DELAY);
        tokio::time::sleep(Duration::from_secs(RESTART_DELAY)).await;
    }
}
