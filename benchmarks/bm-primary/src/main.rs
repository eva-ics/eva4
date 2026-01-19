#[macro_use]
extern crate bma_benchmark;

use busrt::QoS;
use busrt::ipc::{Client, Config};
use busrt::rpc::{Rpc, RpcClient};
use clap::Parser;
use eva_common::common_payloads::ParamsId;
use eva_common::events::{RAW_STATE_TOPIC, RawStateEvent};
use eva_common::payload::{pack, unpack};
use eva_common::prelude::*;
use log::{error, info};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;
use sysinfo::{Pid, ProcessExt, System, SystemExt};

const SENSOR_PFX: &str = "sensor:bm_primary/s";
const ITEMS_PER_BATCH: u32 = 1000;

#[cfg(debug_assertions)]
const ITERS: u32 = 1000;

#[cfg(not(debug_assertions))]
const ITERS: u32 = 1_000_000;

#[derive(Serialize)]
struct ParamsItemsDeploy {
    items: Vec<EmptySensor>,
}

#[derive(Serialize)]
struct EmptySensor {
    oid: String,
}

//#[derive(Deserialize)]
//struct BrokerClient {
//name: String,
//queue: usize,
//}

#[derive(Deserialize)]
struct ItemState {
    //oid: OID,
    value: Value,
}

#[derive(Serialize)]
struct ParamsItemsUndeploy {
    items: Vec<String>,
}

#[cfg(not(debug_assertions))]
mod prepare {
    use busrt::ipc::{Client, Config};
    use busrt::rpc::RpcClient;
    use busrt::{QoS, rpc::Rpc};
    use eva_common::payload::pack;
    use eva_common::prelude::*;
    use serde::Serialize;
    use std::collections::BTreeMap;

    const CONFIG_CORE: &str = r#"
{
    "instant_save": false,
    "inventory_db": "sqlite:///opt/eva4/var/inventory.db",
    "keep_action_history": 86400,
    "suicide_timeout": 15,
    "timeout": 5
}
    "#;
    const CONFIG_BUS: &str = r#"
{
    "buf_size": 8192,
    "buf_ttl": 10,
    "queue_size": 256000,
    "sockets": [
        "var/bus.ipc"
    ]
}
    "#;
    const CONFIG_LOGS: &str = r#"
[
    {
        "level": "warn",
        "output": "console"
    },
    {
        "level": "warn",
        "output": "bus"
    },
    {
        "keep": 86400,
        "level": "warn",
        "output": "memory"
    },
    {
        "format": "regular",
        "level": "warn",
        "output": "file",
        "path": "log/eva.log"
    },
    {
        "level": "warn",
        "output": "syslog"
    }
]
"#;

    #[derive(Serialize)]
    struct RegKeyPayload {
        key: String,
        value: Value,
    }
    async fn set_config_key(rpc: &RpcClient, key: &str, value: Value) -> EResult<()> {
        rpc.call(
            "eva.registry",
            "key_set",
            pack(&RegKeyPayload {
                key: format!("eva/config/{}", key),
                value,
            })?
            .into(),
            QoS::No,
        )
        .await?;
        Ok(())
    }
    pub async fn prepare(name: &str, workers: u32) -> EResult<()> {
        let client = Client::connect(&Config::new("/opt/eva4/var/bus.ipc", name)).await?;
        let rpc = RpcClient::new0(client);
        let mut value: BTreeMap<String, Value> =
            serde_json::from_str(CONFIG_CORE).map_err(Error::failed)?;
        value.insert("workers".into(), Value::U32(workers));
        set_config_key(&rpc, "core", to_value(value)?).await?;
        let mut value = serde_json::from_str(CONFIG_BUS).map_err(Error::failed)?;
        if let Value::Map(ref mut v) = value {
            if let Value::Seq(ref mut a) = v.get_mut(&Value::String("sockets".to_owned())).unwrap()
            {
                for w in 0..workers {
                    a.push(Value::String(format!("var/bus{}.ipc", w)));
                }
            } else {
                panic!();
            }
        } else {
            panic!();
        }
        set_config_key(&rpc, "bus", value).await?;
        let value = serde_json::from_str(CONFIG_LOGS).map_err(Error::failed)?;
        set_config_key(&rpc, "logs", value).await?;
        Ok(())
    }
}

async fn cleanup(rpc: &RpcClient) -> EResult<()> {
    info!("cleaning up");
    for i in 0..ITERS / ITEMS_PER_BATCH {
        let mut sensors = Vec::new();
        for j in 0..ITEMS_PER_BATCH {
            sensors.push(format!("{}_{}_{}", SENSOR_PFX, i, j));
        }
        rpc.call(
            "eva.core",
            "item.undeploy",
            pack(&ParamsItemsUndeploy { items: sensors })?.into(),
            QoS::No,
        )
        .await?;
    }
    Ok(())
}

async fn create(rpc: &RpcClient) -> EResult<()> {
    info!("creating");
    for i in 0..ITERS / ITEMS_PER_BATCH {
        let mut sensors = Vec::new();
        for j in 0..ITEMS_PER_BATCH {
            sensors.push(EmptySensor {
                oid: format!("{}_{}_{}", SENSOR_PFX, i, j),
            });
        }
        rpc.call(
            "eva.core",
            "item.deploy",
            pack(&ParamsItemsDeploy { items: sensors })?.into(),
            QoS::No,
        )
        .await?;
    }
    Ok(())
}

