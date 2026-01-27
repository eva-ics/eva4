use crate::{UI_NOT_FOUND_TO_BASE, WS_URI, aaa};
use crate::{serve, upload};
use eva_common::acl::{OIDMask, OIDMaskList};
use eva_common::events::{LOCAL_STATE_TOPIC, REMOTE_STATE_TOPIC};
use eva_common::hyper_response;
use eva_common::hyper_tools::HResultX;
use eva_common::prelude::*;
use eva_common::{OID_MASK_PREFIX_FORMULA, OID_MASK_PREFIX_REGEX};
use eva_sdk::prelude::*;
use eva_sdk::types::FullRemoteItemState;
use futures::{
    sink::SinkExt,
    stream::{SplitSink, StreamExt},
};
use hyper::{Body, Method, Request, Response, StatusCode, http};
use hyper_tungstenite::{HyperWebsocket, WebSocketStream, tungstenite};
use log::error;
use parking_lot::Mutex;
use rjrpc::http::HyperJsonRpcServer;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, hash_map};
use std::net::IpAddr;
use std::sync::{Arc, LazyLock, OnceLock, atomic};
use std::time::Duration;
use submap::SubMap;
use tungstenite::Message;
use uuid::Uuid;

err_logger!();

#[derive(Serialize, Eq, PartialEq, Copy, Clone, Hash)]
#[serde(rename_all = "snake_case")]
enum Topic {
    State,
    Log,
    Pong,
    Reload,
    Server,
    Stream,
    Bye,
}

#[derive(Serialize, Eq, PartialEq, Copy, Clone, Hash)]
#[serde(rename_all = "lowercase")]
pub enum ServerEvent {
    Restart,
}

const ERR_INVALID_IP: &str = "Invalid IP address";
const WS_QUEUE: usize = 32768;

pub static HJRPC: OnceLock<HyperJsonRpcServer> = OnceLock::new();
pub static WS_SUB: LazyLock<Mutex<SubMap<Arc<WsTx>>>> = LazyLock::new(|| {
    Mutex::new(
        SubMap::new()
            .separator('/')
            .match_any("+")
            .wildcard("#")
            .formula_prefix(OID_MASK_PREFIX_FORMULA)
            .regex_prefix(OID_MASK_PREFIX_REGEX),
    )
});
pub static WS_SUB_LOG: LazyLock<Mutex<SubMap<Arc<WsTx>>>> = LazyLock::new(|| {
    Mutex::new(
        SubMap::new()
            .separator('/')
            .match_any("+")
            .wildcard("#")
            .formula_prefix(OID_MASK_PREFIX_FORMULA)
            .regex_prefix(OID_MASK_PREFIX_REGEX),
    )
});

// web sockets not registered with any token
pub static WS_STANDALONE: LazyLock<tokio::sync::Mutex<HashMap<Uuid, WebSocket>>> =
    LazyLock::new(<_>::default);

type WsStreamMap = BTreeMap<Arc<OID>, BTreeSet<Arc<WsTx>>>;

// streams
pub static WS_STREAMS: LazyLock<tokio::sync::Mutex<WsStreamMap>> = LazyLock::new(<_>::default);
pub static WS_STREAM_HANDLER_TX: OnceLock<async_channel::Sender<(OID, Arc<WsTx>)>> =
    OnceLock::new();

#[derive(Serialize)]
pub struct StreamInfo {
    pub oid: Arc<OID>,
    pub key: String,
}

impl Ord for StreamInfo {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.oid
            .cmp(&other.oid)
            .then_with(|| self.key.cmp(&other.key))
    }
}

