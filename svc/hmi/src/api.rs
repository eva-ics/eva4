use crate::aaa::{self, Auth, Token};
use crate::aci::ACI;
use crate::db;
use crate::ApiKeyId;
use base64::Engine as _;
use eva_common::acl::{self, Acl, OIDMask};
use eva_common::common_payloads::ParamsIdOrListOwned;
use eva_common::common_payloads::ValueOrList;
use eva_common::prelude::*;
use eva_sdk::fs as sdkfs;
use eva_sdk::prelude::*;
use eva_sdk::types::{CompactStateHistory, Fill, HistoricalState, StateProp};
use lazy_static::lazy_static;
use log::{error, trace};
use rjrpc::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::{btree_map, BTreeMap, HashMap};
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;
use uuid::Uuid;

err_logger!();

const ERR_NO_PARAMS: &str = "no params provided";
const ERR_NO_METHOD: &str = "no such RPC method";
const ERR_DF_TIMES: &str = "dataframe times mismatch";
const ERR_DEMO_MODE: &str = "disabled in demo mode";

const API_VERSION: u16 = 4;

lazy_static! {
    static ref EHMI_LOCK: Mutex<()> = <_>::default();
    static ref BASE64: base64::engine::general_purpose::GeneralPurpose = {
        let config = base64::engine::GeneralPurposeConfig::new().with_encode_padding(false);
        base64::engine::GeneralPurpose::new(&base64::alphabet::URL_SAFE, config)
    };
}

macro_rules! parse_ts {
    ($t: expr) => {
        if let Some(ref t) = $t {
            Some(t.as_timestamp()?)
        } else {
            None
        }
    };
}

/// # Note
///
/// The key object is kept during the whole API call even if revoked
pub async fn processor(
    mut request: JsonRpcRequest,
    meta: JsonRpcRequestMeta,
) -> Option<JsonRpcResponse> {
    let params = request.take_params();
    let result = match call(&request.method, params, meta).await {
        Ok(v) => v,
        Err(e) if e.kind() == ErrorKind::EvaHIAuthenticationRequired => {
            let mut je = rjrpc::JrpcException::new(401, String::new());
            je.set_header(
                "www-authenticate".to_owned(),
                "Basic realm=\"?\"".to_owned(),
            );
            return request.jexception(je);
        }
        Err(e) => {
            return request.error(e.into());
        }
    };
    jrpc_q!(request);
    // todo user accounts and tokens
    request.respond(result)
}

async fn login_meta(meta: JsonRpcRequestMeta, ip: Option<IpAddr>) -> EResult<Value> {
    if let Some(key) = meta.key() {
        let source = ip.map_or_else(|| "-".to_owned(), |v| v.to_string());
        Ok(to_value(AuthResult::new(
            login_key(key, ip, &source).await?,
        ))?)
    } else if let Some(creds) = meta.credentials() {
        let source = ip.map_or_else(|| "-".to_owned(), |v| v.to_string());
        login(&creds.0, &creds.1, None, None, ip, &source).await
    } else {
        if let Some(agent) = meta.agent() {
            if agent.starts_with("evaHI ") {
                return Err(Error::new0(ErrorKind::EvaHIAuthenticationRequired));
            }
        }
        Err(Error::access("No authentication data provided"))
    }
}

enum PvtOp {
    List,
    Read,
    Write,
}

fn format_and_check_path(s: &str, acl: &Acl, op: PvtOp) -> EResult<PathBuf> {
    macro_rules! check_acl {
        ($path: expr) => {
            match op {
                PvtOp::Read => acl.require_pvt_read($path)?,
                PvtOp::Write => acl.require_pvt_write($path)?,
                PvtOp::List => {}
            }
        };
    }
    let Some(pvt_path) = crate::PVT_PATH.get().unwrap() else {
        return Err(Error::unsupported("PVT path not set"));
    };
    let fpath = Path::new(s);
    if s == "/" {
        check_acl!("");
        Ok(Path::new(pvt_path).to_owned())
    } else if fpath.is_absolute() {
        Err(Error::access(
            "the path must be relative to the pvt directory",
        ))
    } else if s.contains("../") {
        Err(Error::access("the path can not contain ../"))
    } else if s.contains("/./") || s.starts_with("./") {
        Err(Error::access("the path can not contain or start with ./"))
    } else {
        check_acl!(s);
        let mut path = Path::new(pvt_path).to_owned();
        path.extend(fpath);
        Ok(path)
    }
}

#[derive(Serialize)]
struct AuthResult {
    #[serde(flatten)]
    token: Arc<Token>,
    api_version: u16,
}

impl AuthResult {
    #[inline]
    fn new(token: Arc<Token>) -> Self {
        Self {
            token,
            api_version: API_VERSION,
        }
    }
}

macro_rules! demo_mode_abort {
    () => {
        if crate::demo_mode() {
            return Err(Error::unsupported(ERR_DEMO_MODE));
        }
    };
}

macro_rules! prepare_api_filter_params {
    ($params: expr) => {
        crate::API_FILTER.get().map(|_| $params.clone())
    };
}

macro_rules! run_api_filter {
    ($method: expr, $api_filter_params: expr, $aci: expr) => {
        if let Some(api_filter) = crate::API_FILTER.get() {
            run_api_filter(api_filter, $method, $api_filter_params.unwrap(), $aci).await?;
        }
    };
}

async fn run_api_filter(
    i: &OID,
    api_call_method: &str,
    api_call_params: Value,
    aci: &ACI,
) -> EResult<()> {
    #[derive(Serialize)]
    struct Payload<'a> {
        i: &'a OID,
        params: eva_common::actions::Params,
        #[serde(serialize_with = "eva_common::tools::serialize_duration_as_f64")]
        wait: Duration,
    }
    #[derive(Deserialize)]
    struct ActionResponse {
        exitcode: Option<i16>,
        out: Option<Value>,
        err: Option<Value>,
    }
    let mut kwargs = HashMap::new();
    kwargs.insert(
        "api_call_method".to_owned(),
        Value::String(api_call_method.to_owned()),
    );
    kwargs.insert("api_call_params".to_owned(), api_call_params);
    kwargs.insert("aci".to_owned(), to_value(aci)?);
    kwargs.insert("acl".to_owned(), to_value(aci.acl())?);
    let params = eva_common::actions::Params::new_lmacro(None, Some(kwargs));
    let payload = Payload {
        i,
        params,
        wait: eapi_bus::timeout(),
    };
    let res: ActionResponse = unpack(
        eapi_bus::call("eva.core", "run", pack(&payload)?.into())
            .await?
            .payload(),
    )?;
    if let Some(code) = res.exitcode {
        if code == 0 {
            Ok(())
        } else {
            if let Some(err) = res.err {
                if !err.is_empty() {
                    return Err(Error::access(err));
                }
            }
            if let Some(out) = res.out {
                if !out.is_empty() {
                    return Err(Error::access(out));
                }
            }
            Err(Error::access("Denied by API filter"))
        }
    } else {
        Err(Error::access("API filter timeout"))
    }
}

#[allow(clippy::similar_names)]
async fn login_key(key: &str, ip: Option<IpAddr>, source: &str) -> EResult<Arc<Token>> {
    let mut aci = ACI::new(Auth::LoginKey(None, None), "login", source.to_owned());
    if let Some((acl, svc)) = aaa::auth_key(key).await? {
        if let Some(id) = acl.api_key_id() {
            // API key IDs MUST be prefixed with a dot in both ACI and tokens
            let login = format!(".{}", id);
            aci.set_key_id(&login);
            aci.set_acl_id(acl.id());
            let token = aaa::create_token(&login, acl, svc, ip).await?;
            aci.log_request(log::Level::Info).await.log_ef();
            aci.log_success().await;
            Ok(token)
        } else {
            Err(Error::failed("API key ID not found"))
        }
    } else {
        error!(
            "API login with a key failed ({})",
            ip.map_or_else(String::new, |v| v.to_string())
        );
        let e = Error::access("access denied");
        aci.log_request(log::Level::Info).await.log_ef();
        aci.log_error(&e).await;
        Err(e)
    }
}