async fn publish_raw(rpc: &RpcClient, oid: &OID) -> EResult<()> {
    let topic = format!("{}{}", RAW_STATE_TOPIC, oid.as_path());
    let cl = rpc.client();
    let mut client = cl.lock().await;
    let packed0 = pack(&RawStateEvent::new(1, &Value::U8(0)))?;
    let packed1 = pack(&RawStateEvent::new(1, &Value::U8(1)))?;
    let n = ITERS * 10;
    for i in 1..=n {
        let payload = if i == n {
            pack(&RawStateEvent::new(1, &Value::U8(77)))?.into()
        } else {
            busrt::borrow::Cow::Borrowed(if i % 2 == 0 { &packed0 } else { &packed1 })
        };
        client.publish(&topic, payload, QoS::No).await?;
    }
    Ok(())
}

fn get_mem_kb(sys: &mut System, pid: Pid) -> u64 {
    sys.refresh_process(pid);
    let mem = sys.process(pid).unwrap().memory();
    mem
}

#[tokio::main]
async fn main() -> EResult<()> {
    eva_common::console_logger::configure_env_logger(false);
    if let Err(e) = benchmark().await {
        error!("{}", e);
        Err(e)
    } else {
        Ok(())
    }
}

fn sep() {
    println!("------------------------------------------------------------");
}

#[derive(Parser)]
struct Args {
    #[clap(short = 'w', long = "workers", default_value = "4")]
    workers: u32,
    #[clap(long = "clients", default_value = "1")]
    clients: usize,
    #[clap(long = "dedicated-sockets")]
    dedicated_sockets: bool,
}

#[allow(clippy::cast_precision_loss)]
async fn benchmark() -> EResult<()> {
    let name = format!("bm_primary.{}", std::process::id());
    let args = Args::parse();
    #[cfg(not(debug_assertions))]
    {
        prepare::prepare(&name, args.workers).await?;
        info!("workers: {}", args.workers);
    }
    info!("event sender clients: {}", args.clients);
    info!("iters: {}", ITERS);
    info!("restarting the server");
    let res = bmart::process::command(
        "/opt/eva4/sbin/eva-control",
        &["restart"],
        Duration::from_secs(30),
        bmart::process::Options::default(),
    )
    .await?;
    assert!(res.ok());
    tokio::time::sleep(Duration::from_secs(5)).await;
    let core_pid = Pid::from(
        tokio::fs::read_to_string("/opt/eva4/var/eva.pid")
            .await?
            .parse::<i32>()?,
    );
    let client = Client::connect(&Config::new("/opt/eva4/var/bus.ipc", &name)).await?;
    let rpc = Arc::new(RpcClient::new0(client));
    cleanup(&rpc).await?;
    let mut sys = System::new();
    let mem_before = get_mem_kb(&mut sys, core_pid);
    info!("memory consumed: {} KB", mem_before);
    benchmark_start!();
    create(&rpc).await?;
    info!("deployment speed:");
    benchmark_print!(ITERS);
    sep();
    let mem = get_mem_kb(&mut sys, core_pid);
    info!("memory consumed with sensors: {} KB", mem);
    let single_sensor_usage = (mem as f64 - mem_before as f64) / f64::from(ITERS);
    let million_sensor_usage = (mem as f64 - mem_before as f64) * (1_000_000.0 / f64::from(ITERS));
    info!(
        "single sensor memory usage: {} MB",
        single_sensor_usage / 1024.0
    );
    info!(
        "million sensor memory usage: {} MB",
        million_sensor_usage / 1024.0
    );
    info!("testing raw events");
    benchmark_start!();
    let mut futs = Vec::new();
    for client_id in 0..args.clients {
        let oid = format!("sensor:bm_primary/s_0_{}", client_id).parse::<OID>()?;
        let rpc_c = if args.dedicated_sockets {
            let name = format!("bm_primary.{}.{}", std::process::id(), client_id);
            let client = Client::connect(
                &Config::new(&format!("/opt/eva4/var/bus{}.ipc", client_id), &name)
                    .timeout(Duration::from_secs(30)),
            )
            .await?;
            Arc::new(RpcClient::new0(client))
        } else {
            rpc.clone()
        };
        let fut = tokio::spawn(async move {
            publish_raw(&rpc_c, &oid).await.unwrap();
            loop {
                let res = rpc_c
                    .call(
                        "eva.core",
                        "item.state",
                        pack(&ParamsId { i: oid.as_str() }).unwrap().into(),
                        QoS::No,
                    )
                    .await
                    .unwrap();
                let st: Vec<ItemState> = unpack(res.payload()).unwrap();
                if let Ok(v) = u32::try_from(st[0].value.clone()) {
                    if v == 77 {
                        break;
                    }
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        });
        futs.push(fut);
    }
    for fut in futs {
        fut.await.unwrap();
    }
    benchmark_print!(ITERS * 10 * u32::try_from(args.clients).unwrap());
    sep();
    cleanup(&rpc).await?;
    Ok(())
}
