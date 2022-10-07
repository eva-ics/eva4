use crate::db;
use crate::handler::WebSocket;
use eva_common::acl::Acl;
use eva_common::err_logger;
use eva_common::{EResult, Error};
use genpass_native::random_string;
use lazy_static::lazy_static;
use log::error;
use log::{debug, trace};
use once_cell::sync::OnceCell;
use serde::{ser::SerializeMap, Serialize, Serializer};
use std::collections::{HashMap, HashSet};
use std::convert::{TryFrom, TryInto};
use std::fmt;
use std::future::Future;
use std::net::IpAddr;
use std::sync::atomic;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use uuid::Uuid;

err_logger!();

static SESSION_PROLONG: atomic::AtomicBool = atomic::AtomicBool::new(true);
static SESSION_STICK_IP: atomic::AtomicBool = atomic::AtomicBool::new(true);
static SESSION_ALLOW_LIST_NEIGHBORS: atomic::AtomicBool = atomic::AtomicBool::new(false);

const TOKEN_MODE_NORMAL: u8 = 1;
const TOKEN_MODE_READONLY: u8 = 2;

pub const SESSION_CLEANUP_INTERVAL: Duration = Duration::from_secs(15);
pub const TOKEN_WEBSOCKETS_CLEANUP_INTERVAL: Duration = Duration::from_secs(5);

const ERR_INVALID_TOKEN: &str = "invalid token";
const ERR_INVALID_TOKEN_IP: &str = "token access denied";
const ERR_INVALID_TOKEN_FORMAT: &str = "invalid token format";
const ERR_SESSIONS_DISABLED: &str = "sessions are disabled";

lazy_static! {
    static ref SESSION_TIMEOUT: OnceCell<Duration> = <_>::default();
    static ref TOKEN_WEBSOCKETS: Mutex<HashMap<TokenId, HashMap<Uuid, WebSocket>>> = <_>::default();
}

pub fn parse_auth(
    params: Option<&HashMap<String, String>>,
    headers: &hyper::HeaderMap,
) -> Option<String> {
    let mut auth = if let Some(q) = params {
        q.get("k").map(ToOwned::to_owned)
    } else {
        None
    };
    if auth.is_none() {
        if let Some(ch) = headers.get(hyper::header::COOKIE) {
            for c in ch.to_str().unwrap_or_default().split(';') {
                if let Ok(cookie) = cookie::Cookie::parse(c) {
                    if cookie.name() == "auth" {
                        auth = Some(cookie.value().to_owned());
                        break;
                    }
                }
            }
        }
    }
    auth
}

pub fn set_session_config(
    timeout: Option<f64>,
    prolong: bool,
    stick_ip: bool,
    allow_list_neighbors: bool,
) {
    debug!("session.timeout: {:?}", timeout);
    if let Some(timeout) = timeout {
        SESSION_TIMEOUT
            .set(Duration::from_secs_f64(timeout))
            .expect("unable to set session timeout");
    }
    debug!("session.prolong: {}", prolong);
    SESSION_PROLONG.store(prolong, atomic::Ordering::SeqCst);
    debug!("session.stick_ip: {}", stick_ip);
    SESSION_ALLOW_LIST_NEIGHBORS.store(allow_list_neighbors, atomic::Ordering::SeqCst);
    debug!("session.allow_list_neighbors: {}", allow_list_neighbors);
    SESSION_STICK_IP.store(stick_ip, atomic::Ordering::SeqCst);
}

#[derive(Debug, Clone)]
pub enum Auth {
    Token(Arc<Token>),
    Key(String, Arc<Acl>),
}

impl std::fmt::Display for Auth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Auth::Token(ref token) => write!(f, "user:{}", token.user()),
            Auth::Key(ref key_id, _) => write!(f, "key:{}", key_id),
        }
    }
}

