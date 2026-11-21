fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=proto/boxd/api.proto");
    println!("cargo:rerun-if-changed=proto/e2b/process.proto");
    // protox compiles the schema in pure Rust, so building needs no protoc install.
    let fds = protox::compile(["proto/boxd/api.proto"], ["proto"])?;
    tonic_prost_build::configure().build_server(false).compile_fds(fds)?;
    // envd speaks Connect, not gRPC: only the messages are generated, and
    // src/connectrpc.rs carries them.
    let envd = protox::compile(["proto/e2b/process.proto"], ["proto"])?;
    tonic_prost_build::configure().build_server(false).build_client(false).compile_fds(envd)?;
    println!("cargo:rerun-if-changed=proto/modal/api.proto");
    println!("cargo:rerun-if-changed=proto/modal/task_command_router.proto");
    let modal = protox::compile(["proto/modal/api.proto", "proto/modal/task_command_router.proto"], ["proto"])?;
    tonic_prost_build::configure().build_server(false).compile_fds(modal)?;
    Ok(())
}
