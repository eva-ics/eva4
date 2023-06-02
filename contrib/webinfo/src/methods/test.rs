use crate::client_info::ClientInfo;
use crate::http_errors::{internal_error, HttpError};
use crate::{RPC, TIMEOUT};
use axum::extract::Json;
use eva_common::acl::Acl;
use eva_sdk::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct Info {
    build: u64,
    product_code: String,
    product_name: String,
    system_name: String,
    time: f64,
    uptime: f64,
    version: String,
    #[serde(skip_deserializing)]
    acl: Option<Acl>,
}

pub async fn handle(ci: ClientInfo) -> Result<Json<Info>, HttpError> {
    let mut info: Info = unpack(
        safe_rpc_call(
            RPC.get().unwrap(),
            "eva.core",
            "test",
            busrt::empty_payload!(),
            QoS::Processed,
            *TIMEOUT.get().unwrap(),
        )
        .await
        .map_err(internal_error)?
        .payload(),
    )
    .map_err(internal_error)?;
    info.acl.replace(ci.acl);
    Ok(Json(info))
}
