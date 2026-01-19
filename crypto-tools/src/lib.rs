use std::path::Path;

use eva_common::{EResult, Error};
use tokio_native_tls::native_tls;

pub async fn read_tls_certificate(ca: &str, eva_dir: &str) -> EResult<native_tls::Certificate> {
    let path = eva_common::tools::format_path(eva_dir, Some(ca), None);
    let data = tokio::fs::read(&path).await?;
    let cert = if Path::new(ca)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("der"))
    {
        native_tls::Certificate::from_der(&data)
            .map_err(|e| Error::invalid_params(format!("{}: {}", path, e)))?
    } else {
        native_tls::Certificate::from_pem(&data)
            .map_err(|e| Error::invalid_params(format!("{}: {}", path, e)))?
    };
    Ok(cert)
}
