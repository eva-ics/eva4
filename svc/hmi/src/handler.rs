use crate::aaa;
use crate::{serve, upload};
use eva_common::acl::OIDMask;
use eva_common::hyper_response;
use eva_common::hyper_tools::HResultX;
use eva_common::prelude::*;
use eva_sdk::prelude::*;
use eva_sdk::types::FullRemoteItemState;
use futures::{
    sink::SinkExt,
    stream::{SplitSink, StreamExt},
};
use hyper::{http, Body, Method, Request, Response, StatusCode};
use hyper_tungstenite::{tungstenite, HyperWebsocket, WebSocketStream};
use lazy_static::lazy_static;
use log::error;
use rjrpc::http::HyperJsonRpcServer;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use submap::SubMap;
use tungstenite::Message;
use uuid::Uuid;

err_logger!();

#[derive(Serialize, Eq, PartialEq, Copy, Clone, Hash)]
#[serde(rename_all = "lowercase")]
enum Topic {
    State,
    Log,
    Pong,
    Reload,
    Server,
}

#[derive(Serialize, Eq, PartialEq, Copy, Clone, Hash)]
#[serde(rename_all = "lowercase")]
pub enum ServerEvent {
    Restart,
}

const ERR_INVALID_IP: &str = "Invalid IP address";
const WS_QUEUE: usize = 8192;

lazy_static! {
    static ref HJRPC: HyperJsonRpcServer = {
        let mut s = HyperJsonRpcServer::new();
        s.serve_at("/", &Method::POST);
        s.serve_at("/jrpc", &Method::POST);
        s.serve_at("/jrpc", &Method::GET);
        s
    };
    pub static ref WS_SUB: Mutex<SubMap<Arc<WsTx>>> =
        Mutex::new(SubMap::new().separator('/').match_any("+").wildcard("#"));
    pub static ref WS_SUB_LOG: Mutex<SubMap<Arc<WsTx>>> =
        Mutex::new(SubMap::new().separator('/').match_any("+").wildcard("#"));
    pub static ref WS_BY_API_KEY: Mutex<HashMap<Uuid, WebSocket>> = <_>::default();
}

macro_rules! notify_ws {
    ($ws_list: expr, $frame: expr) => {
        for c in $ws_list {
            if c.tx.is_full() {
                warn!("web socket {} buffer is full, terminating", c.id);
                c.ask_terminate();
            } else if c.tx.send($frame.clone()).await.is_err() {
                c.ask_terminate();
            }
        }
    };
}

pub async fn notify_server_event(event: ServerEvent) -> EResult<()> {
    let clients = WS_SUB.lock().unwrap().list_clients();
    if !clients.is_empty() {
        let frame = WsFrame::new_server_event(event);
        notify_ws!(clients, frame);
    }
    Ok(())
}

pub async fn notify_need_reload() -> EResult<()> {
    let clients = WS_SUB.lock().unwrap().list_clients();
    if !clients.is_empty() {
        let frame = WsFrame::new_reload();
        notify_ws!(clients, frame);
    }
    Ok(())
}

pub async fn notify_state(state: FullRemoteItemState) -> EResult<()> {
    let mut clients = WS_SUB.lock().unwrap().get_subscribers(state.oid.as_path());
    clients.retain(|c| c.auth.acl().check_item_read(&state.oid));
    if !clients.is_empty() {
        let frame = WsFrame::new_state(to_value(state)?);
        notify_ws!(clients, frame);
    }
    Ok(())
}

pub async fn notify_log(topic: &str, record: Value) -> EResult<()> {
    let clients = WS_SUB_LOG.lock().unwrap().get_subscribers(topic);
    if !clients.is_empty() {
        let frame = WsFrame::new_log(record);
        notify_ws!(clients, frame);
    }
    Ok(())
}

#[derive(Serialize, Clone)]
struct WsFrame {
    s: Topic,
    #[serde(skip_serializing_if = "Option::is_none")]
    d: Option<Value>,
}

