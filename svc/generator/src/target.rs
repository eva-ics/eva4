use crate::is_verbose;
use eva_common::events::{
    RAW_STATE_TOPIC, REMOTE_ARCHIVE_STATE_TOPIC, RawStateEvent, RemoteStateEvent,
};
use eva_common::payload::pack;
use eva_common::prelude::*;
use eva_sdk::prelude::*;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

const DEFAULT_METHOD: &str = "var.set";

pub enum Target {
    Oid(OID),
    Service(Service),
}

impl Target {
    async fn notify(&self, value: &Value) -> EResult<()> {
        match self {
            Target::Oid(oid) => {
                let topic = format!("{}{}", RAW_STATE_TOPIC, oid.as_path());
                let payload = pack(&RawStateEvent::new(1, value))?;
                crate::RPC
                    .get()
                    .unwrap()
                    .client()
                    .lock()
                    .await
                    .publish(&topic, payload.into(), QoS::No)
                    .await?;
            }
            Target::Service(svc) => {
                let mut payload: BTreeMap<&str, &Value> =
                    svc.params.iter().map(|(k, v)| (k.as_str(), v)).collect();
                payload.insert("value", value);
                safe_rpc_call(
                    crate::RPC.get().unwrap(),
                    &svc.id,
                    &svc.method,
                    pack(&payload)?.into(),
                    QoS::RealtimeProcessed,
                    *crate::TIMEOUT.get().unwrap(),
                )
                .await?;
            }
        };
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
pub struct Service {
    id: String,
    method: String,
    params: BTreeMap<String, Value>,
}

impl FromStr for Target {
    type Err = Error;
    fn from_str(s: &str) -> EResult<Self> {
        if let Ok(oid) = s.parse::<OID>() {
            Ok(Target::Oid(oid))
        } else {
            let mut sp = s.splitn(2, "::");
            let id = sp.next().unwrap().to_owned();
            let m = sp.next().unwrap_or(DEFAULT_METHOD);
            let mut sp_m = m.splitn(2, '@');
            let method = sp_m.next().unwrap().to_owned();
            let mut params = BTreeMap::new();
            if let Some(sp_p) = sp_m.next().map(|v| v.split(',')) {
                for p in sp_p {
                    let mut sp_param = p.splitn(2, '=');
                    let name = sp_param.next().unwrap();
                    if let Some(val) = sp_param.next() {
                        params.insert(name.to_owned(), Value::String(val.to_owned()));
                    } else {
                        params.insert("i".to_owned(), Value::String(name.to_owned()));
                    }
                }
            }
            Ok(Target::Service(Service { id, method, params }))
        }
    }
}

impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Target::Oid(oid) => write!(f, "{}", oid),
            Target::Service(s) => {
                write!(f, "{}::{}", s.id, s.method)?;
                if !s.params.is_empty() {
                    write!(f, "@")?;
                    for (n, (k, v)) in s.params.iter().enumerate() {
                        if n > 0 {
                            write!(f, ",")?;
                        }
                        if k == "i" {
                            write!(f, "{}", v)?;
                        } else {
                            write!(f, "{}={}", k, v)?;
                        }
                    }
                }
                Ok(())
            }
        }
    }
}

impl Serialize for Target {
    #[inline]
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Target {
    #[inline]
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s: String = Deserialize::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

pub async fn notify(source_name: &str, targets: &[Target], value: Value) {
    for target in targets {
        if is_verbose() {
            info!(
                "notifying {} target {} value {}",
                source_name, target, value
            );
        } else {
            debug!(
                "notifying {} target {} value {}",
                source_name, target, value
            );
        }
        if let Err(e) = target.notify(&value).await {
            error!("Unable to notify {}: {}", target, e);
        }
    }
}

pub async fn notify_archive(targets: &[OID], t: f64, value: Value) -> EResult<()> {
    let payload = pack(&RemoteStateEvent {
        status: 1,
        value,
        act: None,
        ieid: IEID::new(1, 0),
        t,
        node: crate::SYSTEM_NAME.get().unwrap().clone(),
        connected: true,
    })?;
    let payload_c = busrt::borrow::Cow::Borrowed(&payload);
    let rpc = crate::RPC.get().unwrap();
    let client_g = rpc.client();
    let mut client = client_g.lock().await;
    for target in targets {
        let topic = format!("{}{}", REMOTE_ARCHIVE_STATE_TOPIC, target.as_path());
        client.publish(&topic, payload_c.clone(), QoS::No).await?;
    }
    Ok(())
}
