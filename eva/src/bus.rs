/// Bus module
///
/// Contains a wrapper for the bus broker
use crate::EResult;
use busrt::broker::{AaaMap, AsyncAllocator, Broker, Client, ClientAaa, Options, ServerConfig};
use eva_common::registry;
use eva_common::tools::format_path;
use ipnetwork::IpNetwork;
use log::debug;
use serde::Deserialize;
use std::sync::Arc;
use std::time::Duration;

#[derive(Deserialize)]
struct BusClientConfig {
    name: String,
    #[serde(default)]
    hosts: Vec<IpNetwork>,
    #[serde(default)]
    p2p: Vec<String>,
    #[serde(default)]
    broadcast: Vec<String>,
    #[serde(default)]
    publish: Vec<String>,
    #[serde(default)]
    subscribe: Vec<String>,
}

#[derive(Deserialize)]
struct BusSocketConfig {
    path: String,
    buf_size: Option<usize>,
    #[serde(
        default,
        deserialize_with = "eva_common::tools::de_opt_float_as_duration_us"
    )]
    buf_ttl: Option<Duration>,
    #[serde(
        default,
        deserialize_with = "eva_common::tools::de_opt_float_as_duration"
    )]
    timeout: Option<Duration>,
    clients: Option<Vec<BusClientConfig>>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum BusSocket {
    Simple(String),
    WithConfig(BusSocketConfig),
}

impl BusSocket {
    fn path(&self) -> &str {
        match self {
            BusSocket::Simple(s) => s,
            BusSocket::WithConfig(v) => &v.path,
        }
    }
    fn with_path(self, path: String) -> Self {
        match self {
            BusSocket::Simple(_) => BusSocket::Simple(path),
            BusSocket::WithConfig(mut v) => {
                v.path = path;
                BusSocket::WithConfig(v)
            }
        }
    }
    fn server_config(
        &self,
        default_buf_size: usize,
        default_buf_ttl: Duration,
        default_timeout: Duration,
    ) -> ServerConfig {
        match self {
            BusSocket::Simple(_) => ServerConfig::new()
                .buf_size(default_buf_size)
                .buf_ttl(default_buf_ttl)
                .timeout(default_timeout),
            BusSocket::WithConfig(v) => {
                let server_config = ServerConfig::new()
                    .buf_size(v.buf_size.unwrap_or(default_buf_size))
                    .buf_ttl(v.buf_ttl.unwrap_or(default_buf_ttl))
                    .timeout(v.timeout.unwrap_or(default_timeout));
                if let Some(ref clients) = v.clients {
                    let aaa_map = AaaMap::default();
                    {
                        let mut map = aaa_map.lock();
                        for client in clients {
                            map.insert(
                                client.name.clone(),
                                ClientAaa::new()
                                    .hosts_allow(client.hosts.clone())
                                    .allow_p2p_to(
                                        &client
                                            .p2p
                                            .iter()
                                            .map(String::as_str)
                                            .collect::<Vec<&str>>(),
                                    )
                                    .allow_broadcast_to(
                                        &client
                                            .broadcast
                                            .iter()
                                            .map(String::as_str)
                                            .collect::<Vec<&str>>(),
                                    )
                                    .allow_subscribe_to(
                                        &client
                                            .subscribe
                                            .iter()
                                            .map(String::as_str)
                                            .collect::<Vec<&str>>(),
                                    )
                                    .allow_publish_to(
                                        &client
                                            .publish
                                            .iter()
                                            .map(String::as_str)
                                            .collect::<Vec<&str>>(),
                                    ),
                            );
                        }
                    }
                    server_config.aaa_map(aaa_map)
                } else {
                    server_config
                }
            }
        }
    }
}

#[derive(Deserialize)]
struct BusConfig {
    queue_size: usize,
    buf_size: usize,
    #[serde(deserialize_with = "eva_common::tools::de_float_as_duration_us")]
    buf_ttl: Duration,
    sockets: Vec<BusSocket>,
}

#[macro_export]
macro_rules! is_unix_socket {
    ($path: expr) => {
        $path.starts_with("/")
            || $path.ends_with(".sock")
            || $path.ends_with(".socket")
            || $path.ends_with(".ipc")
    };
}

