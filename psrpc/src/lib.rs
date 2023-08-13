use async_trait::async_trait;
use eva_common::err_logger;
use eva_common::payload::{pack, unpack};
use eva_common::prelude::*;
use log::{debug, error, info, trace, warn};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use uuid::Uuid;

err_logger!();

pub const RPC_REQUEST: u8 = 0x01;
pub const RPC_REPLY: u8 = 0x11;
pub const RPC_ERROR: u8 = 0x12;

pub const PROTO_VERSION: u8 = 1;

pub const COMPRESSION_NO: u8 = 0;
pub const COMPRESSION_BZIP2: u8 = 1;

pub const ENCRYPTION_NO: u8 = 0;
pub const ENCRYPTION_AES_128_GCM: u8 = 1;
pub const ENCRYPTION_AES_256_GCM: u8 = 2;

pub const DEFAULT_NODE_RPC_TOPIC: &str = "NODE/RPC/";
pub const TEST_TOPIC: &str = "TEST/";

pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);
pub const DEFAULT_PING_INTERVAL: Duration = Duration::from_secs(5);
pub const DEFAULT_QUEUE_SIZE: usize = 1024;
pub const DEFULT_QOS: i32 = 1;

pub mod options;
pub mod pubsub;
pub mod tools;

pub type PsMessage = Box<dyn pubsub::Message + Send + Sync>;

#[async_trait]
pub trait RpcHandlers {
    async fn handle_message(&self, message: PsMessage); // always blocking
    async fn handle_call(
        &self,
        key_id: Option<&str>,
        method: &str,
        params: Option<Value>,
    ) -> EResult<Value>; // non-blocking
    async fn get_key(&self, key_id: &str) -> EResult<String>; // get encryption key
}

pub struct DummyHandlers {}

#[async_trait]
impl RpcHandlers for DummyHandlers {
    async fn handle_message(&self, _message: PsMessage) {}
    async fn handle_call(
        &self,
        _key_id: Option<&str>,
        _method: &str,
        _params: Option<Value>,
    ) -> EResult<Value> {
        Err(Error::not_implemented("RPC server not implemented"))
    }
    async fn get_key(&self, _key_id: &str) -> EResult<String> {
        Err(Error::not_implemented("RPC server not implemented"))
    }
}

type CallMap = Arc<Mutex<BTreeMap<Uuid, oneshot::Sender<Reply>>>>;

#[derive(Debug, Copy, Clone)]
enum ReplyKind {
    Success,
    Error,
}

struct Reply {
    kind: ReplyKind,
    message: Box<dyn pubsub::Message + Send + Sync>,
}

#[derive(Default)]
struct Tester {
    data: Mutex<Option<(String, oneshot::Sender<()>)>>,
}

impl Tester {
    fn fire(&self, topic: &str, payload: &[u8]) -> bool {
        if payload != [b'+'] {
            return false;
        }
        let mut data = self.data.lock().unwrap();
        if let Some(d) = data.as_ref() {
            if d.0 == topic {
                let _r = data.take().unwrap().1.send(());
                true
            } else {
                false
            }
        } else {
            false
        }
    }
    fn arm(&self, topic: &str) -> oneshot::Receiver<()> {
        let (tx, rx) = oneshot::channel();
        self.data.lock().unwrap().replace((topic.to_owned(), tx));
        rx
    }
}

pub struct RpcClient {
    name: String,
    timeout: Duration,
    client: Arc<dyn pubsub::Client + Send + Sync>,
    processor_fut: Arc<Mutex<JoinHandle<()>>>,
    tester_fut: JoinHandle<()>,
    calls: CallMap,
    qos: i32,
    node_rpc_topic: Arc<String>,
}