impl PartialOrd for StreamInfo {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for StreamInfo {
    fn eq(&self, other: &Self) -> bool {
        self.oid == other.oid && self.key == other.key
    }
}

impl Eq for StreamInfo {}

pub async fn stream_info_list() -> Vec<StreamInfo> {
    let mut result = Vec::new();
    {
        let streams = WS_STREAMS.lock().await;
        for (oid, clients) in &*streams {
            for client in clients {
                let key = if let Some(token) = client.auth.token() {
                    token.id().to_string()
                } else {
                    client.auth.key_id().unwrap_or_default().to_owned()
                };
                result.push(StreamInfo {
                    oid: oid.clone(),
                    key,
                });
            }
        }
    }
    result.sort();
    result
}

struct WsRepl(Arc<OID>, IEID);

fn notify_ws(c: &WsTx, frame: Cow<WsFrame>, repl: Option<&WsRepl>) -> EResult<()> {
    let res = if let Some(repl) = repl {
        c.try_send_state_frame(frame, repl)
    } else {
        c.tx.try_send(frame.into_owned())
    };
    if let Err(e) = res {
        if let async_channel::TrySendError::Full(_) = e {
            warn!("web socket {} buffer is full, terminating", c.id);
        }
        c.ask_terminate();
        return Err(Error::failed(e));
    }
    Ok(())
}

fn notify_ws_list(ws_list: Vec<Arc<WsTx>>, frame: &WsFrame, repl: Option<WsRepl>) {
    for c in ws_list {
        let _ = notify_ws(&c, Cow::Borrowed(frame), repl.as_ref());
    }
}

pub fn notify_server_event(event: ServerEvent) {
    let clients = WS_SUB.lock().list_clients();
    if !clients.is_empty() {
        let frame = WsFrame::new_server_event(event);
        notify_ws_list(clients, &frame, None);
    }
}

pub fn notify_need_reload() {
    let clients = WS_SUB.lock().list_clients();
    if !clients.is_empty() {
        let frame = WsFrame::new_reload();
        notify_ws_list(clients, &frame, None);
    }
}

#[allow(clippy::mutable_key_type)]
pub fn notify_state(state: FullRemoteItemState) -> EResult<()> {
    let mut clients = WS_SUB.lock().get_subscribers(state.oid.as_path());
    clients.retain(|c| c.auth.acl().check_item_read(&state.oid));
    let ws_repl = WsRepl(Arc::new(state.oid.clone()), state.ieid);
    if !clients.is_empty() {
        let frame = WsFrame::new_state(to_value(state)?);
        notify_ws_list(clients.into_iter().collect(), &frame, Some(ws_repl));
    }
    Ok(())
}

#[allow(clippy::mutable_key_type)]
pub fn notify_log(topic: &str, record: Value) {
    let clients = WS_SUB_LOG.lock().get_subscribers(topic);
    if !clients.is_empty() {
        let frame = WsFrame::new_log(record);
        notify_ws_list(clients.into_iter().collect(), &frame, None);
    }
}

#[derive(Clone)]
enum WsFrame {
    Text(Arc<WsTextFrame>),
    Binary(Vec<u8>),
}

#[derive(Serialize, Clone)]
struct WsTextFrame {
    s: Topic,
    #[serde(skip_serializing_if = "Option::is_none")]
    d: Option<Value>,
}

impl WsFrame {
    fn new_binary(data: Vec<u8>) -> Self {
        Self::Binary(data)
    }
    #[inline]
    fn new_state(d: Value) -> Self {
        Self::Text(
            WsTextFrame {
                s: Topic::State,
                d: Some(d),
            }
            .into(),
        )
    }
    #[inline]
    fn new_log(d: Value) -> Self {
        Self::Text(
            WsTextFrame {
                s: Topic::Log,
                d: Some(d),
            }
            .into(),
        )
    }
    #[inline]
    fn new_bye() -> Self {
        Self::Text(
            WsTextFrame {
                s: Topic::Bye,
                d: None,
            }
            .into(),
        )
    }
    #[inline]
    fn is_data(&self) -> bool {
        let Self::Text(s) = self else {
            return false;
        };
        s.s == Topic::State
    }
    #[inline]
    fn is_bye(&self) -> bool {
        let Self::Text(s) = self else {
            return false;
        };
        s.s == Topic::Bye
    }
    #[inline]
    fn new_pong() -> Self {
        Self::Text(
            WsTextFrame {
                s: Topic::Pong,
                d: None,
            }
            .into(),
        )
    }
    #[inline]
    fn new_reload() -> Self {
        Self::Text(
            WsTextFrame {
                s: Topic::Reload,
                d: Some(Value::String("asap".to_owned())),
            }
            .into(),
        )
    }
    #[inline]
    fn new_server_event(event: ServerEvent) -> Self {
        Self::Text(
            WsTextFrame {
                s: Topic::Server,
                d: Some(to_value(event).unwrap()),
            }
            .into(),
        )
    }
}

#[derive(Serialize)]
struct WsBulkFrame {
    s: Topic,
    d: Vec<Value>,
}

impl WsBulkFrame {
    fn new(topic: Topic, values: &mut Vec<Value>) -> Self {
        let mut vals = Vec::new();
        vals.append(values);
        Self { s: topic, d: vals }
    }
}

#[derive(Debug)]
pub struct WsTx {
    id: Uuid,
    auth: aaa::Auth,
    tx: async_channel::Sender<WsFrame>,
    repl: Mutex<HashMap<Arc<OID>, IEID>>,
}

impl WsTx {
    fn ask_terminate(&self) {
        if let Some(token) = self.auth.token() {
            token.unregister_websocket(self.id);
            self.tx.close();
        } else {
            let id = self.id;
            tokio::spawn(async move {
                abort_web_socket_standalone(id).await;
            });
        }
    }
    fn try_send_state_frame(
        &self,
        frame: Cow<WsFrame>,
        ws_repl: &WsRepl,
    ) -> Result<(), async_channel::TrySendError<WsFrame>> {
        match self.repl.lock().entry(ws_repl.0.clone()) {
            hash_map::Entry::Vacant(entry) => {
                entry.insert(ws_repl.1);
            }
            hash_map::Entry::Occupied(mut entry) => {
                if *entry.get() > ws_repl.1 {
                    return Ok(());
                }
                entry.insert(ws_repl.1);
            }
        }
        self.tx.try_send(frame.into_owned())
    }
    async fn send_initial(&self, oids: Option<OIDMaskList>) -> EResult<()> {
        #[derive(Serialize)]
        struct StatePayload {
            i: OIDMaskList,
            include: Vec<String>,
            exclude: Vec<String>,
        }
        if let Some(oids) = oids {
            let (allow, deny) = self.auth.acl().get_items_allow_deny_reading();
            let payload = StatePayload {
                i: oids,
                include: allow,
                exclude: deny,
            };
            let states: Vec<FullRemoteItemState> = unpack(
                eapi_bus::call("eva.core", "item.state", pack(&payload)?.into())
                    .await?
                    .payload(),
            )?;
            for state in states {
                let ws_repl = WsRepl(Arc::new(state.oid.clone()), state.ieid);
                let frame = WsFrame::new_state(to_value(state)?);
                if notify_ws(self, Cow::Owned(frame), Some(&ws_repl)).is_err() {
                    break;
                }
            }
        }
        Ok(())
    }
}

//impl Hash for WsTx {
//fn hash<H: Hasher>(&self, state: &mut H) {
//self.id.hash(state);
//}
//}

impl Eq for WsTx {}

impl PartialEq for WsTx {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Ord for WsTx {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.id.cmp(&other.id)
    }
}

impl PartialOrd for WsTx {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Deserialize)]
struct WsCommand {
    #[serde(alias = "m")]
    method: String,
    #[serde(alias = "p")]
    params: Option<Value>,
}