#[allow(clippy::similar_names)]
async fn login(
    login: &str,
    password: &str,
    xopts: Option<&BTreeMap<String, Value>>,
    token_id: Option<String>,
    ip: Option<IpAddr>,
    source: &str,
) -> EResult<Value> {
    #[derive(Serialize)]
    struct AuthPayload<'a> {
        login: &'a str,
        password: &'a str,
        timeout: f64,
        xopts: Option<&'a BTreeMap<String, Value>>,
    }
    let auth_svcs = aaa::auth_svcs();
    let payload = pack(&AuthPayload {
        login,
        password,
        timeout: eapi_bus::timeout().as_secs_f64(),
        xopts,
    })?;
    let mut aci = ACI::new(
        Auth::Login(login.to_owned(), None),
        "login",
        source.to_owned(),
    );
    for svc in auth_svcs {
        match eapi_bus::call(svc, "auth.user", payload.as_slice().into()).await {
            Ok(result) => {
                if let Some(ti) = token_id {
                    let token = aaa::get_token(ti.try_into()?, ip).await?;
                    if token.user() != login {
                        return Err(Error::access("token user does not match"));
                    }
                    if token.is_readonly() {
                        token.set_normal().await?;
                    }
                    return Ok(to_value(AuthResult::new(token))?);
                }
                let acl = unpack::<Acl>(result.payload())?;
                aci.set_acl_id(acl.id());
                let token = aaa::create_token(login, acl, svc, ip).await?;
                aci.log_request(log::Level::Info).await.log_ef();
                aci.log_success().await;
                return Ok(to_value(AuthResult::new(token))?);
            }
            Err(e) => {
                match e.kind() {
                    ErrorKind::AccessDenied => {
                        if let Some(msg) = e.message() {
                            if msg.starts_with('|') {
                                return Err(e);
                            }
                        }
                    }
                    ErrorKind::AccessDeniedMoreDataRequired => {
                        return Err(e);
                    }
                    _ => {}
                }
                trace!("auth service returned an error: {} {}", svc, e);
            }
        }
    }
    error!(
        "API login failed for {} ({})",
        login,
        ip.map_or_else(String::new, |v| v.to_string())
    );
    let e = Error::access("access denied");
    aci.log_param("login", login)?;
    aci.log_request(log::Level::Info).await.log_ef();
    aci.log_error(&e).await;
    Err(e)
}

async fn method_call(params: Value, meta: JsonRpcRequestMeta) -> EResult<Value> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Params {
        q: String,
        k: Option<String>,
    }
    let p = Params::deserialize(params)?;
    let (call_method, mut call_params) = crate::call_parser::parse_call_str(&p.q)?;
    if let Some(key) = p.k {
        if let Value::Map(ref mut m) = call_params {
            if let btree_map::Entry::Vacant(entry) = m.entry(Value::String("k".to_owned())) {
                entry.insert(Value::String(key));
            }
        }
    }
    if call_method == "call" {
        Err(Error::new(ErrorKind::MethodNotFound, ERR_NO_METHOD))
    } else {
        match call_method.as_str() {
            "s" => call("item.state", Some(call_params), meta).await,
            "h" => call("item.state_history", Some(call_params), meta).await,
            "ehmi" => call("ehmi.create_config", Some(call_params), meta).await,
            _ => call(&call_method, Some(call_params), meta).await,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ParamsEmpty {}

#[inline]
fn gen_source(ip: Option<IpAddr>) -> String {
    ip.map_or_else(|| "-".to_owned(), |v| v.to_string())
}

/// # Errors
///
/// Returns Err if the call is failed
#[allow(clippy::too_many_lines)]
#[async_recursion::async_recursion]
async fn call(method: &str, params: Option<Value>, mut meta: JsonRpcRequestMeta) -> EResult<Value> {
    if method == "call" {
        return method_call(params.unwrap_or_default(), meta).await;
    }
    let ip = meta.ip();
    let source = gen_source(ip);
    match method {
        "login" => {
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct ParamsTokenSetNormal {
                #[serde(alias = "u")]
                user: String,
                #[serde(alias = "p")]
                password: String,
                #[serde(alias = "a")]
                token: String,
                xopts: Option<BTreeMap<String, Value>>,
            }
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct ParamsUserAuth {
                #[serde(alias = "u")]
                user: String,
                #[serde(alias = "p")]
                password: String,
                xopts: Option<BTreeMap<String, Value>>,
            }
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct ParamsKeyAuth {
                k: String,
            }
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct ParamsTokenInfo {
                #[serde(alias = "a")]
                token: String,
            }
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            #[serde(untagged)]
            enum ParamsLogin {
                TokenSetNormal(ParamsTokenSetNormal),
                UserAuth(ParamsUserAuth),
                KeyAuth(ParamsKeyAuth),
                TokenInfo(ParamsTokenInfo),
                Empty(ParamsEmpty),
            }
            if let Some(params) = params {
                let p = ParamsLogin::deserialize(params)?;
                match p {
                    ParamsLogin::UserAuth(creds) => {
                        login(
                            &creds.user,
                            &creds.password,
                            creds.xopts.as_ref(),
                            None,
                            ip,
                            &source,
                        )
                        .await
                    }
                    ParamsLogin::KeyAuth(creds) => Ok(to_value(AuthResult::new(
                        login_key(&creds.k, ip, &source).await?,
                    ))?),
                    ParamsLogin::TokenInfo(ti) => {
                        let token = aaa::get_token(ti.token.try_into()?, ip).await?;
                        Ok(to_value(AuthResult::new(token))?)
                    }
                    ParamsLogin::TokenSetNormal(creds) => {
                        login(
                            &creds.user,
                            &creds.password,
                            creds.xopts.as_ref(),
                            Some(creds.token),
                            ip,
                            &source,
                        )
                        .await
                    }
                    ParamsLogin::Empty(_) => login_meta(meta, ip).await,
                }
            } else {
                login_meta(meta, ip).await
            }
        }
        "logout" => {
            #[derive(Deserialize)]
            struct ParamsLogout {
                #[serde(alias = "a")]
                token: String,
            }
            let p = ParamsLogout::deserialize(
                params.ok_or_else(|| Error::invalid_params(ERR_NO_PARAMS))?,
            )?;
            aaa::destroy_token(&(p.token.try_into()?)).await.log_ef();
            ok!()
        }
        "ehmi.get_config" => method_ehmi_get_config(params.unwrap_or_default()).await,
        _ => {
            if let Some(mut params) = params {
                let auth_key: String = if let Value::Map(ref mut m) = params {
                    if let Some(v) = m.remove(&Value::String("k".to_owned())) {
                        String::deserialize(v)?
                    } else {
                        meta.take_key().ok_or_else(|| {
                            Error::new(ErrorKind::AccessDenied, "No token/API key specified")
                        })?
                    }
                } else {
                    return Err(Error::new0(ErrorKind::AccessDenied));
                };
                let auth = match aaa::authenticate(&auth_key, ip).await {
                    Ok(v) => v,
                    Err(e) => {
                        error!("API access error from {}: {}", source, e);
                        return Err(e);
                    }
                };
                let mut aci = ACI::new(auth, method, source);
                // TODO remove v3 API compat aliases
                if [
                    "state",
                    "state_history",
                    "state_log",
                    "log_get",
                    "action_toggle",
                    "get_neighbor_clients",
                    "set_token_readonly",
                    "check_item_access",
                    "result",
                    "kill",
                    "terminate",
                    "set",
                    "reset",
                    "clear",
                    "toggle",
                    "increment",
                    "decrement",
                ]
                .contains(&method)
                {
                    warn!("deprecated method: {}, consider using the new name", method);
                }
                let result = if let Some(bus_call) = method.strip_prefix("bus::") {
                    let mut sp = bus_call.splitn(2, "::");
                    let target = sp
                        .next()
                        .ok_or_else(|| Error::invalid_params("bus target not specified"))?;
                    let bus_method = sp
                        .next()
                        .ok_or_else(|| Error::invalid_params("bus method not specified"))?;
                    method_bus_call(target, bus_method, params, &mut aci).await
                } else if let Some(x_call) = method.strip_prefix("x::") {
                    let mut sp = x_call.split("::");
                    let target = sp
                        .next()
                        .ok_or_else(|| Error::invalid_params("bus target not specified"))?;
                    let x_method = sp
                        .next()
                        .ok_or_else(|| Error::invalid_params("bus method not specified"))?;
                    method_x_call(target, x_method, params, &mut aci).await
                } else {
                    match method {
                        "test" => method_test(params, &mut aci).await,
                        "set_password" => method_set_password(params, &mut aci).await,
                        "profile.get_field" => method_get_profile_field(params, &mut aci).await,
                        "profile.set_field" => method_set_profile_field(params, &mut aci).await,
                        "item.state" | "state" => method_item_state(params, &mut aci).await,
                        "item.check_access" | "check_item_access" => {
                            method_item_check_access(params, &mut aci).await
                        }
                        "item.state_history" | "state_history" => {
                            method_item_state_history(params, &mut aci).await
                        }
                        "item.state_log" | "state_log" => {
                            method_item_state_log(params, &mut aci).await
                        }
                        "log.get" | "log_get" => method_log_get(params, &mut aci).await,
                        "api_log.get" => method_api_log_get(params, &mut aci).await,
                        "action" => method_action(params, &mut aci).await,
                        "action.toggle" | "action_toggle" => {
                            method_action_toggle(params, &mut aci).await
                        }
                        "action.result" | "result" => method_action_result(params, &mut aci).await,
                        "action.kill" | "kill" => method_action_kill(params, &mut aci).await,
                        "action.terminate" | "terminate" => {
                            method_action_terminate(params, &mut aci).await
                        }
                        "run" => method_run(params, &mut aci).await,
                        "lvar.set" | "set" => method_lvar_set(params, &mut aci).await,
                        "lvar.reset" | "reset" => method_lvar_reset(params, &mut aci).await,
                        "lvar.clear" | "clear" => method_lvar_clear(params, &mut aci).await,
                        "lvar.toggle" | "toggle" => method_lvar_toggle(params, &mut aci).await,
                        "lvar.incr" | "increment" => method_lvar_incr(params, &mut aci).await,
                        "lvar.decr" | "decrement" => method_lvar_decr(params, &mut aci).await,
                        "session.list_neighbors" | "get_neighbor_clients" => {
                            method_session_list_neighbors(params, &mut aci).await
                        }
                        "session.set_readonly" | "set_token_readonly" => {
                            method_session_set_readonly(params, &mut aci).await
                        }
                        "ehmi.create_config" => {
                            method_ehmi_create_config(&auth_key, params, &mut aci, &gen_source(ip))
                                .await
                        }
                        "user_data.get" => method_user_data_get(params, &mut aci).await,
                        "user_data.set" => method_user_data_set(params, &mut aci).await,
                        "user_data.delete" => method_user_data_delete(params, &mut aci).await,
                        "db.list" => method_db_list(params, &mut aci).await,
                        "pvt.put" => method_pvt_put(params, &mut aci).await,
                        "pvt.get" => method_pvt_get(params, &mut aci).await,
                        "pvt.unlink" => method_pvt_unlink(params, &mut aci).await,
                        "pvt.list" => method_pvt_list(params, &mut aci).await,
                        _ => {
                            return Err(Error::new(ErrorKind::MethodNotFound, ERR_NO_METHOD));
                        }
                    }
                };
                if let Err(ref e) = result {
                    aci.log_error(e).await;
                } else {
                    aci.log_success().await;
                }
                result
            } else {
                Err(Error::new0(ErrorKind::InvalidParameter))
            }
        }
    }
}

#[inline]
fn ok_true() -> bool {
    true
}

#[derive(Serialize)]
struct TokenInfo<'a> {
    acl: &'a str,
    mode: &'a str,
    u: &'a str,
}

impl<'a> From<&'a Token> for TokenInfo<'a> {
    fn from(token: &'a Token) -> Self {
        Self {
            acl: token.acl().id(),
            mode: token.mode_as_str(),
            u: token.user(),
        }
    }
}

