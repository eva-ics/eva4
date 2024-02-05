use eva_common::prelude::*;
use eva_common::{err_logger, ITEM_STATUS_ERROR};
use serde::Serialize;

err_logger!();

#[cfg(feature = "service")]
pub mod svc;

pub struct Metric {
    #[cfg(feature = "service")]
    oid: Option<OID>,
    #[cfg(not(feature = "service"))]
    i: String,
    status: ItemStatus,
}

impl Metric {
    #[inline]
    pub fn failed(mut self) -> Self {
        self.status = ITEM_STATUS_ERROR;
        self
    }
    #[inline]
    pub async fn report<S: Serialize>(&self, value: S) {
        self.send_report(value)
            .await
            .log_ef_with("unable to send metric event");
    }
}

#[cfg(not(feature = "service"))]
impl Metric {
    #[inline]
    pub fn new0(group: &str, resource: &str) -> EResult<Self> {
        let i = format!("{}/{}", group, resource);
        Ok(Self { i, status: 1 })
    }
    #[inline]
    pub fn new(group: &str, subgroup: &str, resource: &str) -> EResult<Self> {
        let i = format!("{}/{}/{}", group, subgroup, resource);
        Ok(Self { i, status: 1 })
    }
}