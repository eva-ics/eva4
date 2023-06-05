use crate::aaa::{self, Auth, Token};
use crate::aci::ACI;
use crate::{RPC, TIMEOUT};
use busrt::QoS;
use eva_common::acl::{self, Acl, OIDMask};
use eva_common::common_payloads::ParamsIdOrListOwned;
use eva_common::prelude::*;
use eva_sdk::prelude::*;
use eva_sdk::types::{CompactStateHistory, Fill, HistoricalState, StateProp};
use log::{error, trace};
use rjrpc::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

err_logger!();

const ERR_NO_PARAMS: &str = "no params provided";
const ERR_NO_METHOD: &str = "no such RPC method";
const ERR_DF_TIMES: &str = "dataframe times mismatch";

const API_VERSION: u16 = 4;

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
    if let Some(creds) = meta.credentials() {
        let source = ip.map_or_else(|| "-".to_owned(), |v| v.to_string());
        login(&creds.0, &creds.1, None, None, ip, &source).await
    } else {
        if let Some(agent) = meta.agent() {
            if agent.starts_with("evaHI ") {
                return Err(Error::new0(ErrorKind::EvaHIAuthenticationRequired));
            }
        }
        Err(Error::access("No user name / password provided"))
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
    let rpc = RPC.get().unwrap();
    let timeout = *TIMEOUT.get().unwrap();
    let payload = pack(&AuthPayload {
        login,
        password,
        timeout: timeout.as_secs_f64(),
        xopts,
    })?;
    let mut aci = ACI::new(
        Auth::Login(login.to_owned(), None),
        "login",
        source.to_owned(),
    );
    for svc in auth_svcs {
        match safe_rpc_call(
            rpc,
            svc,
            "auth.user",
            payload.as_slice().into(),
            QoS::Processed,
            timeout,
        )
        .await
        {
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
    struct Params {
        q: String,
    }
    let p = Params::deserialize(params)?;
    let (call_method, call_params) = crate::call_parser::parse_call_str(&p.q)?;
    if call_method == "call" {
        Err(Error::new(ErrorKind::MethodNotFound, ERR_NO_METHOD))
    } else {
        call(&call_method, Some(call_params), meta).await
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ParamsEmpty {}

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
    let source = ip.map_or_else(|| "-".to_owned(), |v| v.to_string());
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
    }
    aci.log_request(log::Level::Debug).await.log_ef();
    ParamsEmpty::deserialize(params)?;
    let mut info: TestInfo = unpack(
        safe_rpc_call(
            RPC.get().unwrap(),
            "eva.core",
            "test",
            busrt::empty_payload!(),
            QoS::Processed,
            *TIMEOUT.get().unwrap(),
        )
        .await?
        .payload(),
    )?;
    info.aci.replace(aci);
    info.acl.replace(aci.acl());
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
    }
    aci.log_request(log::Level::Warn).await.log_ef();
    if let Some(token) = aci.token() {
        aci.check_write()?;
        let p = ParamsSetPassword::deserialize(params)?;
        let rpc = RPC.get().unwrap();
        let timeout = *TIMEOUT.get().unwrap();
        // verify old password
        let payload = pack(&AuthPayload {
            login: token.user(),
            password: &p.current_password,
            timeout: timeout.as_secs_f64(),
        })?;
        safe_rpc_call(
            rpc,
            token.auth_svc(),
            "auth.user",
            payload.as_slice().into(),
            QoS::Processed,
            timeout,
        )
        .await?;
        let payload = pack(&PayloadIdPass {
            i: token.user(),
            password: &p.password,
        })?;
        safe_rpc_call(
            rpc,
            token.auth_svc(),
            "user.set_password",
            payload.as_slice().into(),
            QoS::Processed,
            timeout,
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
        safe_rpc_call(
            RPC.get().unwrap(),
            "eva.core",
            "item.state",
            pack(&payload)?.into(),
            QoS::Processed,
            *TIMEOUT.get().unwrap(),
        )
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
            let dots: u32 = x.parse()?;
            if dots > 10_000 {
                return Err(Error::invalid_params("too many dots (max 10k)"));
            }
            if dots < 2 {
                return Err(Error::invalid_params("dots can not be less than 2"));
            }
            let mut period = (t_end - t_start) / f64::from(dots - 1);
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
                safe_rpc_call(
                    RPC.get().unwrap(),
                    &format!("eva.db.{}", p.database),
                    "state_history",
                    pack(&payload)?.into(),
                    QoS::Processed,
                    *TIMEOUT.get().unwrap(),
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
                        safe_rpc_call(
                            RPC.get().unwrap(),
                            &format!("eva.db.{}", p.database),
                            "state_history",
                            pack(&payload)?.into(),
                            QoS::Processed,
                            *TIMEOUT.get().unwrap(),
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
        safe_rpc_call(
            RPC.get().unwrap(),
            &format!("eva.db.{}", p.database),
            "state_log",
            pack(&payload)?.into(),
            QoS::Processed,
            *TIMEOUT.get().unwrap(),
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
        safe_rpc_call(
            RPC.get().unwrap(),
            "eva.core",
            "log.get",
            pack(&p)?.into(),
            QoS::Processed,
            *TIMEOUT.get().unwrap(),
        )
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
        status: ItemStatus,
        #[serde(default, alias = "v")]
        value: ValueOptionOwned,
        #[serde(default, alias = "p")]
        priority: Option<u8>,
        #[serde(default, alias = "w")]
        wait: Option<f64>,
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
                params: eva_common::actions::Params::new_unit(p.status, p.value.into()),
                priority: p.priority,
                wait: p.wait,
            }
        }
    }
    let p = ParamsAction::deserialize(params)?;
    aci.log_param("i", &p.i)?;
    aci.log_param("status", p.status)?;
    aci.log_param("value", &p.value)?;
    aci.log_request(log::Level::Info).await.log_ef();
    aci.check_write()?;
    aci.acl().require_item_write(&p.i)?;
    let mut timeout = *TIMEOUT.get().unwrap();
    if let Some(w) = p.wait {
        timeout += Duration::from_secs_f64(w);
    }
    let mut result: Value = unpack(
        safe_rpc_call(
            RPC.get().unwrap(),
            "eva.core",
            "action",
            pack(&Into::<PayloadAction>::into(p))?.into(),
            QoS::RealtimeProcessed,
            *TIMEOUT.get().unwrap(),
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
    let p = ParamsRun::deserialize(params)?;
    aci.log_param("i", &p.i)?;
    aci.log_request(log::Level::Info).await.log_ef();
    aci.check_write()?;
    aci.acl().require_item_write(&p.i)?;
    let mut timeout = *TIMEOUT.get().unwrap();
    if let Some(w) = p.wait {
        timeout += Duration::from_secs_f64(w);
    }
    let mut result: Value = unpack(
        safe_rpc_call(
            RPC.get().unwrap(),
            "eva.core",
            "run",
            pack(&Into::<PayloadRun>::into(p))?.into(),
            QoS::RealtimeProcessed,
            *TIMEOUT.get().unwrap(),
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
    }
    let p = ParamsAction::deserialize(params)?;
    aci.log_param("i", &p.i)?;
    aci.log_request(log::Level::Info).await.log_ef();
    aci.check_write()?;
    aci.acl().require_item_write(&p.i)?;
    let mut timeout = *TIMEOUT.get().unwrap();
    if let Some(w) = p.wait {
        timeout += Duration::from_secs_f64(w);
    }
    let mut result: Value = unpack(
        safe_rpc_call(
            RPC.get().unwrap(),
            "eva.core",
            "action.toggle",
            pack(&p)?.into(),
            QoS::RealtimeProcessed,
            *TIMEOUT.get().unwrap(),
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
        safe_rpc_call(
            RPC.get().unwrap(),
            "eva.core",
            "action.result",
            pack(&payload)?.into(),
            QoS::Processed,
            *TIMEOUT.get().unwrap(),
        )
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
    }
    #[derive(Serialize)]
    struct PayloadTerminate {
        u: Uuid,
    }
    let p = ParamsTerminate::deserialize(params)?;
    aci.log_param("u", &p.u)?;
    aci.log_request(log::Level::Warn).await.log_ef();
    aci.check_write()?;
    let payload = PayloadTerminate { u: p.u.parse()? };
    let info: Value = unpack(
        safe_rpc_call(
            RPC.get().unwrap(),
            "eva.core",
            "action.result",
            pack(&payload)?.into(),
            QoS::RealtimeProcessed,
            *TIMEOUT.get().unwrap(),
        )
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
    safe_rpc_call(
        RPC.get().unwrap(),
        "eva.core",
        "action.terminate",
        pack(&payload)?.into(),
        QoS::RealtimeProcessed,
        *TIMEOUT.get().unwrap(),
    )
    .await?;
    ok!()
}

async fn method_action_kill(params: Value, aci: &mut ACI) -> EResult<Value> {
    #[derive(Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct ParamsKill {
        i: OID,
    }
    let p = ParamsKill::deserialize(params)?;
    aci.log_param("i", &p.i)?;
    aci.log_request(log::Level::Warn).await.log_ef();
    aci.check_write()?;
    aci.acl().require_item_write(&p.i)?;
    safe_rpc_call(
        RPC.get().unwrap(),
        "eva.core",
        "action.kill",
        pack(&p)?.into(),
        QoS::RealtimeProcessed,
        *TIMEOUT.get().unwrap(),
    )
    .await?;
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
    }
    let p = ParamsSet::deserialize(params)?;
    aci.log_param("i", &p.i)?;
    aci.log_param("status", p.status)?;
    aci.log_param("value", &p.value)?;
    aci.log_request(log::Level::Info).await.log_ef();
    aci.check_write()?;
    aci.acl().require_item_write(&p.i)?;
    safe_rpc_call(
        RPC.get().unwrap(),
        "eva.core",
        "lvar.set",
        pack(&p)?.into(),
        QoS::RealtimeProcessed,
        *TIMEOUT.get().unwrap(),
    )
    .await?;
    ok!()
}

async fn method_lvar_reset(params: Value, aci: &mut ACI) -> EResult<Value> {
    #[derive(Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct ParamsReset {
        i: OID,
    }
    let p = ParamsReset::deserialize(params)?;
    aci.log_param("i", &p.i)?;
    aci.log_request(log::Level::Info).await.log_ef();
    aci.check_write()?;
    aci.acl().require_item_write(&p.i)?;
    safe_rpc_call(
        RPC.get().unwrap(),
        "eva.core",
        "lvar.reset",
        pack(&p)?.into(),
        QoS::RealtimeProcessed,
        *TIMEOUT.get().unwrap(),
    )
    .await?;
    ok!()
}

async fn method_lvar_clear(params: Value, aci: &mut ACI) -> EResult<Value> {
    #[derive(Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct ParamsClear {
        i: OID,
    }
    let p = ParamsClear::deserialize(params)?;
    aci.log_param("i", &p.i)?;
    aci.log_request(log::Level::Info).await.log_ef();
    aci.check_write()?;
    aci.acl().require_item_write(&p.i)?;
    safe_rpc_call(
        RPC.get().unwrap(),
        "eva.core",
        "lvar.clear",
        pack(&p)?.into(),
        QoS::RealtimeProcessed,
        *TIMEOUT.get().unwrap(),
    )
    .await?;
    ok!()
}

async fn method_lvar_toggle(params: Value, aci: &mut ACI) -> EResult<Value> {
    #[derive(Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct ParamsToggle {
        i: OID,
    }
    let p = ParamsToggle::deserialize(params)?;
    aci.log_param("i", &p.i)?;
    aci.log_request(log::Level::Info).await.log_ef();
    aci.check_write()?;
    aci.acl().require_item_write(&p.i)?;
    safe_rpc_call(
        RPC.get().unwrap(),
        "eva.core",
        "lvar.toggle",
        pack(&p)?.into(),
        QoS::RealtimeProcessed,
        *TIMEOUT.get().unwrap(),
    )
    .await?;
    ok!()
}

async fn method_lvar_incr(params: Value, aci: &mut ACI) -> EResult<Value> {
    #[derive(Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct ParamsIncr {
        i: OID,
    }
    let p = ParamsIncr::deserialize(params)?;
    aci.log_param("i", &p.i)?;
    aci.log_request(log::Level::Info).await.log_ef();
    aci.check_write()?;
    aci.acl().require_item_write(&p.i)?;
    let result: Value = unpack(
        safe_rpc_call(
            RPC.get().unwrap(),
            "eva.core",
            "lvar.incr",
            pack(&p)?.into(),
            QoS::RealtimeProcessed,
            *TIMEOUT.get().unwrap(),
        )
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
    }
    let p = ParamsDecr::deserialize(params)?;
    aci.log_param("i", &p.i)?;
    aci.log_request(log::Level::Info).await.log_ef();
    aci.check_write()?;
    aci.acl().require_item_write(&p.i)?;
    let result: Value = unpack(
        safe_rpc_call(
            RPC.get().unwrap(),
            "eva.core",
            "lvar.decr",
            pack(&p)?.into(),
            QoS::RealtimeProcessed,
            *TIMEOUT.get().unwrap(),
        )
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
    let payload: busrt::borrow::Cow = if let Value::Map(ref m) = params {
        if m.is_empty() {
            busrt::empty_payload!()
        } else {
            pack(&params)?.into()
        }
    } else {
        pack(&params)?.into()
    };
    let res = safe_rpc_call(
        RPC.get().unwrap(),
        target,
        method,
        payload,
        QoS::RealtimeProcessed,
        *TIMEOUT.get().unwrap(),
    )
    .await?;
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
    let payload = XPayload {
        method,
        params,
        aci,
        acl: aci.acl(),
    };
    let res = safe_rpc_call(
        RPC.get().unwrap(),
        target,
        "x",
        pack(&payload)?.into(),
        QoS::RealtimeProcessed,
        *TIMEOUT.get().unwrap(),
    )
    .await?;
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
