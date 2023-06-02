use crate::{
    http_errors::{convert_error, internal_error, HttpError},
    HMI_SVC, REAL_IP_HEADER, RPC, TIMEOUT,
};
use axum::extract::{ConnectInfo, FromRequestParts};
use axum::http::{request::Parts, StatusCode};
use eva_sdk::prelude::*;
use serde::Serialize;
use std::net::IpAddr;
use std::net::SocketAddr;

fn get_client_ip(parts: &mut Parts) -> Result<Option<IpAddr>, HttpError> {
    if let Some(h) = REAL_IP_HEADER.get() {
        if let Some(v) = parts.headers.get(h) {
            return Ok(Some(
                v.to_str()
                    .map_err(internal_error)?
                    .parse::<IpAddr>()
                    .map_err(internal_error)?,
            ));
        }
    }
    let ip = parts
        .extensions
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(addr)| addr.ip());
    Ok(ip)
}

pub struct ClientInfo {
    pub acl: eva_common::acl::Acl,
}

#[axum::async_trait]
impl<S> FromRequestParts<S> for ClientInfo {
    type Rejection = HttpError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        #[derive(Serialize)]
        struct Payload<'a> {
            key: &'a str,
            ip: Option<String>,
        }
        let ip = get_client_ip(parts)?;
        if let Some(key) = parts.headers.get("x-auth-key") {
            let acl: eva_common::acl::Acl = unpack(
                safe_rpc_call(
                    RPC.get().unwrap(),
                    HMI_SVC.get().unwrap(),
                    "authenticate",
                    pack(&Payload {
                        key: key.to_str().map_err(internal_error)?,
                        ip: ip.map(|v| v.to_string()),
                    })
                    .map_err(internal_error)?
                    .into(),
                    QoS::Processed,
                    *TIMEOUT.get().unwrap(),
                )
                .await
                .map_err(|e| convert_error(e, ip.as_ref(), true))?
                .payload(),
            )
            .map_err(internal_error)?;
            Ok(ClientInfo { acl })
        } else {
            Err((
                StatusCode::FORBIDDEN,
                "auth key not specified (AUTH)".to_owned(),
            ))
        }
    }
}
