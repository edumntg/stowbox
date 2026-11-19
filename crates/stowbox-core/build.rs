fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=proto/boxd/api.proto");
    // protox compiles the schema in pure Rust, so building needs no protoc install.
    let fds = protox::compile(["proto/boxd/api.proto"], ["proto"])?;
    tonic_prost_build::configure().build_server(false).compile_fds(fds)?;
    Ok(())
}
