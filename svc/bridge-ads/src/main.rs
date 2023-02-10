use ::ads::AmsAddr;
use eva_common::events::{RawStateEvent, RAW_STATE_TOPIC};
use eva_common::prelude::*;
use eva_sdk::prelude::*;
use eva_sdk::service::poc;
use eva_sdk::service::set_poc;
use lazy_static::lazy_static;
use once_cell::sync::OnceCell;
use serde::Deserialize;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

err_logger!();

mod ads;
mod common;
mod eapi;

use crate::ads::ParseAmsNetId;

#[macro_export]
macro_rules! get_client {
    () => {
        $crate::ADS_CLIENT.get().unwrap().lock().unwrap()
    };
}

lazy_static! {
    static ref ADS_CLIENT: OnceCell<Mutex<::ads::Client>> = <_>::default();
    static ref TIMEOUT: OnceCell<Duration> = <_>::default();
    static ref VERIFY_DELAY: OnceCell<Option<Duration>> = <_>::default();
    static ref RPC: OnceCell<Arc<RpcClient>> = <_>::default();
}

const AUTHOR: &str = "Bohemia Automation";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const DESCRIPTION: &str = "TwinCAT ADS bridge";

#[cfg(not(feature = "std-alloc"))]
#[global_allocator]
static ALLOC: jemallocator::Jemalloc = jemallocator::Jemalloc;

#[svc_main]
async fn main(mut initial: Initial) -> EResult<()> {
    set_poc(Some(Duration::from_secs(0)));
    let timeout = initial.timeout();
    TIMEOUT
        .set(timeout)
        .map_err(|_| Error::core("Unable to set TIMEOUT"))?;
    #[allow(clippy::cast_possible_truncation)]
    let config: common::Config = common::Config::deserialize(
        initial
            .take_config()
            .ok_or_else(|| Error::invalid_data("config not specified"))?,
    )?;
    let local_id = if let Some(local_ams_netid) = config.local_ams_netid {
        ::ads::Source::Addr(::ads::AmsAddr::new(
            local_ams_netid.ams_net_id()?.into(),
            config.local_ams_port,
        ))
    } else {
        ::ads::Source::Auto
    };
    let ping_device: AmsAddr = ::ads::AmsAddr::new(
        if let Some(netid) = config.ping_ams_netid {
            netid.ams_net_id()?.into()
        } else {
            format!("{}.1.1", config.host).ams_net_id()?.into()
        },
        config.ping_ams_port,
    );
    let mut info = ServiceInfo::new(AUTHOR, VERSION, DESCRIPTION);
    info.add_method(
        ServiceMethod::new("ping")
            .required("net_id")
            .required("port"),
    );
    info.add_method(
        ServiceMethod::new("read")
            .required("net_id")
            .required("port")
            .required("index_group")
            .required("index_offset")
            .required("length"),
    );
    info.add_method(
        ServiceMethod::new("write")
            .required("net_id")
            .required("port")
            .required("index_group")
            .required("index_offset")
            .required("data"),
    );
    info.add_method(
        ServiceMethod::new("write_read")
            .required("net_id")
            .required("port")
            .required("index_group")
            .required("index_offset")
            .required("data")
            .required("length"),
    );
    let rpc = initial.init_rpc(eapi::Handlers::new(info)).await?;
    RPC.set(rpc.clone())
        .map_err(|_| Error::core("Unable to set RPC"))?;
    if initial.is_mode_rtf() {
        if let Some(oid) = config.ping_ads_state {
            let event = RawStateEvent::new0(-1);
            rpc.client()
                .lock()
                .await
                .publish(
                    &format!("{}{}", RAW_STATE_TOPIC, oid.as_path()),
                    pack(&event)?.into(),
                    QoS::No,
                )
                .await?;
        }
        return Ok(());
    }
    let client = rpc.client().clone();
    initial.drop_privileges()?;
    svc_init_logs(&initial, client.clone())?;
    svc_start_signal_handlers();
    info!("connecting");
    let ads_client = tokio::task::spawn_blocking(move || {
        ::ads::client::Client::new(
            (config.host, config.port),
            ::ads::Timeouts::new(timeout),
            local_id,
        )
    })
    .await?
    .map_err(Error::io)?;
    ADS_CLIENT
        .set(Mutex::new(ads_client))
        .map_err(|_| Error::core("Unable to set ADS_CLIENT"))?;
    tokio::spawn(async move {
        if let Err(e) = crate::ads::ping_worker(ping_device, timeout, config.ping_ads_state).await {
            error!("ping worker critical error: {}", e);
            poc();
        }
    });
    svc_mark_ready(&client).await?;
    info!("{} started ({})", DESCRIPTION, initial.id());
    svc_block(&rpc).await;
    svc_mark_terminating(&client).await?;
    println!("ADS disconnected");
    Ok(())
}
