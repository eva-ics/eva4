use eva_common::events::{RawStateEventOwned, RAW_STATE_TOPIC};
use eva_common::prelude::*;
use eva_sdk::prelude::*;
use once_cell::sync::{Lazy, OnceCell};
use parking_lot::Mutex;
use serde::Serialize;
use std::collections::HashSet;

err_logger!();

static OID_PREFIX: OnceCell<OID> = OnceCell::new();
static OIDS_CREATED: Lazy<Mutex<HashSet<OID>>> = Lazy::new(<_>::default);

pub fn set_oid_prefix(prefix: OID) -> EResult<()> {
    OID_PREFIX
        .set(prefix)
        .map_err(|_| Error::core("Unable to set OID_PREFIX"))
}

pub struct Metric<'a> {
    group: &'a str,
    subgroup: Option<&'a str>,
    resource: &'a str,
}

impl<'a> Metric<'a> {
    pub fn new0(group: &'a str, resource: &'a str) -> Self {
        Self {
            group,
            subgroup: None,
            resource,
        }
    }
    pub fn new(group: &'a str, subgroup: &'a str, resource: &'a str) -> Self {
        Self {
            group,
            subgroup: Some(subgroup),
            resource,
        }
    }
    pub async fn report<S: Serialize>(&self, value: S) {
        self.send_report(value).await.log_ef();
    }
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
        eapi_bus::publish(
            &format!("{}{}", RAW_STATE_TOPIC, oid.as_path()),
            pack(&RawStateEventOwned::new(1, to_value(value)?))?.into(),
        )
        .await?;
        Ok(())
    }
}