async fn handle_call<H>(
    handlers: Arc<H>,
    processor_client: Arc<dyn pubsub::Client + Send + Sync>,
    message: Box<dyn pubsub::Message + Send + Sync>,
    node_rpc_topic: Arc<String>,
    qos: i32,
) -> EResult<()>
where
    H: RpcHandlers + Send + Sync + 'static,
{
    let flags = message.data()[2];
    let (encryption, compression) = options::parse_flags(flags)?;
    let mut opts = options::Options::new().compression(compression);
    let (sender, payload_pos, key_id) = {
        let mut sp = message.data()[5..].splitn(3, |v| *v == 0);
        let sender = std::str::from_utf8(sp.next().unwrap())?.to_owned();
        let key_id_payload = sp
            .next()
            .ok_or_else(|| Error::invalid_data("key id missing"))?;
        let _payload = sp
            .next()
            .ok_or_else(|| Error::invalid_data("payload missing"))?;
        let key_id = if encryption == options::Encryption::No {
            None
        } else {
            let key_id = std::str::from_utf8(key_id_payload)?.to_owned();
            let key_value = handlers.get_key(&key_id).await?;
            let key = options::EncryptionKey::new(&key_id, &key_value);
            opts = opts.encryption(encryption, &key);
            Some(key_id)
        };
        let payload_pos = sender.len() + key_id_payload.len() + 7;
        (sender, payload_pos, key_id)
    };
    let payload = opts.unpack_payload(message, payload_pos).await?;
    if payload.len() < 17 {
        return Err(Error::invalid_data("RPC payload too short"));
    }
    let call_id = Uuid::from_u128(u128::from_le_bytes(payload[..16].try_into()?));
    let mut sp = payload[16..].splitn(2, |v| *v == 0);
    let method = std::str::from_utf8(sp.next().unwrap())?;
    let params_payload = sp
        .next()
        .ok_or_else(|| Error::invalid_data("params missing"))?;
    let params: Option<Value> = if params_payload.is_empty() {
        None
    } else {
        Some(unpack(params_payload).map_err(Error::invalid_data)?)
    };
    let mut reply = vec![PROTO_VERSION];
    trace!("handling RPC call from {}: {} {}", sender, call_id, method);
    match handlers
        .handle_call(key_id.as_deref(), method, params)
        .await
    {
        Ok(v) => {
            reply.push(RPC_REPLY);
            reply.push(0x00);
            reply.push(0x00);
            reply.extend(call_id.as_u128().to_le_bytes());
            reply.extend(
                opts.pack_payload(pack(&v).map_err(Error::invalid_data)?)
                    .await?,
            );
        }
        Err(e) => {
            if e.kind() == ErrorKind::BusClientNotRegistered {
                info!("RPC error call from {}, method {}: {}", sender, method, e);
            } else {
                error!("RPC error call from {}, method {}: {}", sender, method, e);
            }
            let mut buf = Vec::new();
            buf.extend((e.kind() as i16).to_le_bytes());
            if let Some(msg) = e.message() {
                buf.extend(msg.as_bytes());
            }
            reply.push(RPC_ERROR);
            reply.push(0x00);
            reply.push(0x00);
            reply.extend(call_id.as_u128().to_le_bytes());
            reply.extend(opts.pack_payload(buf).await?);
        }
    }
    processor_client
        .publish(&format!("{}{}", node_rpc_topic, sender), reply, qos)
        .await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn processor<H>(
    rx: async_channel::Receiver<Box<dyn pubsub::Message + Send + Sync>>,
    processor_client: Arc<dyn pubsub::Client + Send + Sync>,
    calls: CallMap,
    my_rpc_topic: String,
    handlers: Arc<H>,
    tester: Arc<Tester>,
    node_rpc_topic: Arc<String>,
    qos: i32,
) where
    H: RpcHandlers + Send + Sync + 'static,
{
    while let Ok(message) = rx.recv().await {
        let topic = message.topic();
        if topic == my_rpc_topic {
            let payload = message.data();
            if payload.len() < 5 {
                warn!("invalid payload received: too small");
                continue;
            }
            if payload[0] != PROTO_VERSION {
                warn!("invalid protocol version");
                continue;
            }
            let op = payload[1];
            match op {
                RPC_REQUEST => {
                    let h = handlers.clone();
                    let c = processor_client.clone();
                    let node_rpc_topic_c = node_rpc_topic.clone();
                    // TODO move to task pool
                    tokio::spawn(async move {
                        if let Err(e) = handle_call(h, c, message, node_rpc_topic_c, qos).await {
                            if e.kind() == ErrorKind::BusClientNotRegistered {
                                info!("{}", e);
                            } else {
                                error!("{}", e);
                            }
                        }
                    });
                }
                RPC_REPLY | RPC_ERROR => {
                    if payload.len() < 22 {
                        warn!("invalid reply payload received: too small");
                        continue;
                    }
                    let call_id =
                        Uuid::from_u128(u128::from_le_bytes(payload[4..20].try_into().unwrap()));
                    trace!("RPC reply {}", call_id);
                    if let Some(tx) = { calls.lock().unwrap().remove(&call_id) } {
                        let _r = tx.send(Reply {
                            kind: if op == RPC_REPLY {
                                ReplyKind::Success
                            } else {
                                ReplyKind::Error
                            },
                            message,
                        });
                    } else {
                        warn!("orphaned RPC response: {}", call_id);
                    }
                }
                v => warn!("invalid payload type: {}", v),
            }
        } else if !tester.fire(topic, message.data()) {
            trace!("PubSub message topic {}", topic);
            handlers.handle_message(message).await;
        }
    }
}

#[derive(Debug, Clone)]
pub struct Config<'a> {
    name: &'a str,
    timeout: Duration,
    ping_interval: Duration,
    queue_size: usize,
    qos: i32,
    test_failed: Option<triggered::Trigger>,
    node_rpc_topic: &'a str,
}