#[async_trait::async_trait]
trait FrameX {
    async fn send_data<D: Serialize + Sync>(&mut self, data: &D) -> EResult<()>;
}

type WsSenderStream = SplitSink<WebSocketStream<hyper::upgrade::Upgraded>, Message>;

#[async_trait::async_trait]
impl FrameX for WsSenderStream {
    async fn send_data<D: Serialize + Sync>(&mut self, data: &D) -> EResult<()> {
        let payload = serde_json::to_string(data)?;
        self.send(Message::text(payload)).await.map_err(Error::io)
    }
}

async fn serve_websocket_sender(
    ws_tx: Arc<WsTx>,
    mut sender: WsSenderStream,
    rx: async_channel::Receiver<WsFrame>,
    buf_ttl: Option<f64>,
) {
    macro_rules! send {
        ($frame: expr) => {
            if sender.send_data($frame).await.is_err() {
                ws_tx.ask_terminate();
                break;
            }
        };
    }
    macro_rules! send_binary {
        ($data: expr) => {
            if sender.send(Message::Binary($data)).await.is_err() {
                ws_tx.ask_terminate();
                break;
            }
        };
    }
    if let Some(bttl) = buf_ttl {
        let mut event_buf: HashMap<Topic, Vec<Value>> = HashMap::new();
        let mut buf_interval = tokio::time::interval(Duration::from_secs_f64(bttl));
        loop {
            tokio::select! {
            f = rx.recv() => {
                if let Ok(frame) = f {
                    if frame.is_data() {
                        let WsFrame::Text(frame) = frame else {
                            continue;
                        };
                        if let Some(ref data) = frame.d {
                            if let Some(v) = event_buf.get_mut(&frame.s) {
                                v.push(data.clone());
                            } else {
                                event_buf.insert(frame.s, vec![data.clone()]);
                            }
                        }
                    } else if frame.is_bye() {
                        let WsFrame::Text(frame) = frame else {
                            continue;
                        };
                        send!(&frame);
                        break;
                    } else {
                        match frame {
                            WsFrame::Text(frame) => send!(&frame),
                            WsFrame::Binary(data) => {
                                send_binary!(data);
                            }
                        }
                    }
                } else {
                    break;
                }
            }
            _r = buf_interval.tick() => {
                    for (topic, values) in &mut event_buf {
                        if !values.is_empty() {
                            let bulk_frame = WsBulkFrame::new(*topic, values);
                            send!(&bulk_frame);
                        }
                    }
                }
            }
        }
    } else {
        while let Ok(frame) = rx.recv().await {
            let is_bye = frame.is_bye();
            match frame {
                WsFrame::Text(ref frame) => {
                    send!(frame);
                }
                WsFrame::Binary(data) => {
                    send_binary!(data);
                }
            }
            if is_bye {
                break;
            }
        }
    }
    // ensure nothing is sent
    sender.close().await.ok();
    rx.close();
}

pub fn get_log_topics(level: u8) -> Vec<&'static str> {
    let mut result = Vec::new();
    if level <= eva_common::LOG_LEVEL_INFO {
        result.push("info");
    }
    if level <= eva_common::LOG_LEVEL_WARN {
        result.push("warn");
    }
    if level <= eva_common::LOG_LEVEL_ERROR {
        result.push("error");
    }
    result
}