async fn method_session_list_neighbors(params: Value, aci: &mut ACI) -> EResult<Value> {
    aci.log_request(log::Level::Debug).await.log_ef();
    ParamsEmpty::deserialize(params)?;
    let tokens = aaa::list_neighbors().await?;
    let result: Vec<TokenInfo> = tokens
        .iter()
        .filter(|v| aci.token().map_or(true, |t| t.id() != v.id()))
        .map(Into::into)
        .collect();
    to_value(result).map_err(Into::into)
}

async fn method_test(params: Value, aci: &mut ACI) -> EResult<Value> {
    #[derive(Serialize, Deserialize)]
    struct TestInfo<'a> {
        #[serde(default = "ok_true")]
        ok: bool,
        build: u64,
        product_code: String,
        product_name: String,
        system_name: String,
        time: f64,
        uptime: f64,
        version: String,
        #[serde(skip_deserializing)]
        aci: Option<&'a ACI>,
        #[serde(skip_deserializing)]
        acl: Option<&'a Acl>,
        #[serde(skip_deserializing, skip_serializing_if = "Option::is_none")]
        hmi_svc_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        system_arch: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        num_cpus: Option<usize>,
    }
    aci.log_request(log::Level::Debug).await.log_ef();
    ParamsEmpty::deserialize(params)?;
    let mut info: TestInfo = unpack(
        eapi_bus::call("eva.core", "test", busrt::empty_payload!())
            .await?
            .payload(),
    )?;
    info.aci.replace(aci);
    info.acl.replace(aci.acl());
    if aci.acl().check_admin() {
        info.hmi_svc_id
            .replace(crate::SVC_ID.get().unwrap().clone());
    } else {
        info.system_arch.take();
        info.num_cpus.take();
    }
    to_value(info).map_err(Into::into)
}

async fn method_set_password(params: Value, aci: &mut ACI) -> EResult<Value> {
    #[derive(Deserialize)]
    struct ParamsSetPassword {
        current_password: String,
        password: String,
    }
    #[derive(Serialize)]
    struct AuthPayload<'a> {
        login: &'a str,
        password: &'a str,
        timeout: f64,
    }
    #[derive(Serialize)]
    struct PayloadIdPass<'a> {
        #[serde(borrow)]
        i: &'a str,
        #[serde(borrow)]
        password: &'a str,
        check_policy: bool,
    }
    aci.log_request(log::Level::Warn).await.log_ef();
    demo_mode_abort!();
    if let Some(token) = aci.token() {
        aci.check_write()?;
        let p = ParamsSetPassword::deserialize(params)?;
        // verify old password
        let payload = pack(&AuthPayload {
            login: token.user(),
            password: &p.current_password,
            timeout: eapi_bus::timeout().as_secs_f64(),
        })?;
        eapi_bus::call(token.auth_svc(), "auth.user", payload.as_slice().into()).await?;
        let payload = pack(&PayloadIdPass {
            i: token.user(),
            password: &p.password,
            check_policy: true,
        })?;
        eapi_bus::call(
            token.auth_svc(),
            "user.set_password",
            payload.as_slice().into(),
        )
        .await?;
        ok!()
    } else {
        Err(Error::access("not authenticated with login/password"))
    }
}

