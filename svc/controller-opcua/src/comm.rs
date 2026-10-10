use crate::common::{OpcAuth, OpcUaConfig};
use eva_common::prelude::*;
use eva_common::services::Initial;
use eva_sdk::service::poc;
use log::{error, warn};
use opcua::client::prelude::*;
use opcua::sync::RwLock;
use std::path::Path;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

type OpcSession = Arc<RwLock<Session>>;

static SESSION: OnceLock<OpcSession> = OnceLock::new();

pub async fn write(
    node: NodeId,
    range: Option<&str>,
    value: Variant,
    timeout: Duration,
    retries: u8,
    t: Option<DateTime>,
) -> EResult<()> {
    if write_multi(vec![node], vec![range], vec![value], timeout, retries, t)
        .await?
        .is_empty()
    {
        Ok(())
    } else {
        Err(Error::io("failed to write OPC-UA node"))
    }
}

pub async fn write_multi(
    nodes: Vec<NodeId>,
    ranges: Vec<Option<&str>>,
    values: Vec<Variant>,
    timeout: Duration,
    retries: u8,
    t: Option<DateTime>,
) -> EResult<Vec<NodeId>> {
    if nodes.len() != ranges.len() {
        return Err(Error::invalid_params(
            "node number does not correspond with range number",
        ));
    }
    if nodes.len() != values.len() {
        return Err(Error::invalid_params(
            "node number does not correspond with value number",
        ));
    }
    let op = eva_common::op::Op::new(timeout);
    let dt = t.unwrap_or_else(DateTime::now);
    let session = SESSION
        .get()
        .ok_or_else(|| Error::not_ready("OPC session not ready"))?;
    let mut to_write: Vec<WriteValue> = nodes
        .into_iter()
        .zip(ranges)
        .zip(values)
        .map(|((node, range), value)| WriteValue {
            node_id: node,
            attribute_id: AttributeId::Value as u32,
            index_range: if let Some(r) = range {
                UAString::from(r)
            } else {
                UAString::null()
            },
            value: DataValue {
                value: Some(value),
                status: Some(StatusCode::Good),
                source_timestamp: Some(dt),
                ..DataValue::default()
            },
        })
        .collect();
    tokio::task::spawn_blocking(move || {
        let mut i = 0;
        loop {
            match session.read().write(&to_write) {
                Ok(mut res) => {
                    if res.len() != to_write.len() {
                        return Err(Error::io("invalid OPC server response"));
                    }
                    let keep: Vec<bool> = res.iter().map(|r| *r != StatusCode::Good).collect();
                    let mut iter = keep.iter();
                    to_write.retain(|_| *iter.next().unwrap());
                    if to_write.is_empty() {
                        return Ok(vec![]);
                    } else if i >= retries {
                        let mut iter = keep.iter();
                        res.retain(|_| *iter.next().unwrap());
                        for (w, r) in to_write.iter().zip(res) {
                            warn!("unable to write node {}: {}", w.node_id, r);
                        }
                        return Ok(to_write.into_iter().map(|v| v.node_id).collect());
                    }
                }
                Err(e) => {
                    if i >= retries {
                        return Err(Error::io(e));
                    }
                }
            }
            op.timeout()?;
            i += 1;
        }
    })
    .await?
}

pub async fn read_multi(
    nodes: Vec<NodeId>,
    ranges: Vec<Option<&str>>,
    timeout: Duration,
    retries: u8,
    batch_size: usize,
) -> EResult<Vec<DataValue>> {
    if nodes.len() != ranges.len() {
        return Err(Error::invalid_params(
            "node number does not correspond with range number",
        ));
    }
    if batch_size == 0 {
        return Err(Error::invalid_params(
            "read batch size must be greater than zero",
        ));
    }
    let op = eva_common::op::Op::new(timeout);
    let session = SESSION
        .get()
        .ok_or_else(|| Error::not_ready("OPC session not ready"))?;
    let to_read: Vec<ReadValueId> = nodes
        .into_iter()
        .zip(ranges)
        .map(|(node, range)| ReadValueId {
            node_id: node,
            attribute_id: AttributeId::Value as u32,
            index_range: if let Some(r) = range {
                UAString::from(r)
            } else {
                UAString::null()
            },
            data_encoding: QualifiedName::null(),
        })
        .collect();
    tokio::task::spawn_blocking(move || {
        read_batches(&to_read, batch_size, retries, &op, |batch| {
            session.read().read(batch, TimestampsToReturn::Neither, 0.0)
        })
    })
    .await?
}

