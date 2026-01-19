use super::GeneratorSource;
use crate::target::{Target, notify};
use eva_common::prelude::*;
use eva_sdk::prelude::*;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::task::JoinHandle;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Params {
    bind: String,
}

pub struct GenSource {}

#[async_trait::async_trait]
impl GeneratorSource for GenSource {
    async fn start(
        &self,
        name: &str,
        params: Value,
        _sampling: f64,
        targets: Arc<Vec<Target>>,
    ) -> EResult<JoinHandle<()>> {
        let params = Params::deserialize(params)?;
        let name = name.to_owned();
        let sock = UdpSocket::bind(&params.bind).await.map_err(Error::failed)?;
        let fut = tokio::spawn(async move {
            while !svc_is_terminating() {
                let mut buf = [0u8; 32];
                match sock.recv_from(&mut buf).await {
                    Ok(_) => {
                        let val = Value::F64(f64::from_le_bytes(buf[0..8].try_into().unwrap()));
                        notify(&name, &targets, val).await;
                    }
                    Err(e) => {
                        error!("{} UDP recv error: {}", name, e);
                    }
                }
            }
        });
        Ok(fut)
    }
}
