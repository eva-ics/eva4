use crate::aaa;
use eva_common::common_payloads::ParamsIdOwned;
use eva_common::events::{
    LocalStateEvent, RemoteStateEvent, AAA_ACL_TOPIC, AAA_KEY_TOPIC, AAA_USER_TOPIC,
    LOCAL_STATE_TOPIC, LOG_EVENT_TOPIC, REMOTE_STATE_TOPIC,
};
use eva_common::prelude::*;
use eva_sdk::prelude::*;
use eva_sdk::types::FullRemoteItemState;
use serde::{Deserialize, Serialize};
use std::time::Duration;

err_logger!();

pub struct Handlers {
    info: ServiceInfo,
}

impl Handlers {
    pub fn new(info: ServiceInfo) -> Self {
        Self { info }
    }
}

#[async_trait::async_trait]
impl RpcHandlers for Handlers {
    #[allow(clippy::too_many_lines)]
    async fn handle_call(&self, event: RpcEvent) -> RpcResult {
        svc_rpc_need_ready!();
        let method = event.parse_method()?;
        let payload = event.payload();
        match method {
            "tpl.reload" => {
                if payload.is_empty() {
                    if let Some(ui_path) = crate::UI_PATH.get().unwrap() {
                        let _r =
                            tokio::task::spawn_blocking(move || crate::tpl::reload_ui(ui_path))
                                .await
                                .map_err(Error::core)?
                                .log_err()
                                .map_err(Error::failed)?;
                    }
                    if let Some(pvt_path) = crate::PVT_PATH.get().unwrap() {
                        let _r =
                            tokio::task::spawn_blocking(move || crate::tpl::reload_pvt(pvt_path))
                                .await
                                .map_err(Error::core)?
                                .log_err()
                                .map_err(Error::failed)?;
                    }
                    Ok(None)
                } else {
                    Err(RpcError::params(None))
                }
            }
            "i18n.cache_purge" => {
                if payload.is_empty() {
                    crate::I18N.get().unwrap().cache_purge();
                    Ok(None)
                } else {
                    Err(RpcError::params(None))
                }
            }
            "api_log.get" => {
                let filter = if payload.is_empty() {
                    crate::db::ApiLogFilter::default()
                } else {
                    unpack(payload)?
                };
                Ok(Some(pack(&crate::aci::log_get(&filter).await?)?))
            }
            "ws.stats" => {
                #[derive(Serialize)]
                struct Stats {
                    clients: usize,
                    sub_clients: usize,
                    subscriptions: usize,
                }
                if payload.is_empty() {
                    let sm = crate::handler::WS_SUB.lock();
                    let stats = Stats {
                        clients: sm.list_clients().len(),
                        sub_clients: sm.client_count(),
                        subscriptions: sm.subscription_count(),
                    };
                    Ok(Some(pack(&stats)?))
                } else {
                    Err(RpcError::params(None))
                }
            }
            "session.broadcast.reload" => {
                if payload.is_empty() {
                    crate::handler::notify_need_reload();
                    Ok(None)
                } else {
                    Err(RpcError::params(None))
                }
            }
            "session.broadcast.restart" => {
                if payload.is_empty() {
                    crate::handler::notify_server_event(crate::handler::ServerEvent::Restart);
                    Ok(None)
                } else {
                    Err(RpcError::params(None))
                }
            }
            "session.list" => {
                #[derive(Serialize, bmart::tools::Sorting)]
                #[sorting(id = "expires_in")]
                struct TokenInfo<'a> {
                    id: String,
                    mode: &'a str,
                    user: &'a str,
                    source: Option<String>,
                    #[serde(serialize_with = "eva_common::tools::serialize_duration_as_u64")]
                    expires_in: Duration,
                }
                if payload.is_empty() {
                    let tokens = crate::aaa::get_tokens().await?;
                    let mut token_info: Vec<TokenInfo> = tokens
                        .iter()
                        .map(|t| TokenInfo {
                            id: t.id().to_string(),
                            mode: t.mode_as_str(),
                            user: t.user(),
                            source: t.ip().map(ToString::to_string),
                            expires_in: t.expires_in().unwrap_or_default(),
                        })
                        .collect();
                    token_info.sort();
                    Ok(Some(pack(&token_info)?))
                } else {
                    Err(RpcError::params(None))
                }
            }
            "session.destroy" => {
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    let p: ParamsIdOwned = unpack(payload)?;
                    crate::aaa::destroy_token(&(p.i.try_into()?)).await?;
                    Ok(None)
                }
            }
            "authenticate" => {
                #[derive(Deserialize)]
                struct AuthInfo {
                    key: String,
                    ip: Option<String>,
                }
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    let p: AuthInfo = unpack(payload)?;
                    let ip = if let Some(ip_str) = p.ip {
                        Some(ip_str.parse().map_err(Error::invalid_params)?)
                    } else {
                        None
                    };
                    let auth = aaa::authenticate(&p.key, ip).await?;
                    Ok(Some(pack(auth.acl())?))
                }
            }
            _ => svc_handle_default_rpc(method, &self.info),
        }
    }
    async fn handle_notification(&self, _event: RpcEvent) {}
    async fn handle_frame(&self, frame: Frame) {
        if frame.kind() == busrt::FrameKind::Publish {
            if let Some(topic) = frame.topic() {
                if let Some(o) = topic.strip_prefix(LOCAL_STATE_TOPIC) {
                    process_state(topic, o, frame.payload(), true).log_ef();
                } else if let Some(o) = topic.strip_prefix(REMOTE_STATE_TOPIC) {
                    process_state(topic, o, frame.payload(), false).log_ef();
                } else if let Some(o) = topic.strip_prefix(LOG_EVENT_TOPIC) {
                    process_log(topic, o, frame.payload());
                } else if let Some(acl_id) = topic.strip_prefix(AAA_ACL_TOPIC) {
                    debug!("deleting tokens/web sockets which contain ACL {}", acl_id);
                    crate::aaa::clear_tokens_by_acl_id(acl_id);
                    crate::handler::remove_websocket_by_acl_id(acl_id).await;
                } else if let Some(key_id) = topic.strip_prefix(AAA_KEY_TOPIC) {
                    debug!("deleting tokens/web sockets for API key {}", key_id);
                    crate::handler::remove_websocket_by_key_id(key_id).await;
                } else if let Some(user) = topic.strip_prefix(AAA_USER_TOPIC) {
                    debug!("deleting tokens which belong to user {}", user);
                    crate::aaa::clear_tokens_by_user(user);
                }
            }
        }
    }
}

