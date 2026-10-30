fn main() {
    // `microsoft.rs` bakes the Azure client id in with `option_env!`, so a
    // changed value must trigger a rebuild.
    println!("cargo:rerun-if-env-changed=SHCCL_MICROSOFT_CLIENT_ID");
}
