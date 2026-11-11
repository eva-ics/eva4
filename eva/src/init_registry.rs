use colored::Colorize;
use eva::{BUILD, VERSION};
use eva_common::tools::get_eva_dir;
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use yedb::Database;

const PFX: &str = "eva";

#[derive(Serialize)]
struct Info<'a> {
    version: &'a str,
    build: u64,
}

macro_rules! imported {
    ($k: expr) => {
        println!("{}", format!(" + {}", $k).green());
    };
}

macro_rules! skipped {
    ($k: expr) => {
        println!("{}", format!(" [skipped] {}", $k).dimmed());
    };
}

fn main() {
    let dir_eva = get_eva_dir();
    let mut db = Database::new();
    let db_path = format!("{dir_eva}/runtime/registry");
    db.set_db_path(&db_path).unwrap();
    db.open().expect("Unable to init/open the registry");
    println!("Importing registry schema...");
    let schema: BTreeMap<String, Value> = serde_saphyr::from_str_with_options(
        &fs::read_to_string(format!("{dir_eva}/share/registry/schema.yml")).unwrap(),
        serde_saphyr::options! {
                    duplicate_keys: serde_saphyr::DuplicateKeyPolicy::PassThrough,
                    merge_keys: serde_saphyr::MergeKeyPolicy::AsOrdinary,
        strict_booleans: true,
        leading_zero_numbers_as_strings: true,
        underscored_numbers_as_strings: true,
        non_finite_float_policy: serde_saphyr::NonFiniteFloatPolicy::PassThrough,
                },
    )
    .unwrap();
    for (k, v) in schema {
        let key_name = format!(".schema/{PFX}/{k}");
        imported!(key_name);
        db.key_set(&key_name, v).unwrap();
    }
    println!("Importing registry defaults...");
    let dir_defaults = format!("{dir_eva}/share/registry/defaults");
    for entry in glob::glob(&format!("{dir_defaults}/**/*.yml"))
        .unwrap()
        .flatten()
    {
        let fname = &entry.as_os_str().to_string_lossy();
        let key_name = format!("{PFX}{}", &fname[dir_defaults.len()..fname.len() - 4]);
        let key_content: Value = serde_saphyr::from_str_with_options(
            &fs::read_to_string(entry).unwrap(),
            serde_saphyr::options! {
                            duplicate_keys: serde_saphyr::DuplicateKeyPolicy::PassThrough,
                            merge_keys: serde_saphyr::MergeKeyPolicy::AsOrdinary,
            strict_booleans: true,
            leading_zero_numbers_as_strings: true,
            underscored_numbers_as_strings: true,
            non_finite_float_policy: serde_saphyr::NonFiniteFloatPolicy::PassThrough,
                        },
        )
        .unwrap();
        if let Err(e) = db.key_get(&key_name) {
            if e.kind() == yedb::ErrorKind::KeyNotFound {
                db.key_set(&key_name, key_content).unwrap();
                imported!(key_name);
            } else {
                panic!("{}", e);
            }
        } else {
            skipped!(key_name);
        }
    }
    db.key_set(
        &format!("{PFX}/data/info"),
        serde_json::to_value(Info {
            version: VERSION,
            build: BUILD,
        })
        .unwrap(),
    )
    .unwrap();
}