pub async fn remove_websocket_by_acl_id(acl_id: &str) {
    WS_STANDALONE
        .lock()
        .await
        .retain(|_, ws| ws.acl_modified_keep(acl_id));
}

pub async fn remove_websocket_by_key_id(key_id: &str) {
    WS_STANDALONE
        .lock()
        .await
        .retain(|_, ws| ws.key_modified_keep(key_id));
}

#[allow(clippy::too_many_lines)]
async fn serve_websocket(
    ws_tx: Arc<WsTx>,
    rx: async_channel::Receiver<WsFrame>,
    websocket: HyperWebsocket,
    buf_ttl: Option<f64>,
    oids_masks: Option<OIDMaskList>,
) -> EResult<()> {
    if let Some(masks) = oids_masks {
        let mut map = WS_SUB.lock();
        for mask in masks.oid_masks() {
            map.subscribe(&mask.as_path(), &ws_tx);
        }
    }
    let websocket = websocket.await.map_err(Error::io)?;
    let (sender, mut receiver) = websocket.split();
    let ws_tx_c = ws_tx.clone();
    let sender_fut = tokio::spawn(async move {
        serve_websocket_sender(ws_tx_c, sender, rx, buf_ttl).await;
    });
    while let Some(message) = receiver.next().await {
        #[allow(clippy::single_match)]
        match message {
            Err(e) => {
                sender_fut.abort();
                if let tungstenite::Error::Protocol(ref p_e) = e
                    && *p_e == tungstenite::error::ProtocolError::ResetWithoutClosingHandshake
                {
                    return Ok(());
                }
                return Err(Error::io(e));
            }
            Ok(tungstenite::Message::Text(msg)) => {
                if !msg.is_empty() {
                    let cmd: WsCommand = serde_json::from_str(&msg).log_err()?;
                    let method = cmd.method.as_str();
                    trace!("web socket {} {}", ws_tx.id, method);
                    match method {
                        "stream.start" => {
                            #[derive(Deserialize)]
                            struct StreamParams {
                                i: OID,
                            }
                            let Some(p) = cmd.params else {
                                continue;
                            };
                            let Ok(params) = StreamParams::deserialize(p) else {
                                warn!("invalid blob stream params");
                                continue;
                            };
                            if !ws_tx.auth.acl().check_item_read(&params.i) {
                                let frame = WsFrame::Text(
                                    WsTextFrame {
                                        s: Topic::Stream,
                                        d: Some(Value::String("forbidden".to_owned())),
                                    }
                                    .into(),
                                );
                                if notify_ws(&ws_tx, Cow::Owned(frame), None).is_err() {
                                    break;
                                }
                                continue;
                            }
                            let frame = WsFrame::Text(
                                WsTextFrame {
                                    s: Topic::Stream,
                                    d: Some(Value::String("start".to_owned())),
                                }
                                .into(),
                            );
                            if notify_ws(&ws_tx, Cow::Owned(frame), None).is_err() {
                                break;
                            }
                            WS_STREAM_HANDLER_TX
                                .get()
                                .unwrap()
                                .send((params.i, ws_tx.clone()))
                                .await
                                .log_ef();
                        }
                        "subscribe.state" => {
                            if let Some(p) = cmd.params
                                && let Ok(masks) = HashSet::<String>::deserialize(p).log_err()
                            {
                                let mut sub_oid_masks: HashSet<OIDMask> =
                                    HashSet::with_capacity(masks.len());
                                for mask in masks {
                                    let Ok(oid_mask) = mask.parse::<OIDMask>() else {
                                        warn!(
                                            "invalid OID mask from web socket subscription: {}",
                                            mask
                                        );
                                        continue;
                                    };
                                    sub_oid_masks.insert(oid_mask);
                                }
                                let mut map = WS_SUB.lock();
                                for mask in sub_oid_masks {
                                    map.subscribe(&mask.as_path(), &ws_tx);
                                }
                            }
                        }
                        "unsubscribe.state" => {
                            let mut map = WS_SUB.lock();
                            map.unsubscribe_all(&ws_tx);
                        }
                        "subscribe.state_initial" => {
                            if let Some(p) = cmd.params
                                && let Ok(masks) = HashSet::<String>::deserialize(p).log_err()
                            {
                                let mut sub_oid_masks: HashSet<OIDMask> =
                                    HashSet::with_capacity(masks.len());
                                for mask in masks {
                                    let Ok(oid_mask) = mask.parse::<OIDMask>() else {
                                        warn!(
                                            "invalid OID mask from web socket subscription: {}",
                                            mask
                                        );
                                        continue;
                                    };
                                    sub_oid_masks.insert(oid_mask);
                                }
                                let mut map = WS_SUB.lock();
                                for mask in &sub_oid_masks {
                                    map.subscribe(&mask.as_path(), &ws_tx);
                                }
                                let ws_tx_c = ws_tx.clone();
                                tokio::spawn(async move {
                                    ws_tx_c
                                        .send_initial(Some(OIDMaskList::new(sub_oid_masks)))
                                        .await
                                        .log_ef();
                                });
                            }
                        }
                        "subscribe.log" => {
                            if let Some(p) = cmd.params
                                && let Ok(level) = u8::deserialize(p).log_err()
                            {
                                let mut map = WS_SUB_LOG.lock();
                                for t in get_log_topics(0) {
                                    map.unsubscribe(t, &ws_tx);
                                }
                                for t in get_log_topics(level) {
                                    map.subscribe(t, &ws_tx);
                                }
                            }
                        }
                        "ping" => {
                            ws_tx
                                .tx
                                .send(WsFrame::new_pong())
                                .await
                                .map_err(Error::io)?;
                        }
                        "bye" => {
                            break;
                        }
                        m => {
                            warn!("invalid web socket method: {}", m);
                            break;
                        }
                    }
                }
            }
            _ => {}
        }
    }
    sender_fut.abort();
    Ok(())
}

