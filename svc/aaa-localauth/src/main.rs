use bmart::tools::Sorting;
use busrt::QoS;
use eva_common::common_payloads::{ParamsId, ParamsIdListOwned};
use eva_common::events::{AAA_KEY_TOPIC, AAA_USER_TOPIC};
use eva_common::op::Op;
use eva_common::prelude::*;
use eva_sdk::prelude::*;
use genpass_native::{random_string, Password};
use once_cell::sync::{Lazy, OnceCell};
use serde::{Deserialize, Serialize};
use std::collections::{hash_map, HashMap, HashSet};
use std::sync::Arc;
use std::sync::Mutex;
use std::time::{Duration, Instant};

err_logger!();

#[cfg(not(feature = "std-alloc"))]
#[global_allocator]
static ALLOC: jemallocator::Jemalloc = jemallocator::Jemalloc;

const AUTHOR: &str = "Bohemia Automation";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const DESCRIPTION: &str = "Local auth service";

const ERR_INVALID_LOGIN_PASS: &str = "invalid login/password";
const ONE_TIME_USER_CLEANUP_INTERVAL: Duration = Duration::from_secs(60);
const ONE_TIME_USER_PREFIX: &str = "OT.";

pub const ID_ALLOWED_SYMBOLS: &str = "_.()[]-\\";

static KEYDB: Lazy<Mutex<KeyDb>> = Lazy::new(<_>::default);
static USERS: Lazy<tokio::sync::Mutex<HashMap<String, User>>> = Lazy::new(<_>::default);
static ONE_TIME_USERS: Lazy<tokio::sync::Mutex<HashMap<String, OneTimeUser>>> =
    Lazy::new(<_>::default);
static ONE_TIME_EXPIRES: OnceCell<Duration> = OnceCell::new();
static REG: OnceCell<Registry> = OnceCell::new();
static RPC: OnceCell<Arc<RpcClient>> = OnceCell::new();
static TIMEOUT: OnceCell<Duration> = OnceCell::new();

#[derive(Deserialize, Default)]
struct PasswordPolicy {
    #[serde(default)]
    min_length: usize,
    #[serde(default)]
    required_letter: bool,
    #[serde(default)]
    required_mixed_case: bool,
    #[serde(default)]
    required_number: bool,
}

#[derive(Deserialize, Copy, Clone)]
#[serde(rename_all = "snake_case")]
enum ProfileField {
    Email,
}

struct OneTimeUser {
    created: Instant,
    user: Option<User>,
}

impl OneTimeUser {
    async fn create(login: &str, acls: HashSet<String>) -> EResult<(Self, String)> {
        let password_plain = random_string(16)?;
        Ok((
            Self {
                created: Instant::now(),
                user: Some(User {
                    login: login.to_owned(),
                    password: Password::new_sha256(&password_plain).await,
                    acls,
                    profile: UserProfile::default(),
                }),
            },
            password_plain,
        ))
    }
    async fn get_acls(&mut self, password: &str) -> EResult<Vec<String>> {
        if let Some(user) = self.user.take() {
            if user.password.verify(password).await? {
                Ok(user.acls.into_iter().collect())
            } else {
                debug!(
                    "one-time user {} authentication failed: invalid password",
                    user.login
                );
                Err(Error::access(ERR_INVALID_LOGIN_PASS))
            }
        } else {
            Err(Error::access("one-time account can not be used twice"))
        }
    }
    #[inline]
    fn is_valid(&self) -> bool {
        self.created.elapsed() < *ONE_TIME_EXPIRES.get().unwrap() && self.user.is_some()
    }
}

/// full user object, stored in the registry
#[derive(Debug, Serialize, Deserialize, Sorting)]
#[sorting(id = "login")]
#[serde(deny_unknown_fields)]
struct User {
    login: String,
    password: Password,
    #[serde(default)]
    acls: HashSet<String>,
    #[serde(default)]
    profile: UserProfile,
}

