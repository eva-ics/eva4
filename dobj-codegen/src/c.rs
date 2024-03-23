use eva_common::dobj::{DataObject, Kind};

use crate::CodeGen;

#[derive(Default)]
pub struct C;

impl C {
    pub fn new() -> Self {
        Self
    }
}

impl CodeGen for C {
    fn ident(&self) -> &'static str {
        "  "
    }
    fn generate_struct(&self, dobj: &DataObject) -> String {
        let mut result = format!("typedef struct {} {{\n", dobj.name);
        for field in &dobj.fields {
            match &field.kind {
                Kind::Array(size, kind) => {
                    result.push_str(&format!(
                        "{}{} {}[{}];\n",
                        self.ident(),
                        self.kind_to_string(kind),
                        field.name,
                        size
                    ));
                }
                kind => {
                    result.push_str(&format!(
                        "{}{} {};\n",
                        self.ident(),
                        self.kind_to_string(kind),
                        field.name
                    ));
                }
            }
        }
        result.push_str(&format!("}} {};\n", dobj.name));
        result
    }

    fn kind_to_string(&self, kind: &Kind) -> String {
        match kind {
            Kind::Bool => "bool".to_owned(),
            Kind::I8 => "int8_t".to_owned(),
            Kind::I16 => "int16_t".to_owned(),
            Kind::I32 => "int32_t".to_owned(),
            Kind::I64 => "int64_t".to_owned(),
            Kind::U8 => "uint8_t".to_owned(),
            Kind::U16 => "uint16_t".to_owned(),
            Kind::U32 => "uint32_t".to_owned(),
            Kind::U64 => "uint64_t".to_owned(),
            Kind::F32 => "float".to_owned(),
            Kind::F64 => "double".to_owned(),
            Kind::Array(..) => String::new(),
            Kind::DataObject(s) => s.to_string(),
        }
    }
}
