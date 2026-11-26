fn main() {
    println!("cargo:rerun-if-changed=assets/receiver.rc");
    println!("cargo:rerun-if-changed=assets/icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile("assets/receiver.rc", embed_resource::NONE);
    }
}
