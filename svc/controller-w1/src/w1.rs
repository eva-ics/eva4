use eva_common::op::Op;
use eva_common::prelude::*;
use std::sync::Arc;
use std::time::Duration;

pub async fn get(path: Arc<String>, timeout: Duration, retries: u8) -> EResult<String> {
    let mut result = None;
    let op = Op::new(timeout);
    for _ in 0..=retries {
        let res = get_attr(path.clone(), op.timeout()?).await;
        if res.is_ok() {
            result = Some(res);
            break;
        }
        result = Some(res);
    }
    result.unwrap_or_else(|| Err(Error::timeout()))
}

pub async fn set(
    path: Arc<String>,
    value: Arc<String>,
    timeout: Duration,
    verify: bool,
    retries: u8,
) -> EResult<()> {
    let mut result = None;
    let op = Op::new(timeout);
    for _ in 0..=retries {
        let res = set_attr(path.clone(), value.clone(), op.timeout()?).await;
        if res.is_ok() {
            if verify {
                if let Some(verify_delay) = crate::VERIFY_DELAY.get().unwrap() {
                    let delay = *verify_delay;
                    op.enough(delay)?;
                    tokio::time::sleep(delay).await;
                }
                let value_str_set = get(path.clone(), op.timeout()?, retries).await?;
                if value.as_str() != value_str_set {
                    result = Some(Err(Error::failed("value set verification error")));
                    continue;
                }
            }
            result = Some(res);
            break;
        }
        result = Some(res);
    }
    result.unwrap_or_else(|| Err(Error::timeout()))
}

pub async fn scan(
    types: Option<Vec<String>>,
    attrs_any: Option<Vec<String>>,
    attrs_all: Option<Vec<String>>,
    timeout: Duration,
) -> EResult<Vec<owfs::Device>> {
    tokio::time::timeout(
        timeout,
        tokio::task::spawn_blocking(move || {
            let mut options = owfs::ScanOptions::new();
            let types_s = types
                .as_ref()
                .map(|v| v.iter().map(String::as_str).collect::<Vec<&str>>());
            if let Some(ref v) = types_s {
                options = options.types(v);
            }
            let attrs_any_s = attrs_any
                .as_ref()
                .map(|v| v.iter().map(String::as_str).collect::<Vec<&str>>());
            if let Some(ref v) = attrs_any_s {
                options = options.attrs_any(v);
            }
            let attrs_all_s = attrs_all
                .as_ref()
                .map(|v| v.iter().map(String::as_str).collect::<Vec<&str>>());
            if let Some(ref v) = attrs_all_s {
                options = options.attrs_all(v);
            }
            scan_sync(options)
        }),
    )
    .await
    .map_err(Error::failed)??
}

#[inline]
fn scan_sync(options: owfs::ScanOptions) -> EResult<Vec<owfs::Device>> {
    unsafe { owfs::scan(options).map_err(Error::failed) }
}

async fn get_attr(path: Arc<String>, timeout: Duration) -> EResult<String> {
    tokio::time::timeout(
        timeout,
        tokio::task::spawn_blocking(move || get_attr_sync(path)),
    )
    .await
    .map_err(Error::failed)??
}

#[inline]
fn get_attr_sync(path: Arc<String>) -> EResult<String> {
    unsafe { owfs::get(&path).map_err(Error::failed) }
}

async fn set_attr(path: Arc<String>, value: Arc<String>, timeout: Duration) -> EResult<()> {
    tokio::time::timeout(
        timeout,
        tokio::task::spawn_blocking(move || set_attr_sync(path, value)),
    )
    .await
    .map_err(Error::failed)??
}

#[inline]
fn set_attr_sync(path: Arc<String>, value: Arc<String>) -> EResult<()> {
    unsafe { owfs::set(&path, &value).map_err(Error::failed) }
}

pub async fn load_device(path: Arc<String>, timeout: Duration) -> EResult<owfs::Device> {
    tokio::time::timeout(
        timeout,
        tokio::task::spawn_blocking(move || load_device_sync(path)),
    )
    .await
    .map_err(Error::failed)??
}

pub async fn device_info(
    device: Arc<owfs::Device>,
    timeout: Duration,
) -> EResult<owfs::DeviceInfo> {
    tokio::time::timeout(
        timeout,
        tokio::task::spawn_blocking(move || device_info_sync(device)),
    )
    .await
    .map_err(Error::failed)??
}

#[inline]
fn load_device_sync(path: Arc<String>) -> EResult<owfs::Device> {
    let mut dev = owfs::Device::new(&path);
    unsafe { dev.load().map_err(Error::failed)? };
    Ok(dev)
}

#[inline]
fn device_info_sync(device: Arc<owfs::Device>) -> EResult<owfs::DeviceInfo> {
    unsafe { device.info().map_err(Error::failed) }
}