async fn method_get_profile_field(params: Value, aci: &mut ACI) -> EResult<Value> {
    #[derive(Serialize, Deserialize)]
    struct ParamsProfileField<'a> {
        #[serde(skip_deserializing)]
        i: Option<&'a str>,
        field: String,
    }
    aci.log_request(log::Level::Trace).await.log_ef();
    if let Some(token) = aci.token() {
        aci.check_write()?;
        let mut p = ParamsProfileField::deserialize(params)?;
        p.i.replace(token.user());
        let payload = pack(&p)?;
        let result: Value = unpack(
            eapi_bus::call(
                token.auth_svc(),
                "user.get_profile_field",
                payload.as_slice().into(),
            )
            .await?
            .payload(),
        )?;
        Ok(result)
    } else {
        Err(Error::access("not authenticated with login/password"))
    }
}

async fn method_set_profile_field(params: Value, aci: &mut ACI) -> EResult<Value> {
    #[derive(Serialize, Deserialize)]
    struct ParamsProfileField<'a> {
        #[serde(skip_deserializing)]
        i: Option<&'a str>,
        field: String,
        value: Value,
    }
    aci.log_request(log::Level::Info).await.log_ef();
    demo_mode_abort!();
    if let Some(token) = aci.token() {
        aci.check_write()?;
        let mut p = ParamsProfileField::deserialize(params)?;
        p.i.replace(token.user());
        let payload = pack(&p)?;
        eapi_bus::call(
            token.auth_svc(),
            "user.set_profile_field",
            payload.as_slice().into(),
        )
        .await?;
        ok!()
    } else {
        Err(Error::access("not authenticated with login/password"))
    }
}

async fn method_item_state(params: Value, aci: &mut ACI) -> EResult<Value> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct StateParams {
        i: Value,
        #[serde(default)]
        full: bool,
    }
    #[derive(Serialize)]
    struct StatePayload {
        i: Value,
        include: Vec<String>,
        exclude: Vec<String>,
        full: bool,
    }
    aci.log_request(log::Level::Debug).await.log_ef();
    let p = StateParams::deserialize(params)?;
    let (allow, deny) = aci.acl().get_items_allow_deny_reading();
    let payload = StatePayload {
        i: p.i,
        include: allow,
        exclude: deny,
        full: p.full,
    };
    let result: Value = unpack(
        eapi_bus::call("eva.core", "item.state", pack(&payload)?.into())
            .await?
            .payload(),
    )?;
    Ok(result)
}

async fn method_item_check_access(params: Value, aci: &mut ACI) -> EResult<Value> {
    #[derive(Serialize)]
    struct PayloadAccess {
        r: bool,
        w: bool,
    }
    aci.log_request(log::Level::Debug).await.log_ef();
    let p = ParamsIdOrListOwned::deserialize(params)?;
    let mut result = BTreeMap::new();
    let mut oids = Vec::with_capacity(p.i.len());
    for i in p.i {
        oids.push(i.parse()?);
    }
    for oid in oids {
        let access = PayloadAccess {
            r: aci.acl().check_item_read(&oid),
            w: aci.acl().check_item_write(&oid),
        };
        result.insert(Value::String(oid.to_string()), to_value(access)?);
    }
    Ok(Value::Map(result))
}

#[inline]
fn get_default_db() -> String {
    crate::DEFAULT_HISTORY_DB_SVC.get().unwrap().clone()
}

#[allow(clippy::too_many_lines)]
async fn method_item_state_history(params: Value, aci: &mut ACI) -> EResult<Value> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OIDsingleOrMulti {
        Single(OID),
        Multi(Vec<OID>),
    }
    #[derive(Deserialize, Eq, PartialEq)]
    #[serde(rename_all = "lowercase")]
    enum OutputFormat {
        List,
        Dict,
    }
    impl Default for OutputFormat {
        #[inline]
        fn default() -> Self {
            Self::List
        }
    }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct StateHistoryParams {
        i: OIDsingleOrMulti,
        #[serde(alias = "s")]
        t_start: Option<Value>,
        #[serde(alias = "e")]
        t_end: Option<Value>,
        #[serde(alias = "w")]
        fill: Option<String>,
        #[serde(alias = "n")]
        limit: Option<u32>,
        #[serde(alias = "x")]
        prop: Option<StateProp>,
        #[serde(alias = "o", default)]
        xopts: BTreeMap<String, Value>,
        #[serde(alias = "a", default = "get_default_db")]
        database: String,
        #[serde(alias = "g", default)]
        output_format: OutputFormat,
    }
    #[derive(Serialize)]
    struct StateHistoryPayload<'a> {
        i: &'a OID,
        t_start: f64,
        t_end: f64,
        fill: Option<Fill>,
        precision: Option<u32>,
        limit: Option<u32>,
        prop: Option<StateProp>,
        xopts: &'a BTreeMap<String, Value>,
        compact: bool,
    }
    aci.log_request(log::Level::Debug).await.log_ef();
    let p = StateHistoryParams::deserialize(params)?;
    let ts_now = eva_common::time::now_ns_float();
    let t_start = parse_ts!(p.t_start).unwrap_or_else(|| ts_now - 86_400.0);
    let t_end = parse_ts!(p.t_end).unwrap_or(ts_now);
    if t_end < t_start {
        return Err(Error::invalid_params("invalid period"));
    }
    let (fill, precision) = if let Some(fill) = p.fill {
        let mut sp = fill.splitn(2, ':');
        let fill_str = sp.next().unwrap();
        let f: Fill = if let Some(x) = fill_str.strip_suffix('A') {
            let points: u32 = x.parse()?;
            if points > 10_000 {
                return Err(Error::invalid_params("too many points (max 10k)"));
            }
            if points < 2 {
                return Err(Error::invalid_params("points can not be less than 2"));
            }
            let mut period = (t_end - t_start) / f64::from(points - 1);
            if period < 1.0 {
                period = 1.0;
            }
            #[allow(clippy::cast_possible_truncation)]
            #[allow(clippy::cast_sign_loss)]
            Fill::Seconds(period.round() as u32)
        } else {
            fill_str.parse()?
        };
        let precs = if let Some(p) = sp.next() {
            Some(p.parse()?)
        } else {
            None
        };
        (Some(f), precs)
    } else {
        (None, None)
    };
    match p.i {
        OIDsingleOrMulti::Single(oid) => {
            aci.acl().require_item_read(&oid)?;
            let payload = StateHistoryPayload {
                i: &oid,
                t_start,
                t_end,
                fill,
                precision,
                limit: p.limit,
                prop: p.prop,
                xopts: &p.xopts,
                compact: true,
            };
            let result: Value = unpack(
                eapi_bus::call(
                    &format!("eva.db.{}", p.database),
                    "state_history",
                    pack(&payload)?.into(),
                )
                .await?
                .payload(),
            )?;
            match p.output_format {
                OutputFormat::List => Ok(result),
                OutputFormat::Dict => {
                    let data = CompactStateHistory::deserialize(result)?;
                    let res: Vec<HistoricalState> = data.into();
                    Ok(to_value(res)?)
                }
            }
        }
        OIDsingleOrMulti::Multi(oids) => {
            if p.output_format != OutputFormat::List {
                return Err(Error::invalid_params(
                    "only output type list is supported for multiple OIDs",
                ));
            }
            if fill.is_some() {
                let mut data: BTreeMap<String, Value> = <_>::default();
                let mut times: Option<Vec<f64>> = None;
                for oid in &oids {
                    aci.acl().require_item_read(oid)?;
                }
                for oid in oids {
                    let payload = StateHistoryPayload {
                        i: &oid,
                        t_start,
                        t_end,
                        fill,
                        precision,
                        limit: p.limit,
                        prop: p.prop,
                        xopts: &p.xopts,
                        compact: true,
                    };
                    let mut result: CompactStateHistory = unpack(
                        eapi_bus::call(
                            &format!("eva.db.{}", p.database),
                            "state_history",
                            pack(&payload)?.into(),
                        )
                        .await?
                        .payload(),
                    )?;
                    if let Some(ref ts) = times {
                        if !result.set_time.is_empty() {
                            match ts.len().cmp(&result.set_time.len()) {
                                std::cmp::Ordering::Greater => {
                                    // invalid dataframe received from the db
                                    // df must always return all requested time series
                                    return Err(Error::invalid_data(ERR_DF_TIMES));
                                }
                                std::cmp::Ordering::Less => {
                                    // received df is longer than the first one, this may be
                                    // possible if the new time serie period has been started
                                    // between requests - simply shrink the received data to match
                                    result.set_time.drain(ts.len()..);
                                    if let Some(ref mut s) = result.status {
                                        if s.len() > ts.len() {
                                            s.drain(ts.len()..);
                                        }
                                    }
                                    if let Some(ref mut v) = result.value {
                                        if v.len() > ts.len() {
                                            v.drain(ts.len()..);
                                        }
                                    }
                                }
                                std::cmp::Ordering::Equal => {}
                            }
                            for (i, t) in result.set_time.into_iter().enumerate() {
                                #[allow(clippy::float_cmp)]
                                if ts[i] != t {
                                    return Err(Error::invalid_data(ERR_DF_TIMES));
                                }
                            }
                        }
                    } else {
                        times.replace(result.set_time);
                    }
                    if let Some(s) = result.status {
                        data.insert(format!("{}/status", oid), Value::Seq(s));
                    }
                    if let Some(v) = result.value {
                        data.insert(format!("{}/value", oid), Value::Seq(v));
                    }
                }
                data.insert("t".to_owned(), to_value(times.unwrap_or_default())?);
                Ok(to_value(data)?)
            } else {
                Err(Error::invalid_params(
                    "fill must be specified for multiple OIDs",
                ))
            }
        }
    }
}

