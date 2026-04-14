use std::fs;
use std::path::PathBuf;

fn main() {
    let schema = qualitool_protocol::schema::generate_schema();
    let json = serde_json::to_string_pretty(&schema).expect("schema serialization failed");

    let out_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("schema/qualitool.schema.json");
    fs::create_dir_all(out_path.parent().unwrap()).expect("failed to create schema directory");
    fs::write(&out_path, &json).expect("failed to write schema file");

    println!("Schema written to {}", out_path.display());
}
