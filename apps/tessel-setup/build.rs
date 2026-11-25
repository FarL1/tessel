fn main() {
    println!("cargo:rerun-if-changed=tessel-setup.rc");
    println!("cargo:rerun-if-changed=../../assets/brand/tessel.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile("tessel-setup.rc", embed_resource::NONE)
            .manifest_optional()
            .expect("compile tessel-setup.rc");
    }
}
