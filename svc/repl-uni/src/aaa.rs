use eva_sdk::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

pub static KEYS: LazyLock<Mutex<HashMap<String, String>>> = LazyLock::new(<_>::default);
pub static ENC_OPTS: LazyLock<Mutex<HashMap<String, psrpc::options::Options>>> =
    LazyLock::new(<_>::default);

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
    let key_svc = crate::KEY_SVC.get().unwrap();
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