impl User {
    fn to_info(&self, full: bool, with_password: bool) -> UserInfo<'_> {
        UserInfo {
            login: &self.login,
            password: if with_password {
                Some(self.password.to_string())
            } else {
                None
            },
            acls: &self.acls,
            profile: if full { Some(&self.profile) } else { None },
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct UserProfile {
    email: Option<String>,
}

impl UserProfile {
    fn get_field(&self, field: ProfileField) -> Option<Value> {
        match field {
            ProfileField::Email => self.email.as_ref().map(|v| Value::String(v.clone())),
        }
    }
    fn set_field(&mut self, field: ProfileField, value: Value) {
        match field {
            ProfileField::Email => {
                if value == Value::Unit {
                    self.email.take();
                } else {
                    self.email.replace(value.to_string());
                }
            }
        }
    }
}

/// user info object, provided on list
#[derive(Serialize, Sorting)]
#[sorting(id = "login")]
struct UserInfo<'a> {
    login: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    password: Option<String>,
    acls: &'a HashSet<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    profile: Option<&'a UserProfile>,
}

/// full key object, stored in the registry
#[derive(Debug, Serialize, Deserialize, Sorting)]
#[serde(deny_unknown_fields)]
struct Key {
    id: String,
    key: String,
    #[serde(default)]
    acls: HashSet<String>,
}

async fn clear_one_time_users() {
    ONE_TIME_USERS
        .lock()
        .await
        .retain(|_, user| user.is_valid());
}

impl Key {
    fn regenerate(&self) -> EResult<Self> {
        Ok(Self {
            id: self.id.clone(),
            key: random_string(32)?,
            acls: self.acls.clone(),
        })
    }
}

/// key info object, provided on list
#[derive(Serialize, Sorting)]
struct KeyInfo<'a> {
    id: &'a str,
    key: &'a str,
    acls: &'a HashSet<String>,
}

impl<'a> From<&'a Arc<Key>> for KeyInfo<'a> {
    fn from(key: &'a Arc<Key>) -> KeyInfo<'a> {
        KeyInfo {
            id: &key.id,
            key: &key.key,
            acls: &key.acls,
        }
    }
}

/// key value object
#[derive(Serialize, Sorting)]
struct KeyValue<'a> {
    id: &'a str,
    key: &'a str,
}

