use axum::http::StatusCode;
use eva_common::prelude::*;
use eva_sdk::prelude::*;
use std::fmt::Write as _;
use std::net::IpAddr;

pub type HttpError = (StatusCode, String);

pub fn internal_error<E>(e: E) -> HttpError
where
    E: std::fmt::Display,
{
    let err = e.to_string();
    error!("{}", err);
    (StatusCode::INTERNAL_SERVER_ERROR, err)
}

pub fn convert_error(e: Error, ip: Option<&IpAddr>, auth: bool) -> HttpError {
    let status_code = match e.kind() {
        ErrorKind::AccessDenied => {
            warn!(
                "access error from {}: {}",
                ip.map_or_else(|| "unknown".to_string(), ToString::to_string),
                e
            );
            StatusCode::FORBIDDEN
        }
        ErrorKind::InvalidParameter => StatusCode::BAD_REQUEST,
        _ => {
            error!("{}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        }
    };
    let mut err = (status_code, e.to_string());
    if auth {
        write!(err.1, " (AUTH)").unwrap();
    }
    err
}
