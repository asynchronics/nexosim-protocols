fn main() -> Result<(), std::io::Error> {
    #[cfg(yamcs_bridge_codegen)]
    prost_build::Config::new()
        .out_dir("src/codegen/")
        .type_attribute(".", "#[derive(serde::Serialize, serde::Deserialize)]")
        .compile_protos(&["ygw.proto"], &["src/proto/"])?;

    Ok(())
}
