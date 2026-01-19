use super::Metric;
use eva_common::err_logger;
use eva_common::events::{RAW_STATE_TOPIC, RawStateEvent, RawStateEventOwned};
use eva_common::prelude::*;
use eva_sdk::prelude::*;
use parking_lot::Mutex;
use serde::Serialize;
use std::collections::HashSet;
use std::sync::{LazyLock, OnceLock};

err_logger!();

static OID_PREFIX: OnceLock<String> = OnceLock::new();
static OIDS_CREATED: LazyLock<Mutex<HashSet<OID>>> = LazyLock::new(<_>::default);

pub fn set_oid_prefix(prefix: String) -> EResult<()> {
    format!("{}/id", prefix).parse::<OID>()?;
    OID_PREFIX
        .set(prefix)
        .map_err(|_| Error::core("Unable to set OID_PREFIX"))
}

impl Metric {
    /// # Panics
    ///
    /// will panic if oid prefix is not set
    #[inline]
    pub fn new0(group: &str, resource: &str) -> Self {
        let oid = format!("{}/{}/{}", OID_PREFIX.get().unwrap(), group, resource)
            .parse::<OID>()
            .log_err_with("OID mapping error")
            .ok();
        Self { oid, status: 1 }
    }
    /// # Panics
    ///
    /// will panic if oid prefix is not set
    #[inline]
    pub fn new(group: &str, subgroup: &str, resource: &str) -> Self {
        let oid = format!(
            "{}/{}/{}/{}",
            OID_PREFIX.get().unwrap(),
            group,
            subgroup,
            resource
        )
        .parse::<OID>()
        .log_err_with("OID mapping error")
        .ok();
        Self { oid, status: 1 }
    }
    #[inline]
    pub fn new_for_host(
        oid_prefix: &str,
        host: &str,
        full_id: &str,
        prefix_contains_host: bool,
    ) -> Self {
        let oid = match if prefix_contains_host {
            format!("{}/{}", oid_prefix.replace(crate::VAR_HOST, host), full_id)
        } else {
            format!("{}/{}/{}", oid_prefix, host, full_id)
        }
        .parse::<OID>()
        {
            Ok(v) => Some(v),
            Err(e) => {
                error!("host: {} i: {} OID mapping error: {}", host, full_id, e);
                None
            }
        };
        Self { oid, status: 1 }
    }
    #[inline]
    pub async fn report<S: Serialize>(&self, value: S) {
        self.send_report(value)
            .await
            .log_ef_with("unable to send metric event");
    }
    pub(super) async fn send_report<S: Serialize>(&self, value: S) -> EResult<()> {
        if let Some(ref oid) = self.oid {
            if !OIDS_CREATED.lock().contains(oid) {
                eapi_bus::create_items(&[oid]).await?;
                OIDS_CREATED.lock().insert(oid.clone());
            }
            let ev = if self.status < 0 {
                RawStateEventOwned::new0(self.status)
            } else {
                RawStateEventOwned::new(self.status, to_value(value)?)
            };
            eapi_bus::publish(
                &format!("{}{}", RAW_STATE_TOPIC, oid.as_path()),
                pack(&ev)?.into(),
            )
            .await?;
            Ok(())
        } else {
            Err(Error::failed("OID mapping error"))
        }
    }
    pub async fn send_bus_event(&self, ev: RawStateEvent<'_>) -> EResult<()> {
        if let Some(ref oid) = self.oid {
            if !OIDS_CREATED.lock().contains(oid) {
                eapi_bus::create_items(&[oid]).await?;
                OIDS_CREATED.lock().insert(oid.clone());
            }
            eapi_bus::publish(
                &format!("{}{}", RAW_STATE_TOPIC, oid.as_path()),
                pack(&ev)?.into(),
            )
            .await?;
            Ok(())
        } else {
            Err(Error::failed("OID mapping error"))
        }
    }
}
