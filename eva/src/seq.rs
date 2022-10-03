use crate::core::Core;
use eva_common::op::Op;
use eva_common::prelude::*;
use eva_robots::{
    SequenceActionEntryOwned, SequenceActionOwned, SequenceEntryOwned, SequenceOwned,
};
use lazy_static::lazy_static;
use log::{error, warn};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use uuid::Uuid;

struct SeqFut {
    fut: JoinHandle<()>,
    wait: Duration,
}

lazy_static! {
    static ref FUTS: Mutex<HashMap<Uuid, SeqFut>> = <_>::default();
}

async fn launch_action_entry(core: Arc<Core>, a: SequenceActionOwned) -> EResult<()> {
    let res = core
        .action(
            None,
            &a.oid,
            a.params,
            eva_common::actions::DEFAULT_ACTION_PRIORITY,
            Some(a.wait),
            true,
        )
        .await?;
    if let crate::core::ActionLaunchResult::State(s) = res {
        if s {
            Ok(())
        } else {
            Err(Error::failed("action failed"))
        }
    } else {
        Err(Error::core("invalid core result"))
    }
}
async fn launch_action_sequence_batch(
    core: Arc<Core>,
    batch: Vec<SequenceActionOwned>,
) -> EResult<()> {
    let mut futs = Vec::new();
    for a in batch {
        let fut = tokio::spawn(launch_action_entry(core.clone(), a));
        futs.push(fut);
    }
    let mut ok = true;
    for fut in futs {
        let res = fut.await?;
        if let Err(e) = res {
            error!("{}", e);
            ok = false;
        }
    }
    if ok {
        Ok(())
    } else {
        Err(Error::failed("sequence failed"))
    }
}
async fn launch_action_sequence(
    core: Arc<Core>,
    seq: Vec<SequenceEntryOwned>,
    timeout: Duration,
) -> EResult<()> {
    let op = Op::new(timeout);
    for entry in seq {
        match entry {
            SequenceEntryOwned::Delay(d) => {
                let delay = Duration::from_micros(d);
                op.enough(delay)?;
                async_io::Timer::after(delay).await;
            }
            SequenceEntryOwned::Actions(entry) => match entry {
                SequenceActionEntryOwned::Single(a) => {
                    op.enough(a.wait)?;
                    launch_action_entry(core.clone(), a).await?;
                }
                SequenceActionEntryOwned::Multi(actions) => {
                    if let Some(max) = actions.iter().map(|a| a.wait).max() {
                        op.enough(max)?;
                        launch_action_sequence_batch(core.clone(), actions).await?;
                    }
                }
            },
        }
    }
    Ok(())
}
async fn abort(core: Arc<Core>, entry: SequenceActionEntryOwned) -> EResult<()> {
    warn!("executing sequence abort actions");
    match entry {
        SequenceActionEntryOwned::Single(a) => launch_action_entry(core, a).await,
        SequenceActionEntryOwned::Multi(a) => launch_action_sequence_batch(core, a).await,
    }
}
/// # Panics
///
/// Will panic if futs mutex is poisoned
pub async fn execute(core: Arc<Core>, sequence: SequenceOwned) -> EResult<()> {
    if sequence.max_expected_duration() > sequence.timeout() {
        return Err(Error::failed(
            "max expected seq duration is longer than allowed timeout",
        ));
    }
    let (tx, rx) = oneshot::channel::<EResult<()>>();
    let core_c = core.clone();
    let abort_timeout = sequence.abort_timeout();
    let fut = tokio::spawn(async move {
        let res = launch_action_sequence(core_c, sequence.seq, sequence.timeout).await;
        let _r = tx.send(res);
    });
    FUTS.lock().unwrap().insert(
        sequence.u,
        SeqFut {
            fut,
            wait: abort_timeout,
        },
    );
    let result = rx.await;
    FUTS.lock().unwrap().remove(&sequence.u);
    match result {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => {
            error!("action sequence error: {}", e);
            if let Some(b) = sequence.on_abort {
                abort(core, b).await?;
            }
            Err(e)
        }
        Err(_) => {
            if let Some(b) = sequence.on_abort {
                abort(core, b).await?;
            }
            Err(Error::aborted())
        }
    }
}
/// # Panics
///
/// Will panic if futs mutex is poisoned
pub fn terminate(u: &Uuid) -> EResult<()> {
    if let Some(s) = FUTS.lock().unwrap().remove(u) {
        s.fut.abort();
        Ok(())
    } else {
        Err(Error::not_found("sequence not found"))
    }
}

/// # Panics
///
/// Will panic if futs mutex is poisoned
pub async fn shutdown() {
    let mut max_timeout = Duration::from_secs(0);
    for s in FUTS.lock().unwrap().values() {
        s.fut.abort();
        if s.wait > max_timeout {
            max_timeout = s.wait;
        }
    }
    async_io::Timer::after(max_timeout).await;
}
