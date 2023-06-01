use cargo_metadata::{CargoOpt, MetadataCommand};
use std::path::Path;

fn main() {
    let mut manifest = Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap()).to_owned();
    manifest.push("Cargo.toml");
    let metadata = MetadataCommand::new()
        .manifest_path(manifest)
        .features(CargoOpt::AllFeatures)
        .exec()
        .unwrap();
    if let Some(resolve) = metadata.resolve {
        resolve
            .nodes
            .iter()
            .filter(|node| {
                node.id.repr.starts_with("serde_json ")
                    && node.features.iter().any(|v| v == "arbitrary_precision")
            })
            .for_each(|node| {
                let deps = resolve
                    .nodes
                    .iter()
                    .filter(|d| d.dependencies.contains(&node.id))
                    .map(|d| d.id.repr.as_str())
                    .collect::<Vec<&str>>();
                panic!(
                    "serde_json arbitrary_precision MUST be off. Dependents: \n\n{}\n\n",
                    deps.join("\n")
                );
            });
    }
}
