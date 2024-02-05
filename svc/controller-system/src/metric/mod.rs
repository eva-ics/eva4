use eva_common::{err_logger, ItemStatus, ITEM_STATUS_ERROR};
use serde::Serialize;

err_logger!();

#[cfg(feature = "service")]
pub mod svc;

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
}