async fn method_item_state_log(params: Value, aci: &mut ACI) -> EResult<Value> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct StateLogParams {
        i: OIDMask,
        #[serde(alias = "s")]
        t_start: Option<Value>,
        #[serde(alias = "e")]
        t_end: Option<Value>,
        #[serde(alias = "w")]
        limit: Option<u32>,
        #[serde(alias = "o", default)]
        xopts: BTreeMap<String, Value>,
        #[serde(alias = "a", default = "get_default_db")]
        database: String,
    }
    #[derive(Serialize)]
    struct StateLogPayload {
        i: OIDMask,
        t_start: Option<f64>,
        t_end: Option<f64>,
        limit: Option<u32>,
        xopts: BTreeMap<String, Value>,
    }
    aci.log_request(log::Level::Debug).await.log_ef();
    let p = StateLogParams::deserialize(params)?;
    aci.acl().require_item_read(&p.i.to_wildcard_oid()?)?;
    let payload = StateLogPayload {
        i: p.i,
        t_start: parse_ts!(p.t_start),
        t_end: parse_ts!(p.t_end),
        limit: p.limit,
        xopts: p.xopts,
    };
    let result: Value = unpack(
        eapi_bus::call(
            &format!("eva.db.{}", p.database),
            "state_log",
            pack(&payload)?.into(),
        )
        .await?
        .payload(),
    )?;
    Ok(result)
}

async fn method_api_log_get(params: Value, aci: &mut ACI) -> EResult<Value> {
    aci.log_request(log::Level::Debug).await.log_ef();
    let mut filter = if params == Value::Unit {
        crate::db::ApiLogFilter::default()
    } else {
        crate::db::ApiLogFilter::deserialize(params)?
    };
    if !crate::public_api_log()
        && !aci.acl().check_admin()
        && !aci.acl().check_op(acl::Op::Moderator)
    {
        if let Some(token) = aci.token() {
            if let Some(ref f_user) = filter.user {
                if token.user() != f_user {
                    return Err(Error::access(
                        "admin access is required to get other users' logs",
                    ));
                }
            } else {
                filter.user = Some(token.user().to_owned());
            }
        } else {
            return Err(Error::access("token authentication required"));
        }
    }
    to_value(crate::aci::log_get(&filter).await?).map_err(Into::into)
}

async fn method_log_get(params: Value, aci: &mut ACI) -> EResult<Value> {
    #[derive(Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct LogGetParams {
        #[serde(alias = "l")]
        level: Option<Value>,
        #[serde(alias = "t")]
        time: Option<u32>,
        #[serde(alias = "n")]
        limit: Option<u32>,
        #[serde(alias = "m", alias = "mod")]
        module: Option<String>,
        #[serde(alias = "x")]
        rx: Option<String>,
    }
    // the params are forwarded to eva.core::log_get as-is
    aci.log_request(log::Level::Debug).await.log_ef();
    aci.acl().require_op(eva_common::acl::Op::Log)?;
    let p = LogGetParams::deserialize(params)?;
    let result: Value = unpack(
        eapi_bus::call("eva.core", "log.get", pack(&p)?.into())
            .await?
            .payload(),
    )?;
    Ok(result)
}

async fn method_action(params: Value, aci: &mut ACI) -> EResult<Value> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct ParamsAction {
        i: OID,
        #[serde(alias = "s")]
        status: Option<ItemStatus>,
        #[serde(alias = "v")]
        value: Value,
        #[serde(default, alias = "p")]
        priority: Option<u8>,
        #[serde(default, alias = "w")]
        wait: Option<f64>,
        note: Option<String>,
    }
    #[derive(Serialize)]
    struct PayloadAction {
        i: OID,
        params: eva_common::actions::Params,
        #[serde(default, alias = "p", skip_serializing_if = "Option::is_none")]
        priority: Option<u8>,
        #[serde(default, alias = "w", skip_serializing_if = "Option::is_none")]
        wait: Option<f64>,
    }
    impl From<ParamsAction> for PayloadAction {
        fn from(p: ParamsAction) -> Self {
            Self {
                i: p.i,
                params: eva_common::actions::Params::new_unit(p.value),
                priority: p.priority,
                wait: p.wait,
            }
        }
    }
    let p_f = prepare_api_filter_params!(params);
    let p = ParamsAction::deserialize(params)?;
    if p.status.is_some() {
        warn!("status field in actions is ignored and deprecated. remove the field from API call payloads");
    }
    aci.log_param("i", &p.i)?;
    if let Some(status) = p.status {
        aci.log_param("status", status)?;
    }
    aci.log_param("value", &p.value)?;
    if let Some(n) = p.note.as_ref() {
        aci.log_note(n);
    }
    aci.log_oid(p.i.clone());
    aci.log_request(log::Level::Info).await.log_ef();
    aci.check_write()?;
    aci.acl().require_item_write(&p.i)?;
    run_api_filter!("action", p_f, aci);
    let mut timeout = eapi_bus::timeout();
    if let Some(w) = p.wait {
        timeout += Duration::from_secs_f64(w);
    }
    let mut result: Value = unpack(
        eapi_bus::call(
            "eva.core",
            "action",
            pack(&Into::<PayloadAction>::into(p))?.into(),
        )
        .await?
        .payload(),
    )?;
    if let Value::Map(ref mut m) = result {
        let uuid_field = Value::String("uuid".to_owned());
        if let Some(u) = m.remove(&uuid_field) {
            let uuid: Uuid = Uuid::deserialize(u)?;
            m.insert(uuid_field, Value::String(uuid.to_string()));
        }
    }
    Ok(result)
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParamsRun {
    pub i: OID,
    #[serde(alias = "a")]
    pub args: Option<Vec<Value>>,
    #[serde(alias = "kw")]
    pub kwargs: Option<HashMap<String, Value>>,
    #[serde(alias = "p")]
    pub priority: Option<u8>,
    #[serde(default, alias = "w")]
    pub wait: Option<f64>,
    #[serde(skip_serializing)]
    pub note: Option<String>,
}