impl Auth {
    #[inline]
    pub fn acl(&self) -> &Acl {
        match self {
            Auth::Token(token) => token.acl(),
            Auth::Key(_, acl) => acl,
        }
    }
    #[inline]
    pub fn as_str(&self) -> &str {
        match self {
            Auth::Token(_) => "token",
            Auth::Key(_, _) => "key",
        }
    }
    #[inline]
    pub fn token(&self) -> Option<&Token> {
        match self {
            Auth::Token(ref token) => Some(token),
            Auth::Key(_, _) => None,
        }
    }
    #[inline]
    pub fn clone_token(&self) -> Option<Arc<Token>> {
        match self {
            Auth::Token(token) => Some(token.clone()),
            Auth::Key(_, _) => None,
        }
    }
    #[inline]
    pub fn user(&self) -> Option<&str> {
        match self {
            Auth::Token(ref token) => Some(token.user()),
            Auth::Key(_, _) => None,
        }
    }
}

#[derive(Debug, Ord, PartialOrd, Eq, PartialEq, Clone, Hash)]
pub struct TokenId {
    u: Uuid,
    salt: String,
}

impl TokenId {
    fn create() -> EResult<Self> {
        Ok(Self {
            u: Uuid::new_v4(),
            salt: random_string(16)?,
        })
    }
    #[inline]
    pub fn to_short_string(&self) -> String {
        format!("{}{}", hex::encode(self.u.as_bytes()), self.salt)
    }
}

pub async fn list_neighbors() -> EResult<Vec<Token>> {
    if SESSION_ALLOW_LIST_NEIGHBORS.load(atomic::Ordering::SeqCst) {
        get_tokens().await
    } else {
        Err(Error::access("the method is not allowed to be called"))
    }
}

#[inline]
pub fn get_tokens() -> impl Future<Output = EResult<Vec<Token>>> {
    db::load_all_tokens()
}

// &str used in auth only, supplied token id
impl TryFrom<&str> for TokenId {
    type Error = Error;
    #[inline]
    fn try_from(s: &str) -> EResult<TokenId> {
        if s.len() < 48 {
            Err(Error::invalid_data(ERR_INVALID_TOKEN_FORMAT))
        } else {
            let u = Uuid::from_slice(&hex::decode(&s[..32])?)?;
            let salt = s[32..].to_owned();
            Ok(TokenId { u, salt })
        }
    }
}

// used to delete / modify token, supplied token:token_id
impl TryFrom<String> for TokenId {
    type Error = Error;
    fn try_from(v: String) -> EResult<TokenId> {
        if let Some(stripped) = v.strip_prefix("token:") {
            Ok(stripped
                .try_into()
                .map_err(|_| Error::invalid_data(ERR_INVALID_TOKEN_FORMAT))?)
        } else {
            Err(Error::invalid_data(ERR_INVALID_TOKEN_FORMAT))
        }
    }
}

impl fmt::Display for TokenId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "token:{}{}", hex::encode(self.u.as_bytes()), self.salt)
    }
}

pub async fn get_token(token_id: TokenId, ip: Option<IpAddr>) -> EResult<Arc<Token>> {
    trace!("authenticating token {}", token_id);
    if let Some(mut token) = db::load_token(token_id).await? {
        if SESSION_STICK_IP.load(atomic::Ordering::SeqCst) && token.ip != ip {
            Err(Error::access(ERR_INVALID_TOKEN_IP))
        } else if token.is_expired()? {
            Err(Error::access(ERR_INVALID_TOKEN))
        } else {
            token.touch().await?;
            Ok(Arc::new(token))
        }
    } else {
        Err(Error::access(ERR_INVALID_TOKEN))
    }
}

#[inline]
pub fn get_expiration() -> EResult<Duration> {
    if let Some(timeout) = SESSION_TIMEOUT.get() {
        Ok(*timeout)
    } else {
        Err(Error::access(ERR_SESSIONS_DISABLED))
    }
}

