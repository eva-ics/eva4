use crate::{
    HMI_SVC, REAL_IP_HEADER, RPC, TIMEOUT,
    http_errors::{HttpError, convert_error, internal_error},
};
use axum::extract::{ConnectInfo, FromRequestParts};
use axum::http::{StatusCode, request::Parts};
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

impl<S: Send + Sync> FromRequestParts<S> for ClientInfo {
    type Rejection = HttpError;

    fn from_request_parts(
        parts: &mut Parts,
        _state: &S,
    ) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        let prepared: Result<(String, Option<IpAddr>), HttpError> = (|| {
            let ip = get_client_ip(parts)?;
            let key = parts
                .headers
                .get("x-auth-key")
                .ok_or((
                    StatusCode::FORBIDDEN,
                    "auth key not specified (AUTH)".to_owned(),
                ))?
                .to_str()
                .map_err(internal_error)?
                .to_owned();
            Ok((key, ip))
        })();

        async move {
            let (key, ip) = prepared?;
            #[derive(Serialize)]
            struct Payload {
                key: String,
                ip: Option<String>,
            }
            let acl: eva_common::acl::Acl = unpack(
                safe_rpc_call(
                    RPC.get().unwrap(),
                    HMI_SVC.get().unwrap(),
                    "authenticate",
                    pack(&Payload {
                        key,
                        ip: ip.map(|addr| addr.to_string()),
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
        }
    }
}
