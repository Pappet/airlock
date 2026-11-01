//! Stamps the build identity (`<version>+<unix seconds>`) into `AIRLOCK_BUILD`.

use std::time::{SystemTime, UNIX_EPOCH};

fn main() {
    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_default();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    println!("cargo:rustc-env=AIRLOCK_BUILD={version}+{now}");
    println!("cargo:rerun-if-changed=src");
}