#[inline]
pub fn sessions_enabled() -> bool {
    SESSION_TIMEOUT.get().is_some()
}

pub async fn create_token(
    user: &str,
    acl: Acl,
    auth_svc: &str,
    ip: Option<IpAddr>,
) -> EResult<Arc<Token>> {
    if sessions_enabled() {
        trace!("creating token for {}", user);
        let token_id = TokenId::create()?;
        let token = Arc::new(Token::new(
            token_id.clone(),
            user.to_owned(),
            acl,
            auth_svc.to_owned(),
            ip,
            None,
            None,
        ));
        db::save_token(token.as_ref()).await?;
        Ok(token)
    } else {
        Err(Error::access(ERR_SESSIONS_DISABLED))
    }
}

pub async fn destroy_token(token_id: &TokenId) -> EResult<()> {
    trace!("destroying token {}", token_id);
    db::delete_token(token_id).await?;
    let websockets = TOKEN_WEBSOCKETS.lock().unwrap().remove(token_id);
    if let Some(sockets) = websockets {
        for ws in sockets.values() {
            ws.terminate();
        }
    }
    Ok(())
}

#[allow(clippy::mutex_atomic)]
#[allow(dead_code)]
#[derive(Debug)]
pub struct Token {
    id: TokenId,
    mode: atomic::AtomicU8,
    t: i64,
    user: String,
    acl: Acl,
    auth_svc: String,
    ip: Option<IpAddr>,
}

impl Serialize for Token {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut map = serializer.serialize_map(Some(4))?;
        map.serialize_entry("acl", self.acl.id())?;
        map.serialize_entry("token", &self.id.to_string())?;
        map.serialize_entry("mode", &self.mode_as_str())?;
        map.serialize_entry("user", &self.user)?;
        map.end()
    }
}

impl Token {
    // keep active as mutex as it is required to lock ws/task ops
    #[allow(clippy::mutex_atomic)]
    pub fn new(
        id: TokenId,
        user: String,
        acl: Acl,
        auth_svc: String,
        ip: Option<IpAddr>,
        mode: Option<u8>,
        creation_time: Option<i64>,
    ) -> Self {
        Self {
            id,
            mode: atomic::AtomicU8::new(mode.unwrap_or(TOKEN_MODE_NORMAL)),
            t: creation_time
                .unwrap_or_else(|| i64::try_from(eva_common::time::now()).unwrap_or_default()),
            user,
            acl,
            auth_svc,
            ip,
        }
    }
    #[inline]
    pub fn mode_as_str(&self) -> &str {
        if self.mode.load(atomic::Ordering::SeqCst) == TOKEN_MODE_READONLY {
            "readonly"
        } else {
            "normal"
        }
    }
    pub fn time(&self) -> i64 {
        self.t
    }
    #[inline]
    pub fn mode(&self) -> u8 {
        self.mode.load(atomic::Ordering::SeqCst)
    }
    #[inline]
    async fn touch(&mut self) -> EResult<()> {
        if SESSION_PROLONG.load(atomic::Ordering::SeqCst) {
            let now = i64::try_from(eva_common::time::now()).unwrap_or_default();
            self.t = now;
            db::set_token_time(&self.id, self.t).await?;
        }
        Ok(())
    }
    #[inline]
    pub fn is_expired(&self) -> EResult<bool> {
        let expires_at =
            self.t + i64::try_from(get_expiration()?.as_secs()).map_err(Error::failed)?;
        let now = i64::try_from(eva_common::time::now()).map_err(Error::failed)?;
        Ok(expires_at < now)
    }
    #[inline]
    pub fn expires_in(&self) -> EResult<Duration> {
        let expires_at =
            self.t + i64::try_from(get_expiration()?.as_secs()).map_err(Error::failed)?;
        let now = i64::try_from(eva_common::time::now()).map_err(Error::failed)?;
        Ok(Duration::from_secs(
            u64::try_from(expires_at - now).unwrap_or_default(),
        ))
    }
    #[inline]
    pub fn is_readonly(&self) -> bool {
        self.mode() == TOKEN_MODE_READONLY
    }
    #[inline]
    pub fn id(&self) -> &TokenId {
        &self.id
    }
    #[inline]
    pub fn acl(&self) -> &Acl {
        &self.acl
    }
    #[inline]
    pub fn user(&self) -> &str {
        &self.user
    }
    #[inline]
    pub fn auth_svc(&self) -> &str {
        &self.auth_svc
    }
    #[inline]
    pub fn ip(&self) -> Option<&IpAddr> {
        self.ip.as_ref()
    }
    #[inline]
    pub fn set_normal(&self) -> impl Future<Output = EResult<()>> + '_ {
        self.mode.store(TOKEN_MODE_NORMAL, atomic::Ordering::SeqCst);
        db::set_token_mode(&self.id, TOKEN_MODE_NORMAL)
    }
    #[inline]
    pub fn set_readonly(&self) -> impl Future<Output = EResult<()>> + '_ {
        self.mode
            .store(TOKEN_MODE_READONLY, atomic::Ordering::SeqCst);
        db::set_token_mode(&self.id, TOKEN_MODE_READONLY)
    }
    #[allow(clippy::mutex_atomic)]
    pub fn register_websocket(&self, ws: WebSocket) {
        let mut websockets = TOKEN_WEBSOCKETS.lock().unwrap();
        if let Some(map) = websockets.get_mut(&self.id) {
            map.insert(ws.id(), ws);
        } else {
            let mut map = HashMap::new();
            map.insert(ws.id(), ws);
            websockets.insert(self.id.clone(), map);
        }
    }
    pub fn unregister_websocket(&self, id: Uuid) {
        if let Some(websockets) = TOKEN_WEBSOCKETS.lock().unwrap().get_mut(&self.id) {
            if let Some(ws) = websockets.get(&id) {
                ws.terminate();
            }
        }
    }
}