fn unsubscribe_web_socket(ws_tx: &Arc<WsTx>) {
    WS_SUB.lock().unregister_client(ws_tx);
    WS_SUB_LOG.lock().unregister_client(ws_tx);
}

async fn abort_web_socket_standalone(ws_id: Uuid) {
    if let Some(ws) = WS_STANDALONE.lock().await.remove(&ws_id) {
        ws.terminate();
    }
}

#[derive(Debug)]
pub struct WebSocket {
    id: Uuid,
    handler: tokio::task::JoinHandle<()>,
    ws_tx: Arc<WsTx>,
}

impl WebSocket {
    pub fn id(&self) -> Uuid {
        self.id
    }
    pub fn terminate(&self) {
        self.handler.abort();
        let tx = self.ws_tx.tx.clone();
        tokio::spawn(async move {
            let _ = tx.send(WsFrame::new_bye()).await;
            tx.close();
        });
        unsubscribe_web_socket(&self.ws_tx);
    }
    fn acl_modified_keep(&self, acl_id: &str) -> bool {
        if let aaa::Auth::Key(_, acl) = &self.ws_tx.auth
            && acl.id() == acl_id
        {
            self.terminate();
            return false;
        }
        true
    }
    fn key_modified_keep(&self, key_id: &str) -> bool {
        let k = format!(".{}", key_id);
        if let aaa::Auth::Key(id, _) = &self.ws_tx.auth
            && *id == k
        {
            self.terminate();
            return false;
        }
        true
    }
}

#[inline]
fn get_real_ip(
    ip: IpAddr,
    headers: &hyper::header::HeaderMap,
    real_ip_header: Arc<Option<String>>,
) -> Result<IpAddr, Error> {
    if let Some(ref ip_header) = *real_ip_header
        && let Some(h) = headers.get(ip_header)
    {
        let s = h
            .to_str()
            .map_err(|e| Error::failed(format!("invalid real ip header: {}", e)))?;
        return s.parse::<IpAddr>().map_err(|e| {
            Error::invalid_params(format!("Unable to parse client address {}: {}", s, e))
        });
    }
    Ok(ip)
}

pub async fn web(
    req: Request<Body>,
    ip: IpAddr,
    real_ip_header: Arc<Option<String>>,
) -> Result<Response<Body>, http::Error> {
    let ip_addr = match get_real_ip(ip, req.headers(), real_ip_header) {
        Ok(v) => v,
        Err(e) => {
            let message = format!("{}: {}", ERR_INVALID_IP, e);
            error!("{}", message);
            return hyper_response!(StatusCode::BAD_REQUEST, message);
        }
    };
    let method = req.method().clone();
    let uri = req.uri().clone();
    match handle_web_request(req, ip_addr).await {
        Ok(mut resp) => {
            if crate::development_mode() {
                resp.headers_mut().insert(
                    hyper::header::ACCESS_CONTROL_ALLOW_ORIGIN,
                    "*".try_into().unwrap(),
                );
                resp.headers_mut().insert(
                    hyper::header::ACCESS_CONTROL_ALLOW_HEADERS,
                    "content-type, x-auth-key".try_into().unwrap(),
                );
            }
            let status_code = resp.status().as_u16();
            let uri_string = uri.to_string();
            let display_uri = uri_string
                .find('?')
                .map_or(uri_string.as_str(), |pos| &uri_string[..pos]);
            if status_code < 400 {
                debug!(
                    r#"{} {} "{} {}" {}"#,
                    ip, ip_addr, method, display_uri, status_code
                );
            } else {
                warn!(
                    r#"{} {} "{} {}" {}"#,
                    ip, ip_addr, method, display_uri, status_code
                );
            }
            Ok(resp)
        }
        Err(e) => {
            error!("http request error: {}", e);
            Err(e)
        }
    }
}

