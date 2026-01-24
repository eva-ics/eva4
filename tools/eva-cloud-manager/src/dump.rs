use bzip2::read::BzEncoder;
use chrono::Local;
use clap::Parser;
use ecm::EVA_DIR;
use eva_client::EvaClient;
use eva_common::common_payloads::ParamsId;
use eva_common::prelude::*;
use log::{info, warn};
use openssl::rsa::{Padding, Rsa};
use serde::Serialize;
use std::io::Read;
use std::time::Duration;
use tokio::io::AsyncWriteExt;

const DUMP_OP_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Parser, Clone, Debug)]
pub struct Options {
    #[clap(short = 'C', long = "connection-path")]
    connection_path: Option<String>,
    #[clap(short = 's', help = "create service request")]
    service_request: bool,
}

async fn call(path: &str, target: &str, method: &str, params: Option<Value>) -> Value {
    match EvaClient::connect(
        path,
        ecm::BUS_CLIENT_NAME,
        eva_client::Config::new().timeout(DUMP_OP_TIMEOUT),
    )
    .await
    {
        Ok(client) => match client.rpc_call(target, method, params).await {
            Ok(v) => v,
            Err(e) => Value::String(e.to_string()),
        },
        Err(e) => Value::String(e.to_string()),
    }
}

#[derive(Serialize)]
struct Dump {
    broker: Value,
    core: Value,
    configs: Value,
    items: Value,
    svc_configs: Value,
    svcs: Value,
    nodes: Value,
    actions: Value,
    log: Value,
}

#[derive(Serialize)]
struct ParamsReg<'a> {
    key: &'a str,
}

#[derive(Serialize)]
struct ParamsLogGet {
    level: u8,
    time: u32,
    limit: u32,
}

impl Default for ParamsLogGet {
    fn default() -> Self {
        Self {
            level: 20,
            time: 3600,
            limit: 10000,
        }
    }
}

#[derive(Serialize)]
struct ParamsActionList {
    time: u32,
    limit: u32,
}

impl Default for ParamsActionList {
    fn default() -> Self {
        Self {
            time: 3600,
            limit: 10000,
        }
    }
}

#[allow(clippy::too_many_lines)]
pub async fn dump(opts: Options) -> EResult<()> {
    let rsa = if opts.service_request {
        let mut pub_key_path = EVA_DIR.clone();
        pub_key_path.push("share/eva.key");
        let key = tokio::fs::read(pub_key_path).await?;
        Some(Rsa::public_key_from_der_pkcs1(&key).map_err(Error::failed)?)
    } else {
        None
    };
    let now = Local::now();
    let dump_name = now.format("%Y%m%d%H%M%S%3f");
    let sfx = if opts.service_request {
        ".sr"
    } else {
        ".dump.bz2"
    };
    let fname = eva_common::tools::format_path(
        &eva_common::tools::get_eva_dir(),
        Some(&format!("var/{dump_name}{sfx}")),
        None,
    );
    let connection_path = opts
        .connection_path
        .unwrap_or_else(|| crate::DEFAULT_CONNECTION_PATH.to_string());
    info!("connecting to {}", connection_path);
    info!("creating a dump file");
    ttycarousel::tokio1::spawn0("collecting bus info").await;
    let broker_client_list: Value = call(&connection_path, ".broker", "client.list", None).await;
    ttycarousel::tokio1::stop_clear().await;
    ttycarousel::tokio1::spawn0("collecting core info").await;
    let core_info: Value = call(&connection_path, "eva.core", "test", None).await;
    ttycarousel::tokio1::stop_clear().await;
    ttycarousel::tokio1::spawn0("collecting configs").await;
    let config_keys: Value = call(
        &connection_path,
        "eva.registry",
        "key_get_recursive",
        Some(to_value(ParamsReg { key: "eva/config" })?),
    )
    .await;
    ttycarousel::tokio1::stop_clear().await;
    ttycarousel::tokio1::spawn0("collecting items").await;
    let item_list: Value = call(
        &connection_path,
        "eva.core",
        "item.list",
        Some(to_value(ParamsId { i: "#" })?),
    )
    .await;
    ttycarousel::tokio1::stop_clear().await;
    ttycarousel::tokio1::spawn0("collecting service info").await;
    let svc_list: Value = call(&connection_path, "eva.core", "svc.list", None).await;
    ttycarousel::tokio1::stop_clear().await;
    ttycarousel::tokio1::spawn0("collecting service configs").await;
    let svc_config_keys: Value = call(
        &connection_path,
        "eva.registry",
        "key_get_recursive",
        Some(to_value(ParamsReg { key: "eva/svc" })?),
    )
    .await;
    ttycarousel::tokio1::stop_clear().await;
    ttycarousel::tokio1::spawn0("collecting node info").await;
    let node_list: Value = call(&connection_path, "eva.core", "node.list", None).await;
    let log: Value = call(
        &connection_path,
        "eva.core",
        "log.get",
        Some(to_value(ParamsLogGet::default())?),
    )
    .await;
    ttycarousel::tokio1::stop_clear().await;
    ttycarousel::tokio1::spawn0("collecting action info").await;
    let actions: Value = call(
        &connection_path,
        "eva.core",
        "action.list",
        Some(to_value(ParamsActionList::default())?),
    )
    .await;
    ttycarousel::tokio1::stop_clear().await;
    let dump = Dump {
        broker: broker_client_list,
        core: core_info,
        configs: config_keys,
        svc_configs: svc_config_keys,
        items: item_list,
        svcs: svc_list,
        nodes: node_list,
        actions,
        log,
    };
    ttycarousel::tokio1::spawn0("Serializing").await;
    let dump_json = serde_json::to_vec(&dump)?;
    ttycarousel::tokio1::stop_clear().await;
    ttycarousel::tokio1::spawn0("Compressing").await;
    let res_compr: EResult<Vec<u8>> = tokio::task::spawn_blocking(move || {
        let cursor = std::io::Cursor::new(dump_json);
        let mut compressor = BzEncoder::new(cursor, bzip2::Compression::default());
        let mut buf = Vec::new();
        compressor.read_to_end(&mut buf)?;
        Ok(buf)
    })
    .await
    .map_err(Error::failed)?;
    let compr = res_compr?;
    ttycarousel::tokio1::stop_clear().await;
    ttycarousel::tokio1::spawn0("Writing").await;
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(false)
        .truncate(true)
        .write(true)
        .open(&fname)
        .await?;
    if let Some(rsa) = rsa {
        let mut writer = tokio::io::BufWriter::new(file);
        let mut cursor = std::io::Cursor::new(compr);
        let buf_size: usize = rsa.size().try_into().map_err(Error::failed)?;
        loop {
            let mut buf = vec![0_u8; buf_size - 11];
            let mut res: Vec<u8> = vec![0; buf_size];
            let r = cursor.read(&mut buf)?;
            if r == 0 {
                break;
            }
            let len = rsa
                .public_encrypt(&buf[..r], &mut res, Padding::PKCS1)
                .map_err(Error::failed)?;
            writer.write_all(&res[..len]).await?;
        }
        writer.flush().await?;
    } else {
        file.write_all(&compr).await?;
    }
    ttycarousel::tokio1::stop_clear().await;
    warn!("dump created, file: {}", fname);
    Ok(())
}
