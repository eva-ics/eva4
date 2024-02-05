use eva_common::events::{RawStateEventOwned, RAW_STATE_TOPIC};
use eva_common::prelude::*;
use eva_common::ITEM_STATUS_ERROR;
#[cfg(feature = "service")]
use eva_sdk::prelude::*;
use once_cell::sync::{Lazy, OnceCell};
use parking_lot::Mutex;
use serde::Serialize;
use std::collections::HashSet;

#[cfg(feature = "service")]
err_logger!();

#[cfg(feature = "service")]
static OID_PREFIX: OnceCell<OID> = OnceCell::new();
#[cfg(feature = "service")]
static OIDS_CREATED: Lazy<Mutex<HashSet<OID>>> = Lazy::new(<_>::default);

#[cfg(feature = "service")]
pub fn set_oid_prefix(prefix: OID) -> EResult<()> {
    OID_PREFIX
        .set(prefix)
        .map_err(|_| Error::core("Unable to set OID_PREFIX"))
}

pub struct Metric<'a> {
    group: &'a str,
    subgroup: Option<&'a str>,
    resource: &'a str,
    status: ItemStatus,
}

impl<'a> Metric<'a> {
    #[inline]
    pub fn new0(group: &'a str, resource: &'a str) -> Self {
        Self {
            group,
            subgroup: None,
            resource,
            status: 1,
        }
    }
    #[inline]
    pub fn new(group: &'a str, subgroup: &'a str, resource: &'a str) -> Self {
        Self {
            group,
            subgroup: Some(subgroup),
            resource,
            status: 1,
        }
    }
    #[inline]
    pub fn failed(mut self) -> Self {
        self.status = ITEM_STATUS_ERROR;
        self
    }
    #[inline]
    pub async fn report<S: Serialize>(&self, value: S) {
        self.send_report(value).await.log_ef();
    }
    #[cfg(feature = "service")]
    async fn send_report<S: Serialize>(&self, value: S) -> EResult<()> {
        let oid = if let Some(n) = &self.subgroup {
            format!(
                "{}/{}/{}/{}",
                OID_PREFIX.get().unwrap(),
                self.group,
                n,
                self.resource
            )
        } else {
            format!(
                "{}/{}/{}",
                OID_PREFIX.get().unwrap(),
                self.group,
                self.resource
            )
        }
        .parse::<OID>()?;
        if !OIDS_CREATED.lock().contains(&oid) {
            eapi_bus::create_items(&[&oid]).await?;
            OIDS_CREATED.lock().insert(oid.clone());
        }
        let ev = if self.status == ITEM_STATUS_ERROR {
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
    }
}