impl<'a> From<&'a Arc<Key>> for KeyValue<'a> {
    fn from(key: &'a Arc<Key>) -> KeyValue<'a> {
        KeyValue {
            id: &key.id,
            key: &key.key,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ParamsIdPass<'a> {
    #[serde(borrow)]
    i: &'a str,
    #[serde(borrow)]
    password: &'a str,
    #[serde(default)]
    check_policy: bool,
}

#[derive(Default)]
struct KeyDb {
    keys_by_id: HashMap<String, Arc<Key>>,
    keys_by_k: HashMap<String, Arc<Key>>,
}

impl KeyDb {
    #[inline]
    fn values(&self) -> std::collections::hash_map::Values<String, Arc<Key>> {
        self.keys_by_id.values()
    }
    fn append(&mut self, key: Arc<Key>) -> EResult<()> {
        if let Some(k) = self.keys_by_k.get(&key.key) {
            if key.id != k.id {
                return Err(Error::busy(format!(
                    "API key {} has the same value as the existing: {}",
                    key.id, k.id
                )));
            }
        }
        self.keys_by_id.insert(key.id.clone(), key.clone());
        self.keys_by_k.insert(key.key.clone(), key);
        Ok(())
    }
    #[inline]
    fn get(&self, id: &str) -> Option<&Arc<Key>> {
        self.keys_by_id.get(id)
    }
    #[inline]
    fn get_by_k(&self, k: &str) -> Option<&Arc<Key>> {
        self.keys_by_k.get(k)
    }
    #[inline]
    fn remove(&mut self, id: &str) -> Option<Arc<Key>> {
        if let Some(key) = self.keys_by_id.remove(id) {
            self.keys_by_k.remove(&key.key);
            Some(key)
        } else {
            None
        }
    }
}

struct Handlers {
    info: ServiceInfo,
    password_policy: PasswordPolicy,
    acl_svc: String,
    otp_svc: Option<String>,
    tx: async_channel::Sender<(String, Option<Value>)>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum KeyOrId {
    Key(Key),
    Id(String),
}
impl KeyOrId {
    fn as_str(&self) -> &str {
        match self {
            Self::Key(key) => key.id.as_str(),
            Self::Id(id) => id.as_str(),
        }
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum UserOrLogin {
    User(User),
    Login(String),
}
impl UserOrLogin {
    fn as_str(&self) -> &str {
        match self {
            Self::User(user) => user.login.as_str(),
            Self::Login(login) => login.as_str(),
        }
    }
}

#[inline]
fn get_timeout() -> Duration {
    *TIMEOUT.get().unwrap()
}

#[async_trait::async_trait]
impl RpcHandlers for Handlers {
    #[allow(clippy::too_many_lines)]
    async fn handle_call(&self, event: RpcEvent) -> RpcResult {
        svc_rpc_need_ready!();
        let method = event.parse_method()?;
        let payload = event.payload();
        match method {
            "key.list" => {
                if payload.is_empty() {
                    let keydb = KEYDB.lock().unwrap();
                    let mut key_info: Vec<KeyInfo> = keydb.values().map(Into::into).collect();
                    key_info.sort();
                    Ok(Some(pack(&key_info)?))
                } else {
                    Err(RpcError::params(None))
                }
            }
            "key.deploy" => {
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    #[derive(Deserialize)]
                    #[serde(deny_unknown_fields)]
                    struct Keys {
                        keys: Vec<Key>,
                    }
                    let new_keys: Keys = unpack(payload)?;
                    let mut reg_keys = Vec::new();
                    {
                        let mut keydb = KEYDB.lock().unwrap();
                        for key in &new_keys.keys {
                            for c in key.id.chars() {
                                if !c.is_alphanumeric()
                                    && !ID_ALLOWED_SYMBOLS.contains(c)
                                    && c != '/'
                                {
                                    return Err(Error::invalid_data(format!(
                                        "invalid symbol in key id {}: {}",
                                        key.id, c
                                    ))
                                    .into());
                                }
                            }
                        }
                        for key in new_keys.keys {
                            let key_id = key.id.clone();
                            keydb.remove(&key_id);
                            reg_keys.push((key_id, to_value(&key)?));
                            keydb.append(Arc::new(key))?;
                        }
                    }
                    let reg = REG.get().unwrap();
                    for (key, val) in reg_keys {
                        warn!("API key created: {}", key);
                        reg.key_set(&format!("key/{key}"), val.clone()).await?;
                        self.tx
                            .send((format!("{AAA_KEY_TOPIC}{key}"), Some(val)))
                            .await
                            .log_ef();
                    }
                    Ok(None)
                }
            }
            "key.undeploy" => {
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    #[derive(Deserialize)]
                    #[serde(deny_unknown_fields)]
                    struct Keys {
                        keys: Vec<KeyOrId>,
                    }
                    let key_ids: Keys = unpack(payload)?;
                    let reg = REG.get().unwrap();
                    let mut ids = Vec::new();
                    {
                        let mut keydb = KEYDB.lock().unwrap();
                        for key_id in key_ids.keys {
                            if let Some(key) = keydb.remove(key_id.as_str()) {
                                warn!("API key destroyed: {}", key.id);
                                ids.push(key.id.clone());
                            }
                        }
                    }
                    for id in ids {
                        reg.key_delete(&format!("key/{id}")).await?;
                        self.tx
                            .send((format!("{AAA_KEY_TOPIC}{id}"), None))
                            .await
                            .log_ef();
                    }
                    Ok(None)
                }
            }
            "key.get_config" => {
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    let p: ParamsId = unpack(payload)?;
                    if let Some(key) = KEYDB.lock().unwrap().get(p.i) {
                        Ok(Some(pack(key)?))
                    } else {
                        Err(Error::not_found(format!("API key {} not found", p.i)).into())
                    }
                }
            }
            "key.export" => {
                #[derive(Serialize)]
                #[serde(deny_unknown_fields)]
                struct Keys<'a> {
                    keys: Vec<&'a Key>,
                }
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    let p: ParamsId = unpack(payload)?;
                    let mut configs = Vec::new();
                    let keys = KEYDB.lock().unwrap();
                    if p.i == "#" || p.i == "*" {
                        for key in keys.values() {
                            configs.push(key.as_ref());
                        }
                    } else if let Some(key) = keys.get(p.i) {
                        configs.push(key.as_ref());
                    } else {
                        return Err(Error::not_found(format!("API key {} not found", p.i)).into());
                    }
                    configs.sort();
                    Ok(Some(pack(&Keys { keys: configs })?))
                }
            }
            "key.get" => {
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    let p: ParamsId = unpack(payload)?;
                    if let Some(key) = KEYDB.lock().unwrap().get(p.i) {
                        Ok(Some(pack(&Into::<KeyValue>::into(key))?))
                    } else {
                        Err(Error::not_found(format!("API key {} not found", p.i)).into())
                    }
                }
            }
            "key.destroy" => {
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    let p: ParamsId = unpack(payload)?;
                    let reg = REG.get().unwrap();
                    if KEYDB.lock().unwrap().remove(p.i).is_some() {
                        reg.key_delete(&format!("key/{}", p.i)).await?;
                        warn!("API key destroyed: {}", p.i);
                        self.tx
                            .send((format!("{AAA_KEY_TOPIC}{}", p.i), None))
                            .await
                            .log_ef();
                        Ok(None)
                    } else {
                        Err(Error::not_found(format!("API key {} not found", p.i)).into())
                    }
                }
            }
            "key.regenerate" => {
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    let p: ParamsId = unpack(payload)?;
                    let reg = REG.get().unwrap();
                    let val = {
                        let mut keydb = KEYDB.lock().unwrap();
                        if let Some(key) = keydb.remove(p.i) {
                            let new_key = key.regenerate()?;
                            let v = to_value(&new_key)?;
                            keydb.append(Arc::new(new_key))?;
                            v
                        } else {
                            return Err(
                                Error::not_found(format!("API key {} not found", p.i)).into()
                            );
                        }
                    };
                    info!("API key regenerated: {}", p.i);
                    reg.key_set(&format!("key/{}", p.i), val.clone()).await?;
                    let result = pack(&val)?;
                    self.tx
                        .send((format!("{AAA_KEY_TOPIC}{}", p.i), Some(val)))
                        .await
                        .log_ef();
                    Ok(Some(result))
                }
            }
            "auth.key" => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct ParamsAuthKey<'a> {
                    #[serde(borrow)]
                    key: &'a str,
                    #[serde(
                        default = "get_timeout",
                        deserialize_with = "eva_common::tools::de_float_as_duration"
                    )]
                    timeout: Duration,
                }
                #[derive(Serialize)]
                struct Payload<'a> {
                    i: Vec<String>,
                    key_id: &'a str,
                }
                if payload.is_empty() {
                    return Err(RpcError::params(None));
                }
                let p: ParamsAuthKey = unpack(payload)?;
                let (key, acls) = if let Some(key) = KEYDB.lock().unwrap().get_by_k(p.key) {
                    debug!("API key authenticated: {}", key.id);
                    (key.clone(), key.acls.iter().cloned().collect())
                } else {
                    debug!("API key authentication failed");
                    return Err(Error::access("invalid API key").into());
                };
                let result = safe_rpc_call(
                    RPC.get().unwrap(),
                    &self.acl_svc,
                    "acl.format",
                    pack(&Payload {
                        i: acls,
                        key_id: &key.id,
                    })?
                    .into(),
                    QoS::Processed,
                    p.timeout,
                )
                .await?;
                Ok(Some(result.payload().to_vec()))
            }
            "password.hash" => {
                #[derive(Deserialize)]
                #[serde(rename_all = "lowercase")]
                enum HashAlgo {
                    Sha256,
                    Sha512,
                    Pbkdf2,
                }
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct ParamsPasswordHash {
                    password: Value,
                    algo: HashAlgo,
                }
                #[derive(Serialize)]
                struct PayloadPasswordHash {
                    hash: String,
                }
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    let p: ParamsPasswordHash = unpack(payload)?;
                    let plain_password = p.password.to_string();
                    let password = match p.algo {
                        HashAlgo::Sha256 => Password::new_sha256(&plain_password).await,
                        HashAlgo::Sha512 => Password::new_sha512(&plain_password).await,
                        HashAlgo::Pbkdf2 => Password::new_pbkdf2(&plain_password).await?,
                    };
                    Ok(Some(pack(&PayloadPasswordHash {
                        hash: password.to_string(),
                    })?))
                }
            }
            "user.list" => {
                #[derive(Deserialize, Default)]
                #[serde(deny_unknown_fields)]
                struct Params {
                    #[serde(default)]
                    full: bool,
                    #[serde(default)]
                    with_password: bool,
                }
                let p = if payload.is_empty() {
                    Params::default()
                } else {
                    unpack(payload)?
                };
                let users = USERS.lock().await;
                let mut user_info: Vec<UserInfo> = users
                    .values()
                    .map(|u| u.to_info(p.full, p.with_password))
                    .collect();
                user_info.sort();
                Ok(Some(pack(&user_info)?))
            }
            "user.deploy" => {
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    #[derive(Deserialize)]
                    #[serde(deny_unknown_fields)]
                    struct Users {
                        users: Vec<User>,
                    }
                    let new_users: Users = unpack(payload)?;
                    let reg = REG.get().unwrap();
                    let mut reg_users = Vec::new();
                    {
                        let mut users = USERS.lock().await;
                        for user in &new_users.users {
                            if user.login.starts_with('.') {
                                return Err(Error::invalid_data(format!(
                                    "user login can not start with a dot: {}",
                                    user.login
                                ))
                                .into());
                            }
                            for c in user.login.chars() {
                                if !c.is_alphanumeric()
                                    && !ID_ALLOWED_SYMBOLS.contains(c)
                                    && c != '/'
                                {
                                    return Err(Error::invalid_data(format!(
                                        "invalid symbol in user login {}: {}",
                                        user.login, c
                                    ))
                                    .into());
                                }
                            }
                        }
                        for user in new_users.users {
                            let login = user.login.clone();
                            reg_users.push((login.clone(), to_value(&user)?));
                            users.insert(login, user);
                        }
                    }
                    for (login, val) in reg_users {
                        warn!("user created: {}", login);
                        reg.key_set(&format!("user/{login}"), val.clone()).await?;
                        self.tx
                            .send((format!("{AAA_USER_TOPIC}{login}"), Some(val)))
                            .await
                            .log_ef();
                    }
                    Ok(None)
                }
            }
            "user.undeploy" => {
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    #[derive(Deserialize)]
                    #[serde(deny_unknown_fields)]
                    struct Users {
                        users: Vec<UserOrLogin>,
                    }
                    let u_users: Users = unpack(payload)?;
                    let reg = REG.get().unwrap();
                    let mut ids = Vec::new();
                    {
                        let mut users = USERS.lock().await;
                        for user in u_users.users {
                            if let Some(user) = users.remove(user.as_str()) {
                                warn!("user destroyed: {}", user.login);
                                ids.push(user.login.clone());
                            }
                        }
                    }
                    for id in ids {
                        reg.key_delete(&format!("user/{id}")).await?;
                        self.tx
                            .send((format!("{AAA_USER_TOPIC}{id}"), None))
                            .await
                            .log_ef();
                    }
                    Ok(None)
                }
            }
            "user.get_profile_field" => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Params {
                    i: String,
                    field: ProfileField,
                }
                #[derive(Serialize)]
                struct Field {
                    value: Option<Value>,
                    readonly: bool,
                }
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    let p: Params = unpack(payload)?;
                    if let Some(user) = USERS.lock().await.get(&p.i) {
                        Ok(Some(pack(&Field {
                            value: user.profile.get_field(p.field),
                            readonly: false,
                        })?))
                    } else {
                        Err(Error::not_found(format!("user {} not found", p.i)).into())
                    }
                }
            }
            "user.set_profile_field" => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Params {
                    i: String,
                    field: ProfileField,
                    value: Value,
                }
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    let p: Params = unpack(payload)?;
                    let reg = REG.get().unwrap();
                    let val = {
                        let mut users = USERS.lock().await;
                        if let Some(user) = users.get_mut(&p.i) {
                            user.profile.set_field(p.field, p.value);
                            to_value(&user)?
                        } else {
                            return Err(Error::not_found(format!("user {} not found", p.i)).into());
                        }
                    };
                    reg.key_set(&format!("user/{}", p.i), val.clone()).await?;
                    Ok(None)
                }
            }
            "user.get_config" => {
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    let p: ParamsId = unpack(payload)?;
                    if let Some(user) = USERS.lock().await.get(p.i) {
                        Ok(Some(pack(user)?))
                    } else {
                        Err(Error::not_found(format!("user {} not found", p.i)).into())
                    }
                }
            }
            "user.export" => {
                #[derive(Serialize)]
                #[serde(deny_unknown_fields)]
                struct Users<'a> {
                    users: Vec<&'a User>,
                }
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    let p: ParamsId = unpack(payload)?;
                    let mut configs = Vec::new();
                    let users = USERS.lock().await;
                    if p.i == "#" || p.i == "*" {
                        for user in users.values() {
                            configs.push(user);
                        }
                    } else if let Some(user) = users.get(p.i) {
                        configs.push(user);
                    } else {
                        return Err(Error::not_found(format!("API user {} not found", p.i)).into());
                    }
                    configs.sort();
                    Ok(Some(pack(&Users { users: configs })?))
                }
            }
            "user.destroy" => {
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    let p: ParamsId = unpack(payload)?;
                    let reg = REG.get().unwrap();
                    if USERS.lock().await.remove(p.i).is_some() {
                        reg.key_delete(&format!("user/{}", p.i)).await?;
                        warn!("user destroyed: {}", p.i);
                        self.tx
                            .send((format!("{AAA_USER_TOPIC}{}", p.i), None))
                            .await
                            .log_ef();
                        Ok(None)
                    } else {
                        Err(Error::not_found(format!("user {} not found", p.i)).into())
                    }
                }
            }
            "user.set_password" => {
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    let p: ParamsIdPass = unpack(payload)?;
                    if p.password.is_empty() {
                        return Err(Error::invalid_data("the password can not be empty").into());
                    }
                    if p.check_policy {
                        self.check_password_policy(p.password)?;
                    }
                    let reg = REG.get().unwrap();
                    let val = {
                        let mut users = USERS.lock().await;
                        if let Some(user) = users.get_mut(p.i) {
                            user.password = Password::new_pbkdf2(p.password).await?;
                            to_value(&user)?
                        } else {
                            return Err(Error::not_found(format!("user {} not found", p.i)).into());
                        }
                    };
                    warn!("user password changed: {}", p.i);
                    reg.key_set(&format!("user/{}", p.i), val.clone()).await?;
                    let result = pack(&val)?;
                    self.tx
                        .send((format!("{AAA_USER_TOPIC}{}", p.i), Some(val)))
                        .await
                        .log_ef();
                    Ok(Some(result))
                }
            }
            "user.create_one_time" => {
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    #[derive(Deserialize)]
                    struct ParamsOneTimeUser {
                        login: Option<String>,
                        acls: HashSet<String>,
                    }
                    #[derive(Serialize)]
                    struct PayloadOneTimeUser {
                        login: String,
                        password: String,
                    }
                    let p: ParamsOneTimeUser = unpack(payload)?;
                    if ONE_TIME_EXPIRES.get().is_some() {
                        let mut ot_users = ONE_TIME_USERS.lock().await;
                        let (entry, login) = loop {
                            let rand_str = random_string(16)?;
                            let login = if let Some(ref l) = p.login {
                                format!("{}.{}", l, rand_str)
                            } else {
                                rand_str
                            };
                            if let hash_map::Entry::Vacant(entry) = ot_users.entry(login.clone()) {
                                break (entry, login);
                            }
                        };
                        let (one_time_user, password_plain) =
                            OneTimeUser::create(&login, p.acls).await?;
                        let u = one_time_user.user.as_ref().unwrap();
                        let ot_user_info = PayloadOneTimeUser {
                            login: format!("{}{}", ONE_TIME_USER_PREFIX, u.login),
                            password: password_plain,
                        };
                        let res = pack(&ot_user_info)?;
                        entry.insert(one_time_user);
                        Ok(Some(res))
                    } else {
                        Err(Error::unsupported("one-time users are not enabled").into())
                    }
                }
            }
            "auth.user" => {
                #[derive(Deserialize)]
                struct ParamsAuthUser<'a> {
                    #[serde(borrow)]
                    login: &'a str,
                    #[serde(borrow)]
                    password: &'a str,
                    #[serde(
                        default = "get_timeout",
                        deserialize_with = "eva_common::tools::de_float_as_duration"
                    )]
                    timeout: Duration,
                    xopts: Option<HashMap<String, Value>>,
                }
                #[derive(Serialize)]
                struct ParamsOtpCheck<'a> {
                    login: &'a str,
                    otp: Option<&'a Value>,
                }
                if payload.is_empty() {
                    return Err(RpcError::params(None));
                }
                let p: ParamsAuthUser = unpack(payload)?;
                let (acls, one_time) =
                    if let Some(login) = p.login.strip_prefix(ONE_TIME_USER_PREFIX) {
                        if let Some(one_time_user) = ONE_TIME_USERS.lock().await.get_mut(login) {
                            (one_time_user.get_acls(p.password).await?, true)
                        } else {
                            debug!("one-time user {} authentication failed: not found", p.login);
                            return Err(Error::access(ERR_INVALID_LOGIN_PASS).into());
                        }
                    } else if let Some(user) = USERS.lock().await.get(p.login) {
                        if !user.password.verify(p.password).await? {
                            debug!("user {} authentication failed: invalid password", p.login);
                            return Err(Error::access(ERR_INVALID_LOGIN_PASS).into());
                        }
                        debug!("user authenticated: {}", p.login);
                        (user.acls.iter().cloned().collect(), false)
                    } else {
                        debug!("user {} authentication failed: not found", p.login);
                        return Err(Error::access(ERR_INVALID_LOGIN_PASS).into());
                    };
                let rpc = RPC.get().unwrap();
                let op = Op::new(p.timeout);
                if !one_time {
                    if let Some(otp_svc) = self.otp_svc.as_ref() {
                        safe_rpc_call(
                            rpc,
                            otp_svc,
                            "otp.check",
                            pack(&ParamsOtpCheck {
                                login: p.login,
                                otp: if let Some(xopts) = p.xopts.as_ref() {
                                    xopts.get("otp")
                                } else {
                                    None
                                },
                            })?
                            .into(),
                            QoS::Processed,
                            op.timeout()?,
                        )
                        .await?;
                    }
                }
                let result = safe_rpc_call(
                    rpc,
                    &self.acl_svc,
                    "acl.format",
                    pack(&ParamsIdListOwned { i: acls })?.into(),
                    QoS::Processed,
                    op.timeout()?,
                )
                .await?;
                Ok(Some(result.payload().to_vec()))
            }
            m => svc_handle_default_rpc(m, &self.info),
        }
    }
}

