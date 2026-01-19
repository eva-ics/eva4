use eva_common::acl::Acl;
use eva_common::prelude::*;
use eva_sdk::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::LazyLock;
use std::sync::Mutex;

pub static ENC_OPTS: LazyLock<Mutex<HashMap<String, psrpc::options::Options>>> =
    LazyLock::new(<_>::default);
pub static KEYS: LazyLock<Mutex<HashMap<String, String>>> = LazyLock::new(<_>::default);
pub static ACLS: LazyLock<Mutex<HashMap<String, Arc<Acl>>>> = LazyLock::new(<_>::default);

pub async fn get_acl(rpc: &RpcClient, key_id: &str) -> EResult<Arc<Acl>> {
    #[derive(Serialize)]
    struct AuthPayload<'a> {
        key: &'a str,
        timeout: f64,
    }
    if let Some(acl) = ACLS.lock().unwrap().get(key_id) {
        trace!("using cached ACL for {}", key_id);
        return Ok(acl.clone());
    }
    let key_value = get_key(rpc, key_id).await?;
    let timeout = crate::TIMEOUT.get().unwrap();
    let payload = pack(&AuthPayload {
        key: &key_value,
        timeout: timeout.as_secs_f64(),
    })?;
    let key_svc = crate::KEY_SVC.get().unwrap();
    trace!("fetching ACL from {} for key {}", key_svc, key_id);
    let res = safe_rpc_call(
        rpc,
        key_svc,
        "auth.key",
        payload.as_slice().into(),
        QoS::Processed,
        *timeout,
    )
    .await?;
    let acl = Arc::new(unpack::<Acl>(res.payload())?);
    ACLS.lock().unwrap().insert(key_id.to_owned(), acl.clone());
    Ok(acl)
}

pub async fn get_enc_opts(rpc: &RpcClient, key_id: &str) -> EResult<psrpc::options::Options> {
    if let Some(opts) = ENC_OPTS.lock().unwrap().get(key_id) {
        trace!("using cached encryption options for {}", key_id);
        return Ok(opts.clone());
    }
    let key_value = get_key(rpc, key_id).await?;
    let enc_key = psrpc::options::EncryptionKey::new(key_id, &key_value);
    let opts =
        psrpc::options::Options::new().encryption(psrpc::options::Encryption::Aes256Gcm, &enc_key);
    ENC_OPTS
        .lock()
        .unwrap()
        .insert(key_id.to_owned(), opts.clone());
    Ok(opts)
}

pub async fn get_key(rpc: &RpcClient, key_id: &str) -> EResult<String> {
    #[derive(Serialize)]
    struct Params<'a> {
        i: &'a str,
    }
    #[derive(Deserialize)]
    struct Payload {
        key: String,
    }
    if let Some(key) = KEYS.lock().unwrap().get(key_id) {
        trace!("using cached key data for {}", key_id);
        return Ok(key.clone());
    }
    let key_svc = crate::KEY_SVC
        .get()
        .ok_or_else(|| Error::unsupported("key service is not set"))?;
    trace!("fetching key data from {} for key {}", key_svc, key_id);
    let data = safe_rpc_call(
        rpc,
        key_svc,
        "key.get",
        pack(&Params { i: key_id })?.into(),
        QoS::Processed,
        *crate::TIMEOUT.get().unwrap(),
    )
    .await?;
    let result: Payload = unpack(data.payload())?;
    KEYS.lock()
        .unwrap()
        .insert(key_id.to_owned(), result.key.clone());
    Ok(result.key)
}
