//! Version records: the release pipeline signs, titles, and verifies one
//! version, so the two files that repeat it by hand must agree with the
//! package version. This catches a bump that misses the exe resource or the
//! installer before a tag is pushed.

use std::fs;
use std::path::PathBuf;

fn read(relative: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(relative);
    fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()))
}

fn has_line(text: &str, expected: &str) -> bool {
    text.lines().any(|line| line.trim() == expected)
}

#[test]
fn the_exe_resource_matches_the_package_version() {
    let version = env!("CARGO_PKG_VERSION");
    let resource = read("assets/icon.rc");

    let numeric = format!("{},0", version.replace('.', ","));
    for key in ["FILEVERSION", "PRODUCTVERSION"] {
        let expected = format!("{key} {numeric}");
        assert!(
            has_line(&resource, &expected),
            "assets/icon.rc must declare `{expected}` for package version {version}"
        );
    }

    for key in ["FileVersion", "ProductVersion"] {
        let expected = format!("VALUE \"{key}\", \"{version}\\0\"");
        assert!(
            resource.contains(&expected),
            "assets/icon.rc must declare `{expected}` for package version {version}"
        );
    }
}

#[test]
fn the_installer_matches_the_package_version() {
    let version = env!("CARGO_PKG_VERSION");
    let script = read("installer/nsis/screencapture.nsi");

    let expected = [
        format!("!define VERSION \"{version}\""),
        format!("!define VERSIONNUM \"{version}.0\""),
    ];
    for line in &expected {
        assert!(
            has_line(&script, line),
            "installer/nsis/screencapture.nsi must declare `{line}` for package version {version}"
        );
    }
}