impl Handlers {
    fn check_password_policy(&self, p: &str) -> EResult<()> {
        let mut err: Vec<String> = Vec::new();
        if p.chars().count() < self.password_policy.min_length {
            err.push(format!(
                "min. length: {} symbols",
                self.password_policy.min_length
            ));
        }
        if self.password_policy.required_letter {
            let mut found = false;
            for ch in p.chars() {
                if ch.is_alphabetic() {
                    found = true;
                    break;
                }
            }
            if !found {
                err.push("must contain at least one letter".to_owned());
            }
        }
        if self.password_policy.required_number {
            let mut found = false;
            for ch in p.chars() {
                if ch.is_numeric() {
                    found = true;
                    break;
                }
            }
            if !found {
                err.push("must contain at least one number".to_owned());
            }
        }
        if self.password_policy.required_mixed_case {
            let mut upper_found = false;
            let mut lower_found = false;
            for ch in p.chars() {
                if ch.is_uppercase() {
                    upper_found = true;
                } else if ch.is_lowercase() {
                    lower_found = true;
                }
                if upper_found && lower_found {
                    break;
                }
            }
            if !upper_found || !lower_found {
                err.push("must contain at least one uppercase and one lowercase letter".to_owned());
            }
        }
        if err.is_empty() {
            Ok(())
        } else {
            Err(Error::invalid_params(format!(
                "Invalid password: {}",
                err.join(", ")
            )))
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    acl_svc: String,
    otp_svc: Option<String>,
    one_time: Option<OneTimeConfig>,
    #[serde(default)]
    password_policy: PasswordPolicy,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OneTimeConfig {
    #[serde(deserialize_with = "eva_common::tools::de_float_as_duration")]
    expires: Duration,
}

#[svc_main]
async fn main(mut initial: Initial) -> EResult<()> {
    TIMEOUT
        .set(initial.timeout())
        .map_err(|_| Error::core("unable to set timeout"))?;
    let config: Config = Config::deserialize(
        initial
            .take_config()
            .ok_or_else(|| Error::invalid_data("config not specified"))?,
    )?;
    if let Some(one_time) = config.one_time {
        ONE_TIME_EXPIRES
            .set(one_time.expires)
            .map_err(|_| Error::core("unable to set ONE_TIME_EXPIRES"))?;
        eva_common::cleaner!(
            "one_time_users",
            clear_one_time_users,
            ONE_TIME_USER_CLEANUP_INTERVAL
        );
    }
    let (tx, rx) = async_channel::bounded(1024);
    let mut info = ServiceInfo::new(AUTHOR, VERSION, DESCRIPTION);
    info.add_method(ServiceMethod::new("key.list"));
    info.add_method(ServiceMethod::new("key.deploy").required("keys"));
    info.add_method(ServiceMethod::new("key.undeploy").required("keys"));
    info.add_method(ServiceMethod::new("key.get_config").required("i"));
    info.add_method(ServiceMethod::new("key.export").required("i"));
    info.add_method(ServiceMethod::new("key.get").required("i"));
    info.add_method(ServiceMethod::new("key.destroy").required("i"));
    info.add_method(ServiceMethod::new("key.regenerate").required("i"));
    info.add_method(
        ServiceMethod::new("auth.key")
            .required("key")
            .optional("timeout"),
    );
    info.add_method(
        ServiceMethod::new("password.hash")
            .required("password")
            .required("algo"),
    );
    info.add_method(ServiceMethod::new("user.list"));
    info.add_method(ServiceMethod::new("user.deploy").required("users"));
    info.add_method(ServiceMethod::new("user.undeploy").required("users"));
    info.add_method(ServiceMethod::new("user.get_config").required("i"));
    info.add_method(ServiceMethod::new("user.export").required("i"));
    info.add_method(ServiceMethod::new("user.destroy").required("i"));
    info.add_method(
        ServiceMethod::new("user.get_profile_field")
            .required("i")
            .required("field"),
    );
    info.add_method(
        ServiceMethod::new("user.set_profile_field")
            .required("i")
            .required("field")
            .required("value"),
    );
    info.add_method(
        ServiceMethod::new("user.set_password")
            .required("i")
            .required("password")
            .optional("check_policy"),
    );
    info.add_method(ServiceMethod::new("user.create_one_time").required("acls"));
    info.add_method(
        ServiceMethod::new("auth.user")
            .required("login")
            .required("password")
            .required("timeout"),
    );
    let handlers = Handlers {
        info,
        password_policy: config.password_policy,
        acl_svc: config.acl_svc,
        otp_svc: config.otp_svc,
        tx,
    };
    let rpc = initial.init_rpc(handlers).await?;
    initial.drop_privileges()?;
    let client = rpc.client().clone();
    let cl = client.clone();
    tokio::spawn(async move {
        while let Ok((topic, val)) = rx.recv().await {
            let payload: busrt::borrow::Cow = if let Some(v) = val {
                if let Ok(p) = pack(&v).log_err() {
                    p.into()
                } else {
                    continue;
                }
            } else {
                busrt::empty_payload!()
            };
            cl.lock()
                .await
                .publish(&topic, payload, QoS::No)
                .await
                .log_ef();
        }
    });
    svc_init_logs(&initial, client.clone())?;
    let registry = initial.init_registry(&rpc);
    let reg_keys = registry.key_get_recursive("key").await?;
    {
        let mut kdb = KEYDB.lock().unwrap();
        for (k, v) in reg_keys {
            debug!("API key loaded: {}", k);
            kdb.append(Arc::new(Key::deserialize(v)?))?;
        }
    }
    let reg_users = registry.key_get_recursive("user").await?;
    {
        let mut u = USERS.lock().await;
        for (k, v) in reg_users {
            debug!("user loaded: {}", k);
            let user: User = User::deserialize(v)?;
            u.insert(user.login.clone(), user);
        }
    }
    RPC.set(rpc.clone())
        .map_err(|_| Error::core("unable to set RPC"))?;
    REG.set(registry)
        .map_err(|_| Error::core("unable to set registry object"))?;
    svc_start_signal_handlers();
    svc_mark_ready(&client).await?;
    info!("{} started ({})", DESCRIPTION, initial.id());
    svc_block(&rpc).await;
    svc_mark_terminating(&client).await?;
    Ok(())
}