impl<'a> Config<'a> {
    pub fn new(name: &'a str) -> Self {
        Self {
            name,
            timeout: DEFAULT_TIMEOUT,
            ping_interval: DEFAULT_PING_INTERVAL,
            queue_size: DEFAULT_QUEUE_SIZE,
            qos: DEFULT_QOS,
            test_failed: None,
            node_rpc_topic: DEFAULT_NODE_RPC_TOPIC,
        }
    }
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
    pub fn ping_interval(mut self, ping_interval: Duration) -> Self {
        self.ping_interval = ping_interval;
        self
    }
    pub fn queue_size(mut self, queue_size: usize) -> Self {
        self.queue_size = queue_size;
        self
    }
    pub fn qos(mut self, qos: i32) -> Self {
        self.qos = qos;
        self
    }
    pub fn node_rpc_topic(mut self, topic: &'a str) -> Self {
        self.node_rpc_topic = topic;
        self
    }
    pub fn arm_failure_trigger(&mut self) -> triggered::Listener {
        let (trigger, listener) = triggered::trigger();
        self.test_failed.replace(trigger);
        listener
    }
}

async fn pstest(
    tester: Arc<Tester>,
    client: Arc<dyn pubsub::Client + Send + Sync + 'static>,
    timeout: Duration,
) -> EResult<()> {
    let op = eva_common::op::Op::new(timeout);
    let topic = format!("{}{}", TEST_TOPIC, Uuid::new_v4());
    tokio::time::timeout(op.timeout()?, client.subscribe(&topic, 1)).await??;
    let rx = tester.arm(&topic);
    tokio::time::timeout(op.timeout()?, client.publish(&topic, vec![b'+'], 1)).await??;
    tokio::time::timeout(op.timeout()?, rx).await??;
    tokio::time::timeout(op.timeout()?, client.unsubscribe(&topic)).await??;
    Ok(())
}

