extern crate protoc_rust;

fn main() {
    protoc_rust::Codegen::new()
        .out_dir("src/protos")
        .inputs(&["proto/profile.proto"])
        .include("proto")
        .run()
        .expect("protoc");
}