#[allow(clippy::too_many_lines)]
async fn handle_web_request(req: Request<Body>, ip: IpAddr) -> Result<Response<Body>, http::Error> {
    if !svc_is_active() {
        return hyper_response!(StatusCode::SERVICE_UNAVAILABLE);
    }
    let path = req.uri().path();
    if (path == "/ws" || WS_URI.get().is_some_and(|ws_uri| path == ws_uri))
        && hyper_tungstenite::is_upgrade_request(&req)
    {
        let params: Option<HashMap<String, String>> = req.uri().query().map(|v| {
            url::form_urlencoded::parse(v.as_bytes())
                .into_owned()
                .collect()
        });
        let api_key: Option<&str> = params
            .as_ref()
            .and_then(|p| p.get("k").map(String::as_str))
            .or_else(|| {
                req.headers()
                    .get("x-auth-key")
                    .map(|v| v.to_str().unwrap_or_default())
            });
        if let Some(k) = api_key {
            match crate::aaa::authenticate(k, Some(ip)).await {
                Ok(auth) => {
                    let (response, websocket) = match hyper_tungstenite::upgrade(req, None) {
                        Ok(v) => v,
                        Err(e) => {
                            return hyper_response!(StatusCode::BAD_REQUEST, e.to_string());
                        }
                    };
                    let buf_ttl = if let Some(b) = params.as_ref().and_then(|p| p.get("buf_ttl")) {
                        let t = b.parse::<f64>().unwrap_or_default();
                        if t > 0.0 {
                            Some(t)
                        } else {
                            warn!("invalid ws buf ttl specified");
                            None
                        }
                    } else {
                        None
                    };
                    let oids = if let Some(o) = params.as_ref().and_then(|p| p.get("state")) {
                        let mut masks = HashSet::new();
                        for mask in o.split(',') {
                            match mask.parse() {
                                Ok(v) => {
                                    masks.insert(v);
                                }
                                Err(e) => {
                                    return hyper_response!(
                                        StatusCode::BAD_REQUEST,
                                        format!("invalid OID mask {mask}: {e}")
                                    );
                                }
                            }
                        }
                        Some(OIDMaskList::new(masks))
                    } else {
                        None
                    };
                    let send_initial: bool = params
                        .as_ref()
                        .is_some_and(|p| p.get("initial").is_some_and(|v| v == "1" || v == "true"));
                    let ws_id = Uuid::new_v4();
                    let (tx, rx) = async_channel::bounded::<WsFrame>(WS_QUEUE);
                    let auth_token = auth.clone_token();
                    let ws_tx = Arc::new(WsTx {
                        id: ws_id,
                        tx,
                        auth,
                        repl: <_>::default(),
                    });
                    let ws_tx_c = ws_tx.clone();
                    WS_SUB.lock().register_client(&ws_tx);
                    if ws_tx.auth.acl().check_op(eva_common::acl::Op::Log) {
                        WS_SUB_LOG.lock().register_client(&ws_tx);
                    }
                    let ws_standalone = if auth_token.is_none() {
                        Some(WS_STANDALONE.lock().await)
                    } else {
                        None
                    };
                    let oids_c = if send_initial { oids.clone() } else { None };
                    let ws_handler = tokio::spawn(async move {
                        if let Err(e) =
                            serve_websocket(ws_tx_c.clone(), rx, websocket, buf_ttl, oids).await
                        {
                            warn!("error in websocket connection: {}", e);
                        }
                        ws_tx_c.ask_terminate();
                    });
                    let ws_tx_c = ws_tx.clone();
                    let ws = WebSocket {
                        id: ws_id,
                        handler: ws_handler,
                        ws_tx: ws_tx_c,
                    };
                    if let Some(token) = auth_token {
                        token.register_websocket(ws);
                    } else {
                        ws_standalone.unwrap().insert(ws_id, ws);
                    }
                    if send_initial {
                        tokio::spawn(async move {
                            ws_tx.send_initial(oids_c).await.log_ef();
                        });
                    }
                    return Ok(response);
                }
                Err(e) => {
                    return hyper_response!(StatusCode::FORBIDDEN, e.to_string());
                }
            }
        }
        return hyper_response!(StatusCode::FORBIDDEN);
    }
    let (parts, body) = req.into_parts();
    let hjrpc = HJRPC.get().unwrap();
    if hjrpc.matches(&parts) {
        hjrpc.process(crate::api::processor, &parts, body, ip).await
    } else {
        let uri = parts.uri.path();
        match parts.method {
            Method::GET => {
                if uri == "/" {
                    return Response::builder()
                        .status(StatusCode::MOVED_PERMANENTLY)
                        .header(hyper::header::LOCATION, "/ui/")
                        .body(Body::from(""));
                }
                if uri == "/ui" {
                    return Response::builder()
                        .status(StatusCode::MOVED_PERMANENTLY)
                        .header(hyper::header::LOCATION, "/ui/")
                        .body(Body::from(""));
                }
                if (uri == "/va" || uri == "/va/") && crate::VENDORED_APPS_PATH.get().is_some() {
                    return Response::builder()
                        .status(StatusCode::MOVED_PERMANENTLY)
                        .header(hyper::header::LOCATION, "/ui/vendored-apps/")
                        .body(Body::from(""));
                }
                if let Some(vendored_app) = uri.strip_prefix("/va/")
                    && crate::VENDORED_APPS_PATH.get().is_some()
                {
                    return Response::builder()
                        .status(StatusCode::MOVED_PERMANENTLY)
                        .header(
                            hyper::header::LOCATION,
                            format!("/ui/vendored-apps/{}", vendored_app),
                        )
                        .body(Body::from(""));
                }
                if uri == "/favicon.ico" {
                    if let Some(ui_path) = crate::UI_PATH.get().unwrap() {
                        return serve::file(
                            uri,
                            ui_path,
                            false,
                            &uri[1..],
                            None,
                            false,
                            &parts.headers,
                            ip,
                            serve::TplDirKind::No,
                            true,
                        )
                        .await
                        .log_err()
                        .into_hyper_response();
                    }
                } else if uri.starts_with("/.evahi/")
                    && let Some(ui_path) = crate::UI_PATH.get().unwrap()
                {
                    return serve::file(
                        uri,
                        ui_path,
                        false,
                        &uri[1..],
                        None,
                        false,
                        &parts.headers,
                        ip,
                        serve::TplDirKind::No,
                        false,
                    )
                    .await
                    .log_err()
                    .into_hyper_response();
                }
                let params: Option<HashMap<String, String>> = parts.uri.query().map(|v| {
                    url::form_urlencoded::parse(v.as_bytes())
                        .into_owned()
                        .collect()
                });
                if let Some(va_file) = uri.strip_prefix("/ui/vendored-apps/")
                    && let Some(va_path) = crate::VENDORED_APPS_PATH.get()
                {
                    return serve::file(
                        uri,
                        va_path,
                        false,
                        va_file,
                        params.as_ref(),
                        true,
                        &parts.headers,
                        ip,
                        serve::TplDirKind::No,
                        false,
                    )
                    .await
                    .log_err()
                    .into_hyper_response();
                }
                if let Some(ui_file) = uri.strip_prefix("/ui/")
                    && let Some(ui_path) = crate::UI_PATH.get().unwrap()
                {
                    return serve::file(
                        uri,
                        ui_path,
                        UI_NOT_FOUND_TO_BASE.load(atomic::Ordering::Relaxed),
                        ui_file,
                        params.as_ref(),
                        true,
                        &parts.headers,
                        ip,
                        serve::TplDirKind::Ui,
                        true,
                    )
                    .await
                    .log_err()
                    .into_hyper_response();
                }
                if uri == "/pvt" {
                    if let Some(ref q) = params
                        && let Some(f) = q.get("f")
                    {
                        return serve::pvt(uri, f, params.as_ref(), &parts.headers, ip)
                            .await
                            .log_err()
                            .into_hyper_response();
                    }
                    return hyper_response!(StatusCode::BAD_REQUEST);
                }
                if let Some(pvt_file) = uri.strip_prefix("/pvt/") {
                    return serve::pvt(uri, pvt_file, params.as_ref(), &parts.headers, ip)
                        .await
                        .log_err()
                        .into_hyper_response();
                }
                if uri == "/rpvt" {
                    if let Some(ref q) = params
                        && let Some(f) = q.get("f")
                    {
                        return serve::remote_pvt(uri, f, params.as_ref(), &parts.headers, ip)
                            .await
                            .log_err()
                            .into_hyper_response();
                    }
                    return hyper_response!(StatusCode::BAD_REQUEST);
                }
                if let Some(rpvt_file) = uri.strip_prefix("/rpvt/") {
                    return serve::remote_pvt(uri, rpvt_file, params.as_ref(), &parts.headers, ip)
                        .await
                        .log_err()
                        .into_hyper_response();
                }
                if let Some(pvt_key) = uri.strip_prefix("/:pvt/") {
                    return serve::pvt_key(uri, pvt_key, params.as_ref(), &parts.headers, ip)
                        .await
                        .log_err()
                        .into_hyper_response();
                }
                if let Some(pub_key) = uri.strip_prefix("/:pub/") {
                    return serve::pub_key(uri, pub_key, params.as_ref())
                        .await
                        .log_err()
                        .into_hyper_response();
                }
                hyper_response!(StatusCode::NOT_FOUND)
            }
            Method::POST => {
                if uri == "/upload"
                    && let Some(boundary) = parts
                        .headers
                        .get(hyper::header::CONTENT_TYPE)
                        .and_then(|ct| ct.to_str().ok())
                        .and_then(|ct| multer::parse_boundary(ct).ok())
                {
                    return upload::process(body, boundary, &parts.headers, ip)
                        .await
                        .log_err()
                        .into_hyper_response();
                }
                hyper_response!(StatusCode::NOT_FOUND)
            }
            Method::OPTIONS => Response::builder()
                .status(StatusCode::NO_CONTENT)
                .body(Body::from("")),
            _ => hyper_response!(StatusCode::METHOD_NOT_ALLOWED),
        }
    }
}

