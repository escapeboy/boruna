//! `--version` must work on every shipped binary, so users and package managers can tell
//! which build they have.

use std::process::Command;

#[test]
fn version_flag_prints_name_and_crate_version() {
    for flag in ["--version", "-V"] {
        let out = Command::new(env!("CARGO_BIN_EXE_boruna-orch"))
            .arg(flag)
            .output()
            .expect("run binary");
        assert!(out.status.success(), "{flag}: {:?}", out);
        let text = String::from_utf8(out.stdout).unwrap();
        assert_eq!(
            text.trim(),
            concat!("boruna-orch ", env!("CARGO_PKG_VERSION")),
            "{flag}"
        );
    }
}
