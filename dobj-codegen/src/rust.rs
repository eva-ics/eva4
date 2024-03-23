use eva_common::dobj::{DataObject, Kind};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Endianess {
    Big,
    Little,
    Native,
}

use crate::CodeGen;

pub struct Rust {
    config: Config,
}

impl CodeGen for Rust {
    fn generate_struct(&self, dobj: &DataObject) -> String {
        let mut scope = codegen::Scope::new();
        let mut s = codegen::Struct::new(&dobj.name);
        if self.config.derive_clone {
            s.derive("Clone");
        }
        if self.config.derive_copy {
            s.derive("Copy");
        }
        if self.config.derive_debug {
            s.derive("Debug");
        }
        if self.config.derive_default {
            s.derive("Default");
        }
        if self.config.derive_eq {
            s.derive("Eq").derive("PartialEq");
        }
        if let Some(endianess) = self.config.binrw {
            s.attr("binrw");
            match endianess {
                Endianess::Big => {
                    s.attr("brw(big)");
                }
                Endianess::Little => {
                    s.attr("brw(little)");
                }
                Endianess::Native => {}
            }
        }
        for field in &dobj.fields {
            let ks = match &field.kind {
                Kind::Array(size, kind) => {
                    if *size < self.config.box_arrays {
                        format!("[{}; {}]", kind, size)
                    } else {
                        format!("Box<[{}; {}]>", kind, size)
                    }
                }
                k => k.to_string(),
            };
            let sf = codegen::Field::new(&field.name, ks);
            s.push_field(sf);
        }
        scope.push_struct(s);
        scope.to_string()
    }

    fn kind_to_string(&self, kind: &Kind) -> String {
        kind.to_string()
    }
}

impl Rust {
    pub fn new(config: Config) -> Self {
        Self { config }
    }
}

fn default_box_arrays() -> usize {
    100
}

#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default = "default_box_arrays")]
    box_arrays: usize,
    #[serde(default)]
    derive_debug: bool,
    #[serde(default)]
    derive_default: bool,
    #[serde(default)]
    derive_clone: bool,
    #[serde(default)]
    derive_copy: bool,
    #[serde(default)]
    derive_eq: bool,
    binrw: Option<Endianess>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            box_arrays: default_box_arrays(),
            derive_debug: false,
            derive_default: false,
            derive_clone: false,
            derive_copy: false,
            derive_eq: false,
            binrw: None,
        }
    }
}

impl Config {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn box_arrays(mut self, box_arrays: usize) -> Self {
        self.box_arrays = box_arrays;
        self
    }
    pub fn derive_debug(mut self, derive_debug: bool) -> Self {
        self.derive_debug = derive_debug;
        self
    }
    pub fn derive_default(mut self, derive_default: bool) -> Self {
        self.derive_default = derive_default;
        self
    }
    pub fn derive_clone(mut self, derive_clone: bool) -> Self {
        self.derive_clone = derive_clone;
        self
    }
    pub fn derive_copy(mut self, derive_copy: bool) -> Self {
        self.derive_copy = derive_copy;
        self
    }
    pub fn derive_eq(mut self, derive_eq: bool) -> Self {
        self.derive_eq = derive_eq;
        self
    }
    pub fn binrw(mut self, binrw: Option<Endianess>) -> Self {
        self.binrw = binrw;
        self
    }
}
