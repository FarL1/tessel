fn main() {
    println!("cargo:rerun-if-changed=tessel.rc");
    println!("cargo:rerun-if-changed=../../assets/brand/tessel.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile("tessel.rc", embed_resource::NONE)
            .manifest_optional()
            .expect("compile tessel.rc");
    }
}
