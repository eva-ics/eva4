use eva_common::EResult;
use eva_common::dobj::DataObject;
use eva_common::dobj::Kind;

pub mod c;
pub mod rust;

pub use c::C;
pub use rust::Rust;

pub trait CodeGen: Send + Sync {
    fn ident(&self) -> &'static str {
        "    "
    }
    fn try_generate_struct(&self, dobj: &DataObject) -> EResult<String>;
    fn kind_to_string(&self, kind: &Kind) -> String;
}
