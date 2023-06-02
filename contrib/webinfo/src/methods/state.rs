use crate::client_info::ClientInfo;
use crate::http_errors::{internal_error, HttpError};
use crate::{RPC, TIMEOUT};
use axum::extract::{Json, Path};
use axum::http::StatusCode;
use eva_common::acl::OIDMask;
use eva_common::prelude::*;
use eva_sdk::prelude::*;
use serde::Serialize;

pub async fn handle(ci: ClientInfo, Path(path): Path<String>) -> Result<Json<Value>, HttpError> {
    #[derive(Serialize)]
    struct StatePayload {
        i: Vec<String>,
        include: Vec<String>,
        exclude: Vec<String>,
        full: bool,
    }
    // firstly add to the query wildcard OID mask if the path is a group
    let mut oids = vec![OIDMask::from_path(&format!("{path}/#"))
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("invalid OID mask: {e}")))?
        .to_string()];
    // if the path is also a valid OID, add it to the query
    if let Ok(oid) = OID::from_path(&path) {
        oids.push(oid.to_string());
    }
    let (allow, deny) = ci.acl.get_items_allow_deny_reading();
    let payload = StatePayload {
        i: oids,
        include: allow,
        exclude: deny,
        full: false,
    };
    // query the node core for item states
    let result: Value = unpack(
        safe_rpc_call(
            RPC.get().unwrap(),
            "eva.core",
            "item.state",
            pack(&payload).map_err(internal_error)?.into(),
            QoS::Processed,
            *TIMEOUT.get().unwrap(),
        )
        .await
        .map_err(internal_error)?
        .payload(),
    )
    .map_err(internal_error)?;
    Ok(Json(result))
}