pub async fn method_run(params: Value, aci: &mut ACI) -> EResult<Value> {
    #[derive(Serialize)]
    pub struct PayloadRun {
        i: OID,
        params: eva_common::actions::Params,
        #[serde(alias = "p", skip_serializing_if = "Option::is_none")]
        priority: Option<u8>,
        #[serde(default, alias = "w", skip_serializing_if = "Option::is_none")]
        wait: Option<f64>,
    }
    impl From<ParamsRun> for PayloadRun {
        fn from(p: ParamsRun) -> Self {
            Self {
                i: p.i,
                params: eva_common::actions::Params::new_lmacro(p.args, p.kwargs),
                priority: p.priority,
                wait: p.wait,
            }
        }
    }
    let p_f = prepare_api_filter_params!(params);
    let p = ParamsRun::deserialize(params)?;
    aci.log_param("i", &p.i)?;
    if let Some(n) = p.note.as_ref() {
        aci.log_note(n);
    }
    aci.log_oid(p.i.clone());
    aci.log_request(log::Level::Info).await.log_ef();
    aci.check_write()?;
    aci.acl().require_item_write(&p.i)?;
    run_api_filter!("run", p_f, aci);
    let mut timeout = eapi_bus::timeout();
    if let Some(w) = p.wait {
        timeout += Duration::from_secs_f64(w);
    }
    let mut result: Value = unpack(
        eapi_bus::call(
            "eva.core",
            "run",
            pack(&Into::<PayloadRun>::into(p))?.into(),
        )
        .await?
        .payload(),
    )?;
    if let Value::Map(ref mut m) = result {
        let uuid_field = Value::String("uuid".to_owned());
        if let Some(u) = m.remove(&uuid_field) {
            let uuid: Uuid = Uuid::deserialize(u)?;
            m.insert(uuid_field, Value::String(uuid.to_string()));
        }
    }
    Ok(result)
}

async fn method_action_toggle(params: Value, aci: &mut ACI) -> EResult<Value> {
    #[derive(Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct ParamsAction {
        i: OID,
        #[serde(alias = "p", skip_serializing_if = "Option::is_none")]
        priority: Option<u8>,
        #[serde(default, alias = "w")]
        wait: Option<f64>,
        #[serde(skip_serializing)]
        note: Option<String>,
    }
    let p_f = prepare_api_filter_params!(params);
    let p = ParamsAction::deserialize(params)?;
    aci.log_param("i", &p.i)?;
    if let Some(n) = p.note.as_ref() {
        aci.log_note(n);
    }
    aci.log_oid(p.i.clone());
    aci.log_request(log::Level::Info).await.log_ef();
    aci.check_write()?;
    aci.acl().require_item_write(&p.i)?;
    run_api_filter!("action.toggle", p_f, aci);
    let mut timeout = eapi_bus::timeout();
    if let Some(w) = p.wait {
        timeout += Duration::from_secs_f64(w);
    }
    let mut result: Value = unpack(
        eapi_bus::call("eva.core", "action.toggle", pack(&p)?.into())
            .await?
            .payload(),
    )?;
    if let Value::Map(ref mut m) = result {
        let uuid_field = Value::String("uuid".to_owned());
        if let Some(u) = m.remove(&uuid_field) {
            let uuid: Uuid = Uuid::deserialize(u)?;
            m.insert(uuid_field, Value::String(uuid.to_string()));
        }
    }
    Ok(result)
}

async fn method_action_result(params: Value, aci: &mut ACI) -> EResult<Value> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct ParamsResult {
        u: String,
    }
    #[derive(Serialize)]
    struct PayloadResult {
        u: Uuid,
    }
    aci.log_request(log::Level::Debug).await.log_ef();
    let p = ParamsResult::deserialize(params)?;
    let payload = PayloadResult { u: p.u.parse()? };
    let mut result: BTreeMap<String, Value> = unpack(
        eapi_bus::call("eva.core", "action.result", pack(&payload)?.into())
            .await?
            .payload(),
    )?;
    let uuid_field = "uuid".to_owned();
    if let Some(u) = result.remove(&uuid_field) {
        let uuid: Uuid = Uuid::deserialize(u)?;
        result.insert(uuid_field, Value::String(uuid.to_string()));
    }
    let oid_c = result
        .get("oid")
        .ok_or_else(|| Error::invalid_data("no OID in the result"))?;
    if let Value::String(ref o) = oid_c {
        let oid: OID = o.parse()?;
        aci.acl().require_item_read(&oid)?;
    } else {
        return Err(Error::invalid_data("invalid OID in the result"));
    }
    Ok(to_value(result)?)
}

async fn method_action_terminate(params: Value, aci: &mut ACI) -> EResult<Value> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct ParamsTerminate {
        u: String,
        note: Option<String>,
    }
    #[derive(Serialize)]
    struct PayloadTerminate {
        u: Uuid,
    }
    let p_f = prepare_api_filter_params!(params);
    let p = ParamsTerminate::deserialize(params)?;
    aci.log_param("u", &p.u)?;
    if let Some(n) = p.note.as_ref() {
        aci.log_note(n);
    }
    aci.log_request(log::Level::Warn).await.log_ef();
    aci.check_write()?;
    run_api_filter!("action.terminate", p_f, aci);
    let payload = PayloadTerminate { u: p.u.parse()? };
    let info: Value = unpack(
        eapi_bus::call("eva.core", "action.result", pack(&payload)?.into())
            .await?
            .payload(),
    )?;
    if let Value::Map(ref m) = info {
        let oid_c = m
            .get(&Value::String("oid".to_owned()))
            .ok_or_else(|| Error::invalid_data("no OID in the result"))?;
        if let Value::String(ref o) = oid_c {
            let oid: OID = o.parse()?;
            aci.acl().require_item_write(&oid)?;
        } else {
            return Err(Error::invalid_data("invalid OID in the result"));
        }
    }
    eapi_bus::call("eva.core", "action.terminate", pack(&payload)?.into()).await?;
    ok!()
}

async fn method_action_kill(params: Value, aci: &mut ACI) -> EResult<Value> {
    #[derive(Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct ParamsKill {
        i: OID,
        #[serde(skip_serializing)]
        note: Option<String>,
    }
    let p_f = prepare_api_filter_params!(params);
    let p = ParamsKill::deserialize(params)?;
    aci.log_param("i", &p.i)?;
    if let Some(n) = p.note.as_ref() {
        aci.log_note(n);
    }
    aci.log_oid(p.i.clone());
    aci.log_request(log::Level::Warn).await.log_ef();
    aci.check_write()?;
    aci.acl().require_item_write(&p.i)?;
    run_api_filter!("action.kill", p_f, aci);
    eapi_bus::call("eva.core", "action.kill", pack(&p)?.into()).await?;
    ok!()
}

async fn method_lvar_set(params: Value, aci: &mut ACI) -> EResult<Value> {
    #[derive(Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct ParamsSet {
        i: OID,
        #[serde(alias = "s")]
        status: Option<ItemStatus>,
        #[serde(
            alias = "v",
            default,
            skip_serializing_if = "ValueOptionOwned::is_none"
        )]
        value: ValueOptionOwned,
        #[serde(skip_serializing)]
        note: Option<String>,
    }
    let p_f = prepare_api_filter_params!(params);
    let p = ParamsSet::deserialize(params)?;
    aci.log_param("i", &p.i)?;
    aci.log_param("status", p.status)?;
    aci.log_param("value", &p.value)?;
    if let Some(n) = p.note.as_ref() {
        aci.log_note(n);
    }
    aci.log_oid(p.i.clone());
    aci.log_request(log::Level::Info).await.log_ef();
    aci.check_write()?;
    aci.acl().require_item_write(&p.i)?;
    run_api_filter!("lvar.set", p_f, aci);
    eapi_bus::call("eva.core", "lvar.set", pack(&p)?.into()).await?;
    ok!()
}