fn push_oid_state_topic(oid: &OID, topics: &mut Vec<String>) {
    topics.push(format!("{}{}", LOCAL_STATE_TOPIC, oid.as_path()));
    topics.push(format!("{}{}", REMOTE_STATE_TOPIC, oid.as_path()));
}

#[allow(clippy::too_many_lines)]
pub fn spawn_stream_processor(mut bus_client: busrt::ipc::Client) -> EResult<()> {
    let event_rx = bus_client
        .take_event_channel()
        .ok_or_else(|| Error::core("Failed to take stream event channel"))?;
    let bus_client = Arc::new(tokio::sync::Mutex::new(bus_client));
    let mut int = tokio::time::interval(Duration::from_secs(1));
    // cleanup worker
    let bus_client_c = bus_client.clone();
    tokio::spawn(async move {
        loop {
            int.tick().await;
            let mut ws_streams = WS_STREAMS.lock().await;
            let mut topics_to_unsubscribe = Vec::new();
            for clients in ws_streams.values_mut() {
                clients.retain(|c| !c.tx.is_closed());
            }
            ws_streams.retain(|oid, clients| {
                if clients.is_empty() {
                    push_oid_state_topic(oid, &mut topics_to_unsubscribe);
                    false
                } else {
                    true
                }
            });
            if topics_to_unsubscribe.is_empty() {
                bus_client_c.lock().await.ping().await.ok();
                continue;
            }
            bus_client_c
                .lock()
                .await
                .unsubscribe_bulk(
                    &topics_to_unsubscribe
                        .iter()
                        .map(String::as_str)
                        .collect::<Vec<&str>>(),
                    QoS::Processed,
                )
                .await
                .log_ef();
        }
    });
    // accept new clients future
    let (tx, rx) = async_channel::bounded(2048);
    WS_STREAM_HANDLER_TX
        .set(tx)
        .map_err(|_| Error::core("WS_STREAM_HANDLER_TX already set"))?;
    tokio::spawn(async move {
        while let Ok((oid, ws_tx)) = rx.recv().await {
            let mut topics = Vec::with_capacity(2);
            push_oid_state_topic(&oid, &mut topics);
            let mut ws_streams = WS_STREAMS.lock().await;
            #[allow(clippy::mutable_key_type)]
            let clients = ws_streams.entry(oid.into()).or_default();
            clients.insert(ws_tx);
            bus_client
                .lock()
                .await
                .subscribe_bulk(
                    &topics.iter().map(String::as_str).collect::<Vec<&str>>(),
                    QoS::Processed,
                )
                .await
                .log_ef();
        }
    });
    // event loop future
    tokio::spawn(async move {
        #[derive(Deserialize, Debug)]
        struct BlobData {
            value: Value,
        }
        while let Ok(frame) = event_rx.recv().await {
            let Some(topic) = frame.topic() else {
                continue;
            };
            let oid = if let Some(topic) = topic.strip_prefix(LOCAL_STATE_TOPIC) {
                let Ok(oid) = OID::from_path(topic).log_err() else {
                    continue;
                };
                oid
            } else if let Some(topic) = topic.strip_prefix(REMOTE_STATE_TOPIC) {
                let Ok(oid) = OID::from_path(topic).log_err() else {
                    continue;
                };
                oid
            } else {
                continue;
            };
            let Ok(data) = unpack::<BlobData>(frame.payload()) else {
                continue;
            };
            let value = data.value;
            if value == Value::Unit {
                let ws_streams = WS_STREAMS.lock().await;
                let Some(clients) = ws_streams.get(&oid) else {
                    continue;
                };
                for ws_tx in clients {
                    ws_tx
                        .tx
                        .send(WsFrame::Text(
                            WsTextFrame {
                                s: Topic::Stream,
                                d: Some(Value::String("eos".to_owned())),
                            }
                            .into(),
                        ))
                        .await
                        .ok();
                }
            }
            let ws_streams = WS_STREAMS.lock().await;
            let Some(clients) = ws_streams.get(&oid) else {
                continue;
            };
            let Value::Bytes(v) = value else {
                continue;
            };
            for ws_tx in clients {
                let frame = WsFrame::new_binary(v.clone());
                if ws_tx.tx.send(frame).await.is_err() {
                    ws_tx.ask_terminate();
                }
            }
        }
    });
    Ok(())
}

pub fn init(token_header: Option<&str>) -> EResult<()> {
    let mut s = HyperJsonRpcServer::new();
    s.serve_at("/", &Method::POST);
    s.serve_at("/jrpc", &Method::POST);
    s.serve_at("/jrpc", &Method::GET);
    if let Some(th) = token_header {
        s.set_external_token_header(th);
    }
    HJRPC.set(s).map_err(|_| Error::core("HJRPC already set"))?;
    Ok(())
}