async fn clear_tokens() {
    debug!("cleaning idle API sessions");
    db::clear_idle_tokens(get_expiration().unwrap())
        .await
        .log_ef();
    db::clear_token_acls().await.log_ef();
}

async fn clear_token_websockets() {
    debug!("cleaning websockets for dropped tokens");
    match db::load_token_ids().await {
        Ok(ws_ids) => {
            let ids: HashSet<TokenId> = ws_ids.into_iter().collect();
            let mut websockets = TOKEN_WEBSOCKETS.lock().unwrap();
            let mut to_remove = Vec::new();
            for token_id in websockets.keys() {
                if !ids.contains(token_id) {
                    to_remove.push(token_id.clone());
                }
            }
            for token_id in to_remove {
                if let Some(sockets) = websockets.remove(&token_id) {
                    for ws in sockets.values() {
                        ws.terminate();
                    }
                }
            }
        }
        Err(e) => error!("{}", e),
    }
}

pub fn clear_tokens_by_user(user: &str) {
    let user = user.to_owned();
    // spawn in bg to unblock bus frame handler
    tokio::spawn(async move {
        db::clear_tokens_by_user(&user).await.log_ef();
    });
}

pub fn clear_tokens_by_acl_id(acl_id: &str) {
    let acl_id = acl_id.to_owned();
    // spawn in bg to unblock bus frame handler
    tokio::spawn(async move {
        db::clear_tokens_by_acl_id(&acl_id).await.log_ef();
    });
}

pub async fn start() -> EResult<()> {
    if sessions_enabled() {
        eva_common::cleaner!("tokens", clear_tokens, SESSION_CLEANUP_INTERVAL);
        eva_common::cleaner!(
            "token_websockets",
            clear_token_websockets,
            TOKEN_WEBSOCKETS_CLEANUP_INTERVAL
        );
    }
    Ok(())
}
