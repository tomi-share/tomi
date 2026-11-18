//! Exit codes of the REAL `tomi-relay` binary (`api-v1.md` §10.1, A195). `infra/update.sh` and
//! `infra/forge/deploy.sh` read them: `78` = refused because of the data (never go back to the
//! previous binary), `1` = any other failure, `0` = fine.

use std::path::Path;
use std::process::{Command, Output};

fn relay(data_dir: &Path, args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_tomi-relay"));
    cmd.args(args)
        .env("TOMI_RELAY_DATA_DIR", data_dir)
        // Port 0 and loopback: nothing of the host is touched, and no case here reaches the bind.
        .env("TOMI_RELAY_LISTEN", "127.0.0.1:0")
        .env_remove("TOMI_RELAY_REQUIRE_EXISTING_DATA_DIR")
        .env_remove("TOMI_RELAY_ADMIN_WEB");
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.output().expect("cannot run tomi-relay")
}

fn code(out: &Output) -> i32 {
    out.status
        .code()
        .expect("tomi-relay was killed by a signal")
}

fn text(out: &Output) -> String {
    format!(
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

const GUARD: (&str, &str) = ("TOMI_RELAY_REQUIRE_EXISTING_DATA_DIR", "true");

#[test]
fn an_empty_data_dir_with_the_guard_exits_78_for_serve_and_for_admin_status() {
    let dir = tempfile::tempdir().unwrap();
    for args in [&["serve"][..], &["admin", "status"][..]] {
        let out = relay(dir.path(), args, &[GUARD]);
        assert_eq!(code(&out), 78, "{args:?}: {}", text(&out));
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("data_dir looks empty"),
            "{}",
            text(&out)
        );
    }
    assert_eq!(
        std::fs::read_dir(dir.path()).unwrap().count(),
        0,
        "nothing is created in a directory that looks empty"
    );
}

#[test]
fn a_spaces_entry_that_is_a_file_exits_78() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("spaces"), b"not a folder").unwrap();
    for args in [&["serve"][..], &["admin", "status"][..]] {
        let out = relay(dir.path(), args, &[]);
        assert_eq!(code(&out), 78, "{args:?}: {}", text(&out));
    }
}

#[test]
fn a_relay_lock_that_is_a_folder_exits_78_for_serve() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("relay.lock")).unwrap();
    let out = relay(dir.path(), &["serve"], &[]);
    assert_eq!(code(&out), 78, "{}", text(&out));
}

#[test]
fn an_invalid_configuration_exits_1() {
    let dir = tempfile::tempdir().unwrap();
    for args in [&["serve"][..], &["admin", "status"][..]] {
        let out = relay(dir.path(), args, &[("TOMI_RELAY_LISTEN", "not-an-address")]);
        assert_eq!(code(&out), 1, "{args:?}: {}", text(&out));
    }
    // A panel without an https public_url is a configuration error too, never `78`.
    let out = relay(
        dir.path(),
        &["serve"],
        &[
            ("TOMI_RELAY_ADMIN_WEB", "true"),
            ("TOMI_RELAY_PUBLIC_URL", "http://localhost"),
        ],
    );
    assert_eq!(code(&out), 1, "{}", text(&out));
}

#[test]
fn admin_status_on_an_empty_data_dir_without_the_guard_exits_0_and_creates_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let out = relay(dir.path(), &["admin", "status"], &[]);
    assert_eq!(code(&out), 0, "{}", text(&out));
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn admin_status_on_the_intact_old_layout_is_not_78() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("tomi.db"), b"").unwrap();
    let out = relay(dir.path(), &["admin", "status"], &[]);
    assert_eq!(code(&out), 1, "{}", text(&out));
    let err = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(err.contains("not migrated yet"), "{err}");
    assert!(!dir.path().join("spaces").exists());

    // The same directory with a `spaces/` next to `tomi.db`: both layouts, a data refusal.
    std::fs::create_dir(dir.path().join("spaces")).unwrap();
    let out = relay(dir.path(), &["admin", "status"], &[]);
    assert_eq!(code(&out), 78, "{}", text(&out));
}