async fn method_lvar_reset(params: Value, aci: &mut ACI) -> EResult<Value> {
    #[derive(Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct ParamsReset {
        i: OID,
        #[serde(skip_serializing)]
        note: Option<String>,
    }
    let p_f = prepare_api_filter_params!(params);
    let p = ParamsReset::deserialize(params)?;
    aci.log_param("i", &p.i)?;
    if let Some(n) = p.note.as_ref() {
        aci.log_note(n);
    }
    aci.log_oid(p.i.clone());
    aci.log_request(log::Level::Info).await.log_ef();
    aci.check_write()?;
    aci.acl().require_item_write(&p.i)?;
    run_api_filter!("lvar.reset", p_f, aci);
    eapi_bus::call("eva.core", "lvar.reset", pack(&p)?.into()).await?;
    ok!()
}

async fn method_lvar_clear(params: Value, aci: &mut ACI) -> EResult<Value> {
    #[derive(Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct ParamsClear {
        i: OID,
        #[serde(skip_serializing)]
        note: Option<String>,
    }
    let p_f = prepare_api_filter_params!(params);
    let p = ParamsClear::deserialize(params)?;
    aci.log_param("i", &p.i)?;
    if let Some(n) = p.note.as_ref() {
        aci.log_note(n);
    }
    aci.log_oid(p.i.clone());
    aci.log_request(log::Level::Info).await.log_ef();
    aci.check_write()?;
    aci.acl().require_item_write(&p.i)?;
    run_api_filter!("lvar.clear", p_f, aci);
    eapi_bus::call("eva.core", "lvar.clear", pack(&p)?.into()).await?;
    ok!()
}

async fn method_lvar_toggle(params: Value, aci: &mut ACI) -> EResult<Value> {
    #[derive(Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct ParamsToggle {
        i: OID,
        #[serde(skip_serializing)]
        note: Option<String>,
    }
    let p_f = prepare_api_filter_params!(params);
    let p = ParamsToggle::deserialize(params)?;
    aci.log_param("i", &p.i)?;
    if let Some(n) = p.note.as_ref() {
        aci.log_note(n);
    }
    aci.log_oid(p.i.clone());
    aci.log_request(log::Level::Info).await.log_ef();
    aci.check_write()?;
    aci.acl().require_item_write(&p.i)?;
    run_api_filter!("lvar.toggle", p_f, aci);
    eapi_bus::call("eva.core", "lvar.toggle", pack(&p)?.into()).await?;
    ok!()
}

async fn method_lvar_incr(params: Value, aci: &mut ACI) -> EResult<Value> {
    #[derive(Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct ParamsIncr {
        i: OID,
        #[serde(skip_serializing)]
        note: Option<String>,
    }
    let p_f = prepare_api_filter_params!(params);
    let p = ParamsIncr::deserialize(params)?;
    aci.log_param("i", &p.i)?;
    if let Some(n) = p.note.as_ref() {
        aci.log_note(n);
    }
    aci.log_oid(p.i.clone());
    aci.log_request(log::Level::Info).await.log_ef();
    aci.check_write()?;
    aci.acl().require_item_write(&p.i)?;
    run_api_filter!("lvar.incr", p_f, aci);
    let result: Value = unpack(
        eapi_bus::call("eva.core", "lvar.incr", pack(&p)?.into())
            .await?
            .payload(),
    )?;
    Ok(result)
}

async fn method_lvar_decr(params: Value, aci: &mut ACI) -> EResult<Value> {
    #[derive(Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct ParamsDecr {
        i: OID,
        #[serde(skip_serializing)]
        note: Option<String>,
    }
    let p_f = prepare_api_filter_params!(params);
    let p = ParamsDecr::deserialize(params)?;
    aci.log_param("i", &p.i)?;
    if let Some(n) = p.note.as_ref() {
        aci.log_note(n);
    }
    aci.log_oid(p.i.clone());
    aci.log_request(log::Level::Info).await.log_ef();
    aci.check_write()?;
    run_api_filter!("lvar.decr", p_f, aci);
    aci.acl().require_item_write(&p.i)?;
    let result: Value = unpack(
        eapi_bus::call("eva.core", "lvar.decr", pack(&p)?.into())
            .await?
            .payload(),
    )?;
    Ok(result)
}

async fn method_bus_call(
    target: &str,
    method: &str,
    params: Value,
    aci: &mut ACI,
) -> EResult<Value> {
    aci.log_request(log::Level::Debug).await.log_ef();
    aci.check_write()?;
    aci.acl().require_admin()?;
    let p_f = prepare_api_filter_params!(params);
    run_api_filter!(&format!("bus::{}::{}", target, method), p_f, aci);
    let payload: busrt::borrow::Cow = if let Value::Map(ref m) = params {
        if m.is_empty() {
            busrt::empty_payload!()
        } else {
            pack(&params)?.into()
        }
    } else {
        pack(&params)?.into()
    };
    let res = eapi_bus::call(target, method, payload).await?;
    let payload = res.payload();
    if payload.is_empty() {
        Ok(Value::Unit)
    } else {
        Ok(unpack(payload)?)
    }
}

async fn method_x_call(target: &str, method: &str, params: Value, aci: &mut ACI) -> EResult<Value> {
    #[derive(Serialize)]
    struct XPayload<'a> {
        method: &'a str,
        params: Value,
        aci: &'a ACI,
        acl: &'a Acl,
    }
    aci.log_request(log::Level::Debug).await.log_ef();
    let p_f = prepare_api_filter_params!(params);
    run_api_filter!(&format!("x::{}::{}", target, method), p_f, aci);
    let payload = XPayload {
        method,
        params,
        aci,
        acl: aci.acl(),
    };
    let res = eapi_bus::call(target, "x", pack(&payload)?.into()).await?;
    let payload = res.payload();
    if payload.is_empty() {
        Ok(Value::Unit)
    } else {
        Ok(unpack(payload)?)
    }
}

async fn method_session_set_readonly(params: Value, aci: &mut ACI) -> EResult<Value> {
    aci.log_request(log::Level::Debug).await.log_ef();
    ParamsEmpty::deserialize(params)?;
    if let Some(token) = aci.token() {
        token.set_readonly().await?;
        to_value(token).map_err(Into::into)
    } else {
        Err(Error::failed("no token provided"))
    }
}

async fn method_ehmi_create_config(
    key: &str,
    params: Value,
    aci: &mut ACI,
    source: &str,
) -> EResult<Value> {
    #[derive(Serialize)]
    struct AppPayload {
        token: String,
        api_version: u16,
        config: BTreeMap<String, Value>,
    }
    #[derive(Serialize)]
    struct App {
        ehmi_app_url: String,
    }
    aci.log_request(log::Level::Debug).await.log_ef();
    let mut hasher = openssl::sha::Sha256::new();
    hasher.update(source.as_bytes());
    hasher.update(key.as_bytes());
    hasher.update(&pack(&params)?);
    let config_key = BASE64.encode(hasher.finish());
    let mut config: BTreeMap<String, Value> = BTreeMap::deserialize(params)?;
    let app_id = config
        .remove("app")
        .map_or_else(|| "ec".to_owned(), |v| v.to_string());
    config.remove("uid");
    let Some(Value::String(hmi_url)) = config.remove("hmi_url") else {
        return Err(Error::invalid_params("hmi_url not provided"));
    };
    let _ehmi_lock = EHMI_LOCK.lock().await;
    let mut token_short_id = if let Some(ti) = db::ehmi_config_token_id(&config_key).await? {
        let token_id = aaa::TokenId::try_from(ti.as_str())?;
        if aaa::get_token(token_id, None).await.is_ok() {
            Some(ti)
        } else {
            None
        }
    } else {
        None
    };
    if token_short_id.is_none() {
        let token = if key.starts_with("token:") {
            if let Auth::Token(token) = aaa::authenticate(key, None).await? {
                token
            } else {
                return Err(Error::core("auth token failed"));
            }
        } else {
            login_key(key, None, source).await?
        };
        let payload = AppPayload {
            token: token.id().to_string(),
            api_version: API_VERSION,
            config,
        };
        let ti = token.id().to_short_string();
        db::insert_ehmi_config(&ti, &config_key, payload).await?;
        token_short_id.replace(ti);
    }
    let mut hasher = openssl::sha::Sha256::new();
    hasher.update(token_short_id.unwrap().as_bytes());
    let uid = BASE64.encode(hasher.finish());
    let app = App {
        ehmi_app_url: format!(
            "{}/ui/ehmi/{}/?ck={}&uid={}",
            hmi_url.trim_end_matches('/'),
            app_id,
            config_key,
            uid
        ),
    };
    Ok(to_value(app)?)
}