pub struct EvaBroker {
    broker: Broker,
    config: BusConfig,
    queue_size: usize,
}

struct AllocationReq {
    size: usize,
    response: oneshot::Sender<Vec<u8>>,
    client: String,
}

pub struct ThreadAsyncAllocator {
    tx: async_channel::Sender<AllocationReq>,
    rx: async_channel::Receiver<AllocationReq>,
}

impl Default for ThreadAsyncAllocator {
    fn default() -> Self {
        let (tx, rx) = async_channel::bounded(32768);
        Self { tx, rx }
    }
}

impl ThreadAsyncAllocator {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn run(&self) {
        let rx = self.rx.clone();
        std::thread::spawn(move || {
            loop {
                while let Ok(req) = rx.recv_blocking() {
                    log::warn!(
                        "ThreadAsyncAllocator: allocating {} bytes for {}",
                        req.size,
                        req.client
                    );
                    let buf = vec![0; req.size];
                    req.response.send(buf).ok();
                }
            }
        });
    }
}

#[async_trait::async_trait]
impl AsyncAllocator for ThreadAsyncAllocator {
    async fn allocate(&self, client: &str, size: usize) -> Option<Vec<u8>> {
        let (tx, rx) = oneshot::channel();
        let req = AllocationReq {
            size,
            response: tx,
            client: client.to_owned(),
        };
        self.tx.send(req).await.ok()?;
        rx.await.ok()
    }
}

impl EvaBroker {
    pub fn new_from_db(
        db: &mut yedb::Database,
        dir_eva: &str,
        alloc: Option<(usize, Arc<ThreadAsyncAllocator>)>,
    ) -> EResult<EvaBroker> {
        let mut config: BusConfig =
            serde_json::from_value(db.key_get(&registry::format_config_key("bus"))?)?;
        debug!("bus.buf_size = {}", config.buf_size);
        debug!("bus.buf_ttl = {:?}", config.buf_ttl);
        debug!("bus.queue_size = {}", config.queue_size);
        let mut formatted_sockets = Vec::new();
        for socket in config.sockets {
            #[allow(clippy::case_sensitive_file_extension_comparisons)]
            if is_unix_socket!(socket.path()) {
                let path = format_path(dir_eva, Some(socket.path()), None);
                formatted_sockets.push(socket.with_path(path));
            } else {
                formatted_sockets.push(socket);
            }
        }
        config.sockets = formatted_sockets;
        debug!(
            "bus.sockets = {}",
            config
                .sockets
                .iter()
                .map(BusSocket::path)
                .collect::<Vec<&str>>()
                .join(",")
        );
        let mut opts = Options::default().force_register(true);
        if let Some((limit, aa)) = alloc {
            opts = opts.with_async_allocator(limit, aa);
        }
        let mut broker = Broker::create(&opts);
        broker.set_queue_size(config.queue_size);
        let queue_size = config.queue_size;
        Ok(EvaBroker {
            broker,
            config,
            queue_size,
        })
    }
    pub async fn init(&mut self, core: &mut crate::core::Core) -> EResult<()> {
        self.broker.init_default_core_rpc().await?;
        for socket in &self.config.sockets {
            let server_config =
                socket.server_config(self.config.buf_size, self.config.buf_ttl, core.timeout());
            #[allow(clippy::case_sensitive_file_extension_comparisons)]
            if let Some(ws_path) = socket.path().strip_prefix("ws://") {
                self.broker
                    .spawn_websocket_server(ws_path, server_config, None)
                    .await?;
            } else if is_unix_socket!(socket.path()) {
                core.add_file_to_remove(socket.path());
                self.broker
                    .spawn_unix_server(socket.path(), server_config)
                    .await?;
            } else {
                self.broker
                    .spawn_tcp_server(socket.path(), server_config)
                    .await?;
            }
        }
        Ok(())
    }
    #[inline]
    pub async fn register_client(&self, name: &str) -> EResult<Client> {
        self.broker.register_client(name).await.map_err(Into::into)
    }
    #[inline]
    pub async fn register_secondary_for(&self, client: &Client) -> EResult<Client> {
        self.broker
            .register_secondary_for(client)
            .await
            .map_err(Into::into)
    }
    #[inline]
    pub fn queue_size(&self) -> usize {
        self.queue_size
    }
}