impl RpcClient {
    /// # Panics
    ///
    /// The tester future will panic if the processor future holder mutex is poisoned
    pub async fn create<H, C>(mut client: C, handlers: H, config: Config<'_>) -> EResult<Self>
    where
        H: RpcHandlers + Send + Sync + 'static,
        C: pubsub::Client + Send + Sync + 'static,
    {
        let my_rpc_topic = format!("{}{}", config.node_rpc_topic, config.name);
        let rx = client
            .take_data_channel(config.queue_size)
            .ok_or_else(|| Error::failed("the data channel is already taken"))?;
        tokio::time::timeout(config.timeout, client.subscribe(&my_rpc_topic, config.qos)).await??;
        let client = Arc::new(client);
        let calls: CallMap = <_>::default();
        let tester: Arc<Tester> = <_>::default();
        let qos = config.qos;
        let node_rpc_topic = Arc::new(config.node_rpc_topic.to_owned());
        let processor_fut = Arc::new(Mutex::new(tokio::spawn(processor(
            rx,
            client.clone(),
            calls.clone(),
            my_rpc_topic,
            Arc::new(handlers),
            tester.clone(),
            node_rpc_topic.clone(),
            qos,
        ))));
        let pfut = processor_fut.clone();
        let cl = client.clone();
        let test_failed = config.test_failed.clone();
        let timeout = config.timeout;
        let ping_interval = config.ping_interval;
        let tester_fut = tokio::spawn(async move {
            loop {
                tokio::time::sleep(ping_interval).await;
                if let Err(e) = pstest(tester.clone(), cl.clone(), timeout).await {
                    error!("PubSub server test failed: {}", e);
                    if let Some(ref tf) = test_failed {
                        tf.trigger();
                    }
                    pfut.lock().unwrap().abort();
                    let _r = cl.bye().await;
                } else {
                    debug!("PubSub server test passed");
                }
            }
        });
        Ok(Self {
            name: config.name.to_owned(),
            timeout,
            client,
            processor_fut,
            tester_fut,
            calls,
            qos: config.qos,
            node_rpc_topic,
        })
    }
    #[inline]
    pub fn is_connected(&self) -> bool {
        self.client.is_connected()
    }
    #[inline]
    pub fn qos(&self) -> i32 {
        self.qos
    }
    /// # Panics
    ///
    /// Will panic if the calls mutex ins poisoned
    pub async fn call(
        &self,
        target: &str,
        method: &str,
        params: Option<&Value>,
        opts: &options::Options,
    ) -> EResult<Value> {
        let call_id = Uuid::new_v4();
        trace!("RPC call {} {}::{}", call_id, target, method);
        let mut message: Vec<u8> = vec![PROTO_VERSION, RPC_REQUEST, opts.flags(), 0x00, 0x00];
        message.extend(self.name.as_bytes());
        message.push(0x00);
        if let Some(key_id) = opts.key_id() {
            message.extend(key_id.as_bytes());
        }
        message.push(0x00);
        let mut payload = Vec::new();
        payload.extend(call_id.as_u128().to_le_bytes());
        payload.extend(method.as_bytes());
        payload.push(0x00);
        if let Some(p) = params {
            payload.extend(pack(p).map_err(Error::invalid_data)?);
        }
        message.extend(opts.pack_payload(payload).await?);
        let (tx, rx) = oneshot::channel();
        self.calls.lock().unwrap().insert(call_id, tx);
        macro_rules! unwrap_or_cancel {
            ($result: expr) => {
                match $result {
                    Ok(v) => v,
                    Err(e) => {
                        self.calls.lock().unwrap().remove(&call_id);
                        return Err(Into::<Error>::into(e).into());
                    }
                }
            };
        }
        let topic = format!("{}{}", self.node_rpc_topic, target);
        let fut = self.client.publish(&topic, message, self.qos);
        unwrap_or_cancel!(unwrap_or_cancel!(
            tokio::time::timeout(self.timeout, fut).await
        ));
        let res = rx.await.map_err(Into::<Error>::into)?;
        let payload = opts.unpack_payload(res.message, 20).await?;
        match res.kind {
            ReplyKind::Success => Ok(unpack(&payload).map_err(Error::invalid_data)?),
            ReplyKind::Error => {
                if payload.len() < 2 {
                    Err(Error::invalid_data("invalid reply err payload"))
                } else {
                    let code = i16::from_le_bytes([payload[0], payload[1]]);
                    if payload.len() > 2 {
                        Err(Error::new(
                            ErrorKind::from(code),
                            std::str::from_utf8(&payload[2..])?,
                        ))
                    } else {
                        Err(Error::new0(ErrorKind::from(code)))
                    }
                }
            }
        }
    }
    #[inline]
    pub fn client(&self) -> Arc<dyn pubsub::Client + Send + Sync> {
        self.client.clone()
    }
}

impl Drop for RpcClient {
    fn drop(&mut self) {
        self.tester_fut.abort();
        self.processor_fut.lock().unwrap().abort();
    }
}

#[cfg(test)]
mod test {
    use super::options::{Compression, Encryption, EncryptionKey, Options};

    #[tokio::test]
    async fn test_pack_unpack() {
        let payload = "hello I am here".as_bytes().to_vec();
        macro_rules! test_opts {
            ($opts: expr) => {
                let data = $opts.pack_payload(payload.clone()).await.unwrap();
                let mut buf = vec![1, 2, 3];
                buf.extend(data);
                let message = Box::new(paho_mqtt::message::Message::new("test", buf, 0));
                assert_eq!($opts.unpack_payload(message, 3).await.unwrap(), payload);
            };
        }
        let opts = Options::new();
        test_opts!(opts);
        let opts = Options::new().compression(Compression::Bzip2);
        test_opts!(opts);
        let opts =
            Options::new().encryption(Encryption::Aes128Gcm, &EncryptionKey::new("x", "hello123"));
        test_opts!(opts);
        let opts = Options::new()
            .compression(Compression::Bzip2)
            .encryption(Encryption::Aes128Gcm, &EncryptionKey::new("x", "hello123"));
        test_opts!(opts);
        let opts =
            Options::new().encryption(Encryption::Aes256Gcm, &EncryptionKey::new("x", "hello123"));
        test_opts!(opts);
        let opts = Options::new()
            .compression(Compression::Bzip2)
            .encryption(Encryption::Aes256Gcm, &EncryptionKey::new("x", "hello123"));
        test_opts!(opts);
    }
}