fn read_batches<F>(
    to_read: &[ReadValueId],
    batch_size: usize,
    retries: u8,
    op: &eva_common::op::Op,
    mut read: F,
) -> EResult<Vec<DataValue>>
where
    F: FnMut(&[ReadValueId]) -> std::result::Result<Vec<DataValue>, StatusCode>,
{
    let mut result = Vec::with_capacity(to_read.len());
    for batch in to_read.chunks(batch_size) {
        let mut i = 0;
        loop {
            match read(batch) {
                Ok(v) => {
                    if v.len() != batch.len() {
                        return Err(Error::io("invalid OPC server response"));
                    }
                    result.extend(v);
                    break;
                }
                Err(e) => {
                    op.timeout()?;
                    i += 1;
                    if i > retries {
                        return Err(Error::io(e));
                    }
                }
            }
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_two_thousand_nodes_in_ordered_batches() {
        let requests: Vec<ReadValueId> = (0..2000)
            .map(|i| ReadValueId {
                node_id: format!("ns=2;i={i}").parse().unwrap(),
                attribute_id: AttributeId::Value as u32,
                index_range: UAString::null(),
                data_encoding: QualifiedName::null(),
            })
            .collect();
        let mut batch_sizes = Vec::new();
        let mut seen = Vec::new();
        let op = eva_common::op::Op::new(Duration::from_secs(5));
        let result = read_batches(&requests, 128, 0, &op, |batch| {
            batch_sizes.push(batch.len());
            seen.extend(batch.iter().map(|r| r.node_id.to_string()));
            Ok(batch.iter().map(|_| DataValue::default()).collect())
        })
        .unwrap();
        assert_eq!(result.len(), 2000);
        assert_eq!(batch_sizes.len(), 16);
        assert!(batch_sizes.iter().all(|&size| size <= 128));
        assert_eq!(
            seen.first().unwrap(),
            &requests.first().unwrap().node_id.to_string()
        );
        assert_eq!(
            seen.last().unwrap(),
            &requests.last().unwrap().node_id.to_string()
        );
    }

    #[test]
    fn rejects_short_batch_response() {
        let request = ReadValueId {
            node_id: NodeId::null(),
            attribute_id: AttributeId::Value as u32,
            index_range: UAString::null(),
            data_encoding: QualifiedName::null(),
        };
        let op = eva_common::op::Op::new(Duration::from_secs(5));
        assert!(read_batches(&[request], 128, 0, &op, |_| Ok(vec![])).is_err());
    }
}

#[allow(clippy::too_many_lines)]
pub async fn init_session(
    config: OpcUaConfig,
    initial: &Initial,
    timeout: Duration,
    ping_node: Option<NodeId>,
) -> EResult<()> {
    let pki_dir = if let Some(dir) = config.pki_dir {
        if dir.starts_with('/') {
            Path::new(&dir).to_owned()
        } else {
            let mut path = Path::new(initial.eva_dir()).to_owned();
            path.push(dir);
            path
        }
    } else {
        let mut path = Path::new(
            &initial
                .data_path()
                .ok_or_else(|| Error::failed("unable to get data path, running under nobody"))?,
        )
        .to_owned();
        path.push("pki");
        path
    };
    let token = if let Some(auth) = config.auth {
        match auth {
            OpcAuth::Credentials(x) => IdentityToken::UserName(x.user, x.password),
            OpcAuth::X509(x) => {
                macro_rules! format_path {
                    ($p: expr) => {
                        if $p.starts_with('/') {
                            Path::new(&$p).to_owned()
                        } else {
                            let mut p = pki_dir.clone();
                            p.push($p);
                            p
                        }
                    };
                }
                let cert_file = format_path!(x.cert_file);
                let key_file = format_path!(x.key_file);
                IdentityToken::X509(cert_file, key_file)
            }
        }
    } else {
        IdentityToken::Anonymous
    };
    let mut client = ClientBuilder::new()
        .application_name("eva4.controller-opcua")
        .application_uri("urn:")
        .product_uri("urn:")
        .ignore_clock_skew()
        .trust_server_certs(config.trust_server_certs)
        .create_sample_keypair(config.create_keys)
        .pki_dir(pki_dir)
        .session_retry_limit(0)
        .session_name(format!("{}.{}", initial.system_name(), initial.id()))
        .session_timeout(u32::try_from(timeout.as_millis())?)
        .client()
        .ok_or_else(|| Error::failed("unable to create OPC client"))?;
    let session: OpcSession = tokio::task::spawn_blocking(move || {
        client.connect_to_endpoint(
            (
                config.url.as_str(),
                config.security_policy.to_str(),
                config.message_security_mode,
                UserTokenPolicy::anonymous(),
            ),
            token,
        )
    })
    .await?
    .map_err(Error::io)?;
    let sess = session.clone();
    tokio::spawn(async move {
        let mut int = tokio::time::interval(Duration::from_secs(1));
        loop {
            int.tick().await;
            if !sess.read().is_connected() {
                error!("OPC session disconnected");
                poc();
                return;
            }
        }
    });
    if let Some(pn) = ping_node {
        tokio::spawn(async move {
            macro_rules! poc {
                ($msg: expr) => {
                    error!("OPC server ping error ({}): {}", pn, $msg);
                    poc();
                    return;
                };
            }
            let n = vec![pn.clone()];
            let r = vec![None];
            let timeout = *crate::TIMEOUT.get().unwrap();
            let mut int = tokio::time::interval(timeout / 2);
            loop {
                int.tick().await;
                match read_multi(n.clone(), r.clone(), timeout, 0, 1).await {
                    Ok(v) => {
                        if v.first().and_then(|v| v.value.as_ref()).is_none() {
                            poc!("no value");
                        }
                    }
                    Err(e) => {
                        poc!(e);
                    }
                }
            }
        });
    }
    SESSION
        .set(session)
        .map_err(|_| Error::core("Unable to set SESSION"))?;
    Ok(())
}
