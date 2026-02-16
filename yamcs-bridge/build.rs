fn main() -> Result<(), std::io::Error> {
    #[cfg(yamcs_bridge_codegen)]
    prost_build::Config::new()
        .out_dir("src/codegen/")
        .compile_protos(&["ygw.proto"], &["src/proto/"])?;

    Ok(())
}