async fn method_ehmi_get_config(params: Value) -> EResult<Value> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Params {
        ck: String,
    }
    let p = Params::deserialize(params)?;
    db::get_ehmi_config(&p.ck).await
}

async fn method_user_data_get(params: Value, aci: &mut ACI) -> EResult<Value> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Params {
        key: String,
    }
    #[derive(Serialize)]
    struct Response {
        value: Value,
    }
    aci.log_request(log::Level::Trace).await.log_ef();
    let p = Params::deserialize(params)?;
    let Some(token) = aci.token() else {
        return Err(Error::access("not authenticated as a user"));
    };
    Ok(to_value(Response {
        value: db::get_user_data(token.user(), &p.key).await?,
    })?)
}

async fn method_user_data_set(params: Value, aci: &mut ACI) -> EResult<Value> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Params {
        key: String,
        value: Value,
    }
    aci.log_request(log::Level::Trace).await.log_ef();
    let p = Params::deserialize(params)?;
    let Some(token) = aci.token() else {
        return Err(Error::access("not authenticated as a user"));
    };
    db::set_user_data(token.user(), &p.key, p.value).await?;
    ok!()
}

async fn method_user_data_delete(params: Value, aci: &mut ACI) -> EResult<Value> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Params {
        key: String,
    }
    aci.log_request(log::Level::Trace).await.log_ef();
    let p = Params::deserialize(params)?;
    let Some(token) = aci.token() else {
        return Err(Error::access("not authenticated as a user"));
    };
    db::delete_user_data(token.user(), &p.key).await?;
    ok!()
}

async fn method_db_list(params: Value, aci: &mut ACI) -> EResult<Value> {
    #[derive(Serialize)]
    struct Params {
        filter: &'static str,
    }
    #[derive(Deserialize)]
    struct SvcId {
        id: String,
        enabled: bool,
    }
    #[derive(Serialize)]
    struct DbInfo<'a> {
        id: &'a str,
        default: bool,
    }
    aci.log_request(log::Level::Debug).await.log_ef();
    ParamsEmpty::deserialize(params)?;
    let svc_list: Vec<SvcId> = unpack(
        eapi_bus::call(
            "eva.core",
            "svc.list",
            pack(&Params {
                filter: "^eva\\.db\\..*$",
            })?
            .into(),
        )
        .await?
        .payload(),
    )?;
    let default_db = get_default_db();
    let dbs: Vec<DbInfo> = svc_list
        .iter()
        .filter_map(|svc| {
            if svc.enabled {
                svc.id.strip_prefix("eva.db.").map(|id| DbInfo {
                    id,
                    default: id == default_db,
                })
            } else {
                None
            }
        })
        .collect();
    to_value(dbs).map_err(Into::into)
}

async fn method_pvt_put(params: Value, aci: &mut ACI) -> EResult<Value> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Params {
        #[serde(alias = "i")]
        path: String,
        #[serde(alias = "c")]
        content: String,
    }
    let p_f = prepare_api_filter_params!(params);
    let p = Params::deserialize(params)?;
    aci.log_param("path", &p.path)?;
    aci.log_request(log::Level::Info).await.log_ef();
    aci.check_write()?;
    run_api_filter!("pvt.put", p_f, aci);
    let path = format_and_check_path(&p.path, aci.acl(), PvtOp::Write)?;
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let mut fh = tokio::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&path)
        .await?;
    fh.write_all(p.content.as_bytes()).await?;
    fh.flush().await?;
    ok!()
}

async fn method_pvt_get(params: Value, aci: &mut ACI) -> EResult<Value> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Params {
        #[serde(alias = "i")]
        path: String,
    }
    #[derive(Serialize)]
    struct Payload {
        content: String,
    }
    aci.log_request(log::Level::Debug).await.log_ef();
    let p = Params::deserialize(params)?;
    let path = format_and_check_path(&p.path, aci.acl(), PvtOp::Read)?;
    let content = match tokio::fs::read_to_string(path).await {
        Ok(v) => v,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(Error::not_found(p.path)),
        Err(e) => return Err(e.into()),
    };
    to_value(Payload { content }).map_err(Into::into)
}

async fn method_pvt_unlink(params: Value, aci: &mut ACI) -> EResult<Value> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Params {
        #[serde(alias = "i")]
        path: String,
    }
    let p_f = prepare_api_filter_params!(params);
    let p = Params::deserialize(params)?;
    aci.log_param("path", &p.path)?;
    aci.log_request(log::Level::Info).await.log_ef();
    aci.check_write()?;
    run_api_filter!("pvt.unlink", p_f, aci);
    let path = format_and_check_path(&p.path, aci.acl(), PvtOp::Write)?;
    if let Err(e) = tokio::fs::remove_file(path).await {
        if e.kind() == std::io::ErrorKind::NotFound {
            return Err(Error::not_found(p.path));
        }
        return Err(e.into());
    }
    ok!()
}

async fn method_pvt_list(params: Value, aci: &mut ACI) -> EResult<Value> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Params {
        #[serde(alias = "i")]
        path: String,
        #[serde(alias = "m")]
        masks: Option<ValueOrList<String>>,
        #[serde(default, alias = "p")]
        kind: sdkfs::Kind,
        #[serde(default, alias = "r")]
        recursive: bool,
    }
    aci.log_request(log::Level::Debug).await.log_ef();
    let p = Params::deserialize(params)?;
    let path = format_and_check_path(&p.path, aci.acl(), PvtOp::List)?;
    let masks: Vec<String> = if let Some(m) = p.masks {
        m.to_vec()
    } else {
        vec!["*".to_owned()]
    };
    let entries = match sdkfs::list(
        &path,
        &masks.iter().map(String::as_str).collect::<Vec<&str>>(),
        p.kind,
        p.recursive,
        true,
    )
    .await
    {
        Ok(v) => v,
        Err(e) if e.kind() == ErrorKind::ResourceNotFound => return Err(Error::not_found(p.path)),
        Err(e) => return Err(e),
    };
    let s = path.to_string_lossy();
    let plen = crate::PVT_PATH
        .get()
        .unwrap()
        .as_ref()
        .map_or(0, |v| v.len() + 1);
    let result: Vec<sdkfs::Entry> = entries
        .into_iter()
        .filter_map(|r| {
            let p = r.path.to_string_lossy();
            if plen < p.len() && aci.acl().check_pvt_read(&p[plen..]) && s.len() + 1 < p.len() {
                return Some(sdkfs::Entry {
                    path: Path::new(&p[s.len() + 1..]).to_owned(),
                    meta: r.meta,
                    kind: r.kind,
                });
            }
            None
        })
        .collect();
    to_value(result).map_err(Into::into)
}