impl WsFrame {
    #[inline]
    fn new_state(d: Value) -> Self {
        Self {
            s: Topic::State,
            d: Some(d),
        }
    }
    #[inline]
    fn new_log(d: Value) -> Self {
        Self {
            s: Topic::Log,
            d: Some(d),
        }
    }
    #[inline]
    fn is_data(&self) -> bool {
        self.s == Topic::State
    }
    #[inline]
    fn new_pong() -> Self {
        Self {
            s: Topic::Pong,
            d: None,
        }
    }
    #[inline]
    fn new_reload() -> Self {
        Self {
            s: Topic::Reload,
            d: Some(Value::String("asap".to_owned())),
        }
    }
    #[inline]
    fn new_server_event(event: ServerEvent) -> Self {
        Self {
            s: Topic::Server,
            d: Some(to_value(event).unwrap()),
        }
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
}

impl WsTx {
    fn ask_terminate(&self) {
        if let Some(token) = self.auth.token() {
            token.unregister_websocket(self.id);
        }
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
    if let Some(bttl) = buf_ttl {
        let mut event_buf: HashMap<Topic, Vec<Value>> = HashMap::new();
        let mut buf_interval = tokio::time::interval(Duration::from_secs_f64(bttl));
        loop {
            tokio::select! {
            f = rx.recv() => {
                if let Ok(frame) = f {
                    if frame.is_data() {
                        if let Some(data) = frame.d {
                            if let Some(v) = event_buf.get_mut(&frame.s) {
                                v.push(data);
                            } else {
                                event_buf.insert(frame.s, vec![data]);
                            }
                        }
                    } else {
                        send!(&frame);
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
            send!(&frame);
        }
    }
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

async fn serve_websocket(
    ws_tx: Arc<WsTx>,
    rx: async_channel::Receiver<WsFrame>,
    websocket: HyperWebsocket,
    buf_ttl: Option<f64>,
) -> EResult<()> {
    let websocket = websocket.await.map_err(Error::io)?;
    let (sender, mut receiver) = websocket.split();
    let ws_tx_c = ws_tx.clone();
    let sender_fut = tokio::spawn(async move {
        serve_websocket_sender(ws_tx_c, sender, rx, buf_ttl).await;
    });
    while let Some(message) = receiver.next().await {
        #[allow(clippy::single_match)]
        match message.map_err(Error::io)? {
            tungstenite::Message::Text(msg) => {
                if !msg.is_empty() {
                    let cmd: WsCommand = serde_json::from_str(&msg).log_err()?;
                    let method = cmd.method.as_str();
                    trace!("web socket {} {}", ws_tx.id, method);
                    match method {
                        "subscribe.state" => {
                            if let Some(p) = cmd.params {
                                let masks: Vec<OIDMask> = Vec::deserialize(p).log_err()?;
                                let mut map = WS_SUB.lock().unwrap();
                                for mask in masks {
                                    map.subscribe(&mask.as_path(), &ws_tx);
                                }
                            }
                        }
                        "subscribe.log" => {
                            if let Some(p) = cmd.params {
                                let level: u8 = u8::deserialize(p).log_err()?;
                                let mut map = WS_SUB_LOG.lock().unwrap();
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
        WS_SUB.lock().unwrap().unregister_client(&self.ws_tx);
        WS_SUB_LOG.lock().unwrap().unregister_client(&self.ws_tx);
    }
}

#[inline]
fn get_real_ip(
    ip: IpAddr,
    headers: &hyper::header::HeaderMap,
    real_ip_header: Arc<Option<String>>,
) -> Result<IpAddr, Box<dyn std::error::Error>> {
    if let Some(ref ip_header) = *real_ip_header {
        if let Some(s) = headers.get(ip_header) {
            return Ok(s.to_str()?.parse::<IpAddr>()?);
        }
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
        Ok(resp) => {
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
    if req.uri().path() == "/ws" && hyper_tungstenite::is_upgrade_request(&req) {
        let params: Option<HashMap<String, String>> = req.uri().query().map(|v| {
            url::form_urlencoded::parse(v.as_bytes())
                .into_owned()
                .collect()
        });
        if let Some(p) = params {
            if let Some(k) = p.get("k") {
                match crate::aaa::authenticate(k, Some(ip)).await {
                    Ok(auth) => {
                        if let Some(token) = auth.clone_token() {
                            let (response, websocket) = match hyper_tungstenite::upgrade(req, None)
                            {
                                Ok(v) => v,
                                Err(e) => {
                                    return hyper_response!(StatusCode::BAD_REQUEST, e.to_string());
                                }
                            };
                            let buf_ttl = if let Some(b) = p.get("buf_ttl") {
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
                            let ws_id = Uuid::new_v4();
                            let (tx, rx) = async_channel::bounded::<WsFrame>(WS_QUEUE);
                            let ws_tx = Arc::new(WsTx {
                                id: ws_id,
                                tx,
                                auth,
                            });
                            let ws_tx_c = ws_tx.clone();
                            WS_SUB.lock().unwrap().register_client(&ws_tx);
                            if ws_tx.auth.acl().check_op(eva_common::acl::Op::Log) {
                                WS_SUB_LOG.lock().unwrap().register_client(&ws_tx);
                            }
                            let ws_handler = tokio::spawn(async move {
                                if let Err(e) =
                                    serve_websocket(ws_tx_c.clone(), rx, websocket, buf_ttl).await
                                {
                                    error!("error in websocket connection: {}", e);
                                }
                                ws_tx_c.ask_terminate();
                            });
                            let ws = WebSocket {
                                id: ws_id,
                                handler: ws_handler,
                                ws_tx,
                            };
                            token.register_websocket(ws);
                            return Ok(response);
                        }
                        return hyper_response!(StatusCode::FORBIDDEN, "auth token required");
                    }
                    Err(e) => {
                        return hyper_response!(StatusCode::FORBIDDEN, e.to_string());
                    }
                }
            }
        }
        return hyper_response!(StatusCode::FORBIDDEN);
    }
    let (parts, body) = req.into_parts();
    if HJRPC.matches(&parts) {
        HJRPC.process(crate::api::processor, &parts, body, ip).await
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
                if uri == "/favicon.ico" {
                    if let Some(ui_path) = crate::UI_PATH.get().unwrap() {
                        return serve::file(
                            uri,
                            ui_path,
                            &uri[1..],
                            None,
                            false,
                            &parts.headers,
                            ip,
                            serve::TplDirKind::No,
                        )
                        .await
                        .log_err()
                        .into_hyper_response();
                    }
                } else if uri.starts_with("/.evahi/") {
                    if let Some(ui_path) = crate::UI_PATH.get().unwrap() {
                        return serve::file(
                            uri,
                            ui_path,
                            &uri[1..],
                            None,
                            false,
                            &parts.headers,
                            ip,
                            serve::TplDirKind::No,
                        )
                        .await
                        .log_err()
                        .into_hyper_response();
                    }
                }
                let params: Option<HashMap<String, String>> = parts.uri.query().map(|v| {
                    url::form_urlencoded::parse(v.as_bytes())
                        .into_owned()
                        .collect()
                });
                if let Some(ui_file) = uri.strip_prefix("/ui/") {
                    if let Some(ui_path) = crate::UI_PATH.get().unwrap() {
                        return serve::file(
                            uri,
                            ui_path,
                            ui_file,
                            params.as_ref(),
                            true,
                            &parts.headers,
                            ip,
                            serve::TplDirKind::Ui,
                        )
                        .await
                        .log_err()
                        .into_hyper_response();
                    }
                }
                if uri == "/pvt" {
                    if let Some(ref q) = params {
                        if let Some(f) = q.get("f") {
                            return serve::pvt(uri, f, params.as_ref(), &parts.headers, ip)
                                .await
                                .log_err()
                                .into_hyper_response();
                        }
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
                    if let Some(ref q) = params {
                        if let Some(f) = q.get("f") {
                            return serve::remote_pvt(uri, f, params.as_ref(), &parts.headers, ip)
                                .await
                                .log_err()
                                .into_hyper_response();
                        }
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
                if uri == "/upload" {
                    if let Some(boundary) = parts
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
                }
                hyper_response!(StatusCode::NOT_FOUND)
            }
            _ => hyper_response!(StatusCode::METHOD_NOT_ALLOWED),
        }
    }
}