fn process_state(topic: &str, path: &str, payload: &[u8], local: bool) -> EResult<()> {
    match OID::from_path(path) {
        Ok(oid) => {
            if local {
                match unpack::<LocalStateEvent>(payload) {
                    Ok(v) => {
                        crate::handler::notify_state(FullRemoteItemState::from_local_state_event(
                            v,
                            oid,
                            crate::SYSTEM_NAME.get().unwrap(),
                        ))?;
                    }
                    Err(e) => {
                        warn!("invalid local state event payload {}: {}", topic, e);
                    }
                }
            } else {
                match unpack::<RemoteStateEvent>(payload) {
                    Ok(v) => {
                        crate::handler::notify_state(
                            FullRemoteItemState::from_remote_state_event(v, oid),
                        )?;
                    }
                    Err(e) => {
                        warn!("invalid remote state event payload {}: {}", topic, e);
                    }
                }
            }
        }
        Err(e) => warn!("invalid OID in state event {}: {}", topic, e),
    }
    Ok(())
}

fn process_log(topic: &str, path: &str, payload: &[u8]) {
    match unpack::<Value>(payload) {
        Ok(v) => {
            crate::handler::notify_log(path, v);
        }
        Err(e) => {
            warn!("invalid local state event payload {}: {}", topic, e);
        }
    }
}
