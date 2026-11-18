//! `api-v1.md` §11.12 (distribution, health, administration) and §11.13 (rate limits),
//! plus start-up behaviour.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::result_large_err)]

mod common;

use std::net::SocketAddr;

use axum::body::Body;
use axum::http::Request;
use common::*;
use serde_json::{json, Value};
use tomi_relay::admin::{self, Cmd};
use tomi_relay::routes::{AuthClass, ENDPOINTS};

fn write(dir: &std::path::Path, rel: &str, content: &[u8]) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, content).unwrap();
}

// ===== 11.12 distribution =========================================================

#[tokio::test]
async fn a121_nothing_leaves_the_public_directory() {
    let env = Env::new().await;
    write(env.data_dir(), "config.toml", b"secret = 1");
    write(
        env.data_dir(),
        "public/updates/android/latest.json",
        br#"{"version":"0.2.0"}"#,
    );
    write(env.data_dir(), "public/updates/.hidden", b"hidden");
    write(
        env.data_dir(),
        "public/updates/android/tomi-0.2.0.apk",
        &[7u8; 5000],
    );
    for bad in [
        "/updates/../config.toml",
        "/updates/%2e%2e/config.toml",
        "/updates/%2E%2E/x",
        "/updates/android/../../config.toml",
        "/updates/android/",
        "/updates/android",
        "/updates/",
        "/updates/.hidden",
        "/updates/android/%2ehidden",
        "/updates/android%5Clatest.json",
        "/updates/android/latest.json%00",
        "/updates/nothing.json",
    ] {
        let r = env.send("GET", bad, &[], None).await;
        assert_eq!(r.status, 404, "{bad}: {}", String::from_utf8_lossy(&r.body));
        assert!(!String::from_utf8_lossy(&r.body).contains("secret"));
    }
    // What is public is served.
    let r = env
        .send("GET", "/updates/android/latest.json", &[], None)
        .await;
    assert_eq!(r.status, 200);
    assert_eq!(r.json()["version"], "0.2.0");
}

#[tokio::test]
async fn root_serves_a_static_landing_page() {
    let env = Env::new().await;
    let r = env.send("GET", "/", &[], None).await;
    assert_eq!(r.status, 200);
    let ct = r.header("content-type").unwrap_or_default();
    assert!(ct.starts_with("text/html"), "content-type: {ct}");
    // Script-free: a strict CSP and no inline script in the body.
    let csp = r.header("content-security-policy").unwrap_or_default();
    assert!(csp.contains("default-src 'none'"), "csp: {csp}");
    let body = String::from_utf8_lossy(&r.body);
    assert!(!body.contains("<script"), "the page must run no script");
    assert!(body.contains("Tomi"), "the page names the product");
    // A blind relay leaks nothing of a space or a token on its front page.
    assert!(!body.contains("space_id"));
    assert!(!body.contains("install"));
}

#[cfg(unix)]
#[tokio::test]
async fn a121b_a_symlink_leaving_public_is_not_followed() {
    let env = Env::new().await;
    write(env.data_dir(), "outside.txt", b"secret");
    std::fs::create_dir_all(env.data_dir().join("public/updates")).unwrap();
    std::os::unix::fs::symlink(
        env.data_dir().join("outside.txt"),
        env.data_dir().join("public/updates/link.json"),
    )
    .unwrap();
    env.send("GET", "/updates/link.json", &[], None)
        .await
        .err(404, "not_found");
}

#[tokio::test]
async fn a121c_updates_are_served_as_the_spec_says() {
    let env = Env::new().await;
    write(
        env.data_dir(),
        "public/updates/android/latest.json",
        br#"{"version":"0.2.0"}"#,
    );
    write(
        env.data_dir(),
        "public/updates/android/tomi-0.2.0.apk",
        &(0..5000).map(|i| (i % 251) as u8).collect::<Vec<_>>(),
    );
    write(env.data_dir(), "public/updates/windows/setup.sig", b"sig");
    write(env.data_dir(), "public/updates/windows/setup.exe", b"exe");

    let manifest = env
        .send("GET", "/updates/android/latest.json", &[], None)
        .await;
    assert_eq!(manifest.status, 200);
    assert_eq!(
        manifest.header("content-type").as_deref(),
        Some("application/json")
    );
    assert_eq!(
        manifest.header("cache-control").as_deref(),
        Some("no-cache")
    );
    let etag = manifest.header("etag").unwrap();
    assert!(manifest.header("last-modified").unwrap().ends_with("GMT"));
    // Conditional request.
    let again = env
        .send(
            "GET",
            "/updates/android/latest.json",
            &[("if-none-match", etag.as_str())],
            None,
        )
        .await;
    assert_eq!((again.status, again.body.len()), (304, 0));
    // The APK: type, immutable cache, ranges.
    let apk = env
        .send("GET", "/updates/android/tomi-0.2.0.apk", &[], None)
        .await;
    assert_eq!(
        apk.header("content-type").as_deref(),
        Some("application/vnd.android.package-archive")
    );
    assert_eq!(
        apk.header("cache-control").as_deref(),
        Some("public, max-age=31536000, immutable")
    );
    assert_eq!(apk.body.len(), 5000);
    let part = env
        .send(
            "GET",
            "/updates/android/tomi-0.2.0.apk",
            &[("range", "bytes=10-19")],
            None,
        )
        .await;
    assert_eq!(part.status, 206);
    assert_eq!(
        part.header("content-range").as_deref(),
        Some("bytes 10-19/5000")
    );
    assert_eq!(part.body.as_ref(), &apk.body[10..20]);
    env.send(
        "GET",
        "/updates/android/tomi-0.2.0.apk",
        &[("range", "bytes=0-1,5-6")],
        None,
    )
    .await
    .err(416, "range_not_satisfiable");
    // HEAD.
    let head = env
        .send("HEAD", "/updates/android/tomi-0.2.0.apk", &[], None)
        .await;
    assert_eq!((head.status, head.body.len()), (200, 0));
    assert_eq!(head.header("content-length").as_deref(), Some("5000"));
    // Other types.
    let sig = env
        .send("GET", "/updates/windows/setup.sig", &[], None)
        .await;
    assert_eq!(sig.header("content-type").as_deref(), Some("text/plain"));
    let exe = env
        .send("GET", "/updates/windows/setup.exe", &[], None)
        .await;
    assert_eq!(
        exe.header("content-type").as_deref(),
        Some("application/octet-stream")
    );
    // Read only.
    env.send("POST", "/updates/android/latest.json", &[], None)
        .await
        .err(405, "method_not_allowed");
    env.send("PUT", "/updates/android/latest.json", &[], None)
        .await
        .err(405, "method_not_allowed");
    // Assetlinks.
    env.send("GET", "/.well-known/assetlinks.json", &[], None)
        .await
        .err(404, "not_found");
    write(env.data_dir(), "public/.well-known/assetlinks.json", b"[]");
    let links = env
        .send("GET", "/.well-known/assetlinks.json", &[], None)
        .await;
    assert_eq!(links.status, 200);
    assert_eq!(
        links.header("content-type").as_deref(),
        Some("application/json")
    );
    assert_eq!(links.header("cache-control").as_deref(), Some("no-cache"));
    env.send("GET", "/.well-known/other.json", &[], None)
        .await
        .err(404, "not_found");
}

#[tokio::test]
async fn a122_the_install_page_has_a_strict_policy() {
    let env = Env::new().await;
    env.send("GET", "/l/pair", &[], None)
        .await
        .err(404, "not_found");
    write(
        env.data_dir(),
        "public/l/index.html",
        b"<html><body>Installer Tomi</body></html>",
    );
    for path in ["/l/pair", "/l/", "/l", "/l/anything/at/all"] {
        let r = env.send("GET", path, &[], None).await;
        assert_eq!(r.status, 200, "{path}");
        assert_eq!(r.body.as_ref(), b"<html><body>Installer Tomi</body></html>");
        assert_eq!(
            r.header("content-type").as_deref(),
            Some("text/html; charset=utf-8")
        );
        assert_eq!(
            r.header("content-security-policy").as_deref(),
            Some("default-src 'none'; style-src 'unsafe-inline'; img-src 'self'")
        );
        assert_eq!(r.header("referrer-policy").as_deref(), Some("no-referrer"));
    }
}

#[tokio::test]
async fn a123_health() {
    let env = Env::new().await;
    let (devs, _) = env.space_of(1).await;
    let ok = env.send("GET", "/healthz", &[], None).await;
    assert_eq!(ok.status, 200);
    assert_eq!(ok.json(), json!({ "status": "ok" }));
    // A broken database of a space.
    env.db().inject_fault(true);
    let down = env.send("GET", "/healthz", &[], None).await;
    assert_eq!(down.status, 503);
    assert_eq!(down.json(), json!({ "status": "unavailable" }));
    // The API says the same thing with a problem document.
    env.get(&devs[0], "/v1/space").await.err(503, "unavailable");
    env.db().inject_fault(false);
    assert_eq!(env.send("GET", "/healthz", &[], None).await.status, 200);
    // So does a broken routing database.
    env.state.spaces.routing().inject_fault(true);
    assert_eq!(env.send("GET", "/healthz", &[], None).await.status, 503);
    env.state.spaces.routing().inject_fault(false);
    assert_eq!(env.send("GET", "/healthz", &[], None).await.status, 200);
}

// ===== 11.12 administration =======================================================

fn text(out: &admin::Output) -> &str {
    &out.stdout
}

#[tokio::test]
async fn a124_reset_wipes_the_last_space_and_prints_a_new_token() {
    let env = Env::new().await;
    let (devs, _) = env.space_of(2).await;
    let blob = env.upload_blob(&devs[0], &[1u8; 700], 700).await;
    env.send_item(&devs[0], &[&blob]).await.1.ok(201);
    let (addr, stop, _server) = serve(&env).await;
    let mut ws = Ws::open(addr, &devs[1]).await;
    let old_token = devs[1].tok().to_owned();
    let space = env.space();
    assert!(space.dir.is_dir());

    // Without the confirmation, or with a wrong one: refused, nothing happens.
    let refused = admin::run(
        &env.state,
        Cmd::Reset {
            confirm: None,
            space: None,
        },
    )
    .await;
    assert_eq!(refused.exit, 2);
    let wrong = admin::run(
        &env.state,
        Cmd::Reset {
            confirm: Some("deadbeef".into()),
            space: None,
        },
    )
    .await;
    assert_eq!(wrong.exit, 2);
    env.get(&devs[0], "/v1/space").await.ok(200);

    let out = admin::run(
        &env.state,
        Cmd::Reset {
            confirm: Some(env.space_id[..8].to_owned()),
            space: None,
        },
    )
    .await;
    assert_eq!(out.exit, 0, "{}", out.stderr);
    let new_token = text(&out).trim().to_owned();
    assert_eq!(ws.recv_close(1000).await, Some(4004));
    assert_eq!(
        env.send("GET", "/v1/hello", &[], None).await.ok(200)["space_initialized"],
        false
    );
    // The new token is a pending invitation (the relay keeps its hash only).
    assert!(tomi_relay::install::is_pending(env.state.spaces.routing(), &new_token).unwrap());
    assert_eq!(
        tomi_relay::install::list(env.state.spaces.routing())
            .unwrap()
            .len(),
        1,
        "the invitations before the reset are replaced"
    );
    // The directory of the space (database and blob files) is gone, and so is its routing.
    assert!(!space.dir.exists(), "the space directory is gone");
    assert!(env.state.spaces.is_empty());
    assert!(env.state.spaces.by_device(&devs[0].id).is_none());
    env.bearer("GET", "/v1/space", &old_token, None)
        .await
        .err(409, "space_not_initialized");
    env.challenge(&devs[0].id)
        .await
        .err(409, "space_not_initialized");

    // The new token creates a fresh space (a new space_id: old signatures are useless).
    let founder = Dev::new("PC");
    let space_id = new_uuid();
    let cert = founder
        .keys
        .sign_device_cert(&space_id, &founder.keys.public(), T0 as u64)
        .unwrap()
        .to_bytes()
        .unwrap();
    env.send(
        "POST",
        "/v1/space",
        &[],
        Some(json!({ "install_token": new_token, "space_id": space_id, "name": "PC", "platform": "windows", "certificate": b64(&cert) })),
    )
    .await
    .ok(201);
    let _ = stop.send(());
}

fn invite(rotate: bool, new: bool) -> Cmd {
    Cmd::Invite { rotate, new }
}

fn status(json: bool, space: Option<&str>) -> Cmd {
    Cmd::Status {
        json,
        space: space.map(str::to_owned),
    }
}

#[tokio::test]
async fn a125_invite_lists_creates_and_rotates_invitations() {
    let env = Env::new().await;
    let token = env.install_token.clone();
    // The listing shows that an invitation is pending, never its token: the relay keeps only
    // the hash of a token.
    let listed = admin::run(&env.state, invite(false, false)).await;
    assert_eq!(listed.exit, 0, "{}", listed.stderr);
    assert!(text(&listed).contains("invitation "), "{}", text(&listed));
    assert!(!text(&listed).contains(&token[..9]));
    assert_eq!(text(&listed).matches("invitation ").count(), 1);

    // `--new` adds an invitation and shows its token; the first one is still good.
    let added = admin::run(&env.state, invite(false, true)).await;
    assert_eq!(added.exit, 0);
    let second = text(&added).trim().to_owned();
    assert_ne!(second, token);
    assert_eq!(
        text(&admin::run(&env.state, invite(false, false)).await)
            .matches("invitation ")
            .count(),
        2
    );

    // `--rotate` replaces all of them by one.
    let rotated = admin::run(&env.state, invite(true, false)).await;
    assert_eq!(rotated.exit, 0);
    let new = text(&rotated).trim().to_owned();
    assert!(new != token && new != second);
    assert_eq!(
        tomi_relay::install::list(env.state.spaces.routing())
            .unwrap()
            .len(),
        1
    );
    // The old tokens no longer work, the new one does.
    let founder = Dev::new("PC");
    env.create_space_raw(&founder, &token)
        .await
        .err(403, "install_token_invalid");
    env.create_space_raw(&founder, &second)
        .await
        .err(403, "install_token_invalid");
    env.create_space_raw(&founder, &new).await.ok(201);

    // With a space the command still works: the next person gets an invitation too.
    let none = admin::run(&env.state, invite(false, false)).await;
    assert_eq!(none.exit, 0);
    assert!(text(&none).contains("no pending invitation"));
    let third = admin::run(&env.state, invite(false, true)).await;
    assert_eq!(third.exit, 0);
    assert!(
        tomi_relay::install::is_pending(env.state.spaces.routing(), text(&third).trim()).unwrap()
    );
}

#[tokio::test]
async fn a125b_status_devices_and_revoke_outputs() {
    let env = Env::new().await;
    let status_json = |space: Option<&str>| {
        let cmd = status(true, space);
        let st = env.state.clone();
        async move {
            let out = admin::run(&st, cmd).await;
            assert_eq!(out.exit, 0, "{}", out.stderr);
            serde_json::from_str::<Value>(&out.stdout).unwrap()
        }
    };
    let v = status_json(None).await;
    assert_eq!(v["spaces"], 0);
    assert_eq!(v["space_initialized"], false);
    assert_eq!(v["invitations_pending"], 1);
    assert_eq!(v["fcm"]["state"], "disabled");
    assert_eq!(v["quota"]["max_bytes"], 1_073_741_824u64);

    let (devs, _) = env.space_of(3).await;
    // Without --space: the relay as a whole.
    let v = status_json(None).await;
    assert_eq!(v["spaces"], 1);
    assert_eq!(v["space_initialized"], true);
    assert_eq!(v["invitations_pending"], 0);
    assert!(v.get("members").is_none(), "no detail of a space");
    assert!(admin::run(&env.state, status(false, None))
        .await
        .stdout
        .contains("spaces: 1"));
    // With --space: that space (an id or a prefix of 8 characters).
    let v = status_json(Some(&env.space_id[..8])).await;
    assert_eq!(v["space_id"], env.space_id);
    assert_eq!(v["members"], 3);
    assert_eq!(v["active"], 3);
    assert_eq!(v["key_epoch"], 1);
    assert_eq!(v["rotation_required"], false);
    assert!(v["quota"]["max_bytes"].as_u64().unwrap() > 0);
    assert!(v["quota"]["space_max_bytes"].is_null());
    assert!(admin::run(&env.state, status(false, Some(&env.space_id)))
        .await
        .stdout
        .contains("members: 3"));
    // An unknown space, a prefix that is too short.
    assert_eq!(
        admin::run(&env.state, status(true, Some("ffffffff")))
            .await
            .exit,
        1
    );
    assert_eq!(
        admin::run(&env.state, status(true, Some("6b51")))
            .await
            .exit,
        2
    );

    let list = admin::run(
        &env.state,
        Cmd::Devices {
            json: true,
            space: None,
        },
    )
    .await;
    let v: Value = serde_json::from_str(&list.stdout).unwrap();
    let rows = v["devices"].as_array().unwrap();
    assert_eq!(rows.len(), 3);
    let fingerprint = hex::encode(tomi_relay::wire::sha256(&devs[1].pk()))[..16].to_owned();
    assert!(rows
        .iter()
        .any(|r| r["device_id"] == devs[1].id && r["fingerprint"] == fingerprint.as_str()));
    assert!(rows
        .iter()
        .all(|r| r["format_max"] == 1 && r["last_seen_at"].is_string()));

    // Revoke: short prefix, unknown device, last member.
    assert_eq!(
        admin::run(
            &env.state,
            Cmd::Revoke {
                target: devs[1].id[..7].to_owned(),
                space: None
            }
        )
        .await
        .exit,
        2
    );
    assert_eq!(
        admin::run(
            &env.state,
            Cmd::Revoke {
                target: "ffffffff".into(),
                space: None
            }
        )
        .await
        .exit,
        1
    );
    let out = admin::run(
        &env.state,
        Cmd::Revoke {
            target: devs[1].id[..8].to_owned(),
            space: None,
        },
    )
    .await;
    assert_eq!(out.exit, 0, "{}", out.stderr);
    assert_eq!(
        env.get(&devs[0], "/v1/space").await.ok(200)["rotation_required"],
        true
    );
    assert_eq!(
        status_json(Some(&env.space_id[..8])).await["rotation_required"],
        true
    );
    admin::run(
        &env.state,
        Cmd::Revoke {
            target: devs[2].id.clone(),
            space: None,
        },
    )
    .await;
    // The last member cannot be revoked: reset is the way.
    let last = admin::run(
        &env.state,
        Cmd::Revoke {
            target: devs[0].id.clone(),
            space: None,
        },
    )
    .await;
    assert_eq!(last.exit, 2);
    assert!(last.stderr.contains("reset"));
}

#[tokio::test]
async fn a126_a_broken_fcm_account_is_reported_and_harmless() {
    let dir = tempfile::tempdir().unwrap();
    let account = dir.path().join("fcm.json");
    std::fs::write(&account, b"{ this is not a service account").unwrap();
    let path = account.clone();
    let env = Env::in_dir(dir, move |cfg, opts| {
        cfg.fcm_service_account_file = Some(path.clone());
        opts.push = PushChoice::FromConfig;
    })
    .await;
    let v: Value =
        serde_json::from_str(&admin::run(&env.state, status(true, None)).await.stdout).unwrap();
    assert_eq!(v["fcm"]["state"], "auth_error");
    assert!(v["fcm"]["last_error"]["class"].is_string());
    // The relay serves everything else normally.
    let (devs, _) = env.space_of(2).await;
    env.send_item(&devs[0], &[]).await.1.ok(201);
    env.get(&devs[1], "/v1/events?after=0").await.ok(200);
    env.send("GET", "/healthz", &[], None).await.ok(200);
    // The text form says it too.
    assert!(admin::run(&env.state, status(false, None))
        .await
        .stdout
        .contains("fcm: auth_error"));
}

#[tokio::test]
async fn a126b_a_valid_fcm_account_starts_unknown() {
    let dir = tempfile::tempdir().unwrap();
    let account = dir.path().join("fcm.json");
    std::fs::write(
        &account,
        rsa_key::service_account_json("https://oauth.test/token"),
    )
    .unwrap();
    let path = account.clone();
    let env = Env::in_dir(dir, move |cfg, opts| {
        cfg.fcm_service_account_file = Some(path.clone());
        opts.push = PushChoice::FromConfig;
    })
    .await;
    let v: Value =
        serde_json::from_str(&admin::run(&env.state, status(true, None)).await.stdout).unwrap();
    assert_eq!(v["fcm"]["state"], "unknown");
    assert_eq!(v["fcm"]["unregistered_tokens_7d"], 0);
}

#[tokio::test]
async fn a127_no_route_gives_access_to_the_administration() {
    assert_eq!(ENDPOINTS.len(), 43);
    for (i, e) in ENDPOINTS.iter().enumerate() {
        assert_eq!(usize::from(e.n), i + 1, "numbered like the table of §4.1");
        let p = e.path.to_ascii_lowercase();
        for forbidden in ["admin", "reset", "install", "token", "invite"] {
            assert!(!p.contains(forbidden), "{} {}", e.method, e.path);
        }
    }
    // Everything that revokes, rotates or deletes the space needs the session of a member.
    for e in ENDPOINTS.iter().filter(|e| {
        matches!(e.path, "/v1/me" | "/v1/rotations" | "/v1/devices")
            || (e.method, e.path) == ("DELETE", "/v1/space")
    }) {
        assert_eq!(e.auth, AuthClass::Bearer, "{} {}", e.method, e.path);
    }
    // The routes without any credential are the public ones, and only those.
    let public: Vec<_> = ENDPOINTS
        .iter()
        .filter(|e| e.auth == AuthClass::None)
        .map(|e| (e.method, e.path))
        .collect();
    assert_eq!(
        public,
        vec![
            ("GET", "/healthz"),
            ("GET", "/v1/hello"),
            ("POST", "/v1/auth/challenge"),
            ("POST", "/v1/auth/session"),
            ("POST", "/v1/pairing/channels/{channel_id}/claim"),
            ("GET", "/updates/{*path}"),
            ("GET", "/.well-known/assetlinks.json"),
            ("GET", "/l/{*path}"),
            ("POST", "/v1/space/receipt"),
        ]
    );

    // And the running router agrees: every endpoint of the table, called without credential.
    let env = Env::new().await;
    let (devs, _) = env.space_of(1).await;
    let fill = |path: &str| {
        path.replace("{device_id}", &devs[0].id)
            .replace("{channel_id}", "1")
            .replace("{item_id}", &new_uuid())
            .replace("{blob_id}", &new_uuid())
            .replace("{transfer_id}", &new_uuid())
            .replace("{*path}", "x")
    };
    for e in ENDPOINTS.iter() {
        let r = env.send(e.method, &fill(e.path), &[], None).await;
        match e.auth {
            AuthClass::Bearer | AuthClass::BearerOrPairing => {
                assert_eq!(r.status, 401, "{} {}", e.method, e.path)
            }
            AuthClass::None | AuthClass::Install | AuthClass::FirstMessage => {
                assert_ne!(r.status, 401, "{} {}", e.method, e.path);
            }
        }
        assert!(r.header("x-request-id").is_some());
    }
    // Plausible administration paths do not exist.
    for path in [
        "/admin",
        "/admin/reset",
        "/v1/admin",
        "/v1/admin/revoke",
        "/v1/reset",
        "/v1/revoke",
        "/v1/install-token",
        "/v1/space/install-token",
        "/install-token",
        "/metrics",
        "/debug",
    ] {
        for method in ["GET", "POST", "DELETE", "PUT"] {
            let r = env.send(method, path, &[], None).await;
            assert_eq!(r.status, 404, "{method} {path}");
        }
    }
    // `/v1/devices/revoke` is not a hidden admin route: it collides with the known pattern
    // `/v1/devices/{device_id}` (PATCH only). So the methods other than PATCH answer a
    // spec-correct 405, never a 2xx (§4.1). Documenting the collision rather than hiding it.
    for method in ["GET", "POST", "DELETE", "PUT"] {
        let r = env.send(method, "/v1/devices/revoke", &[], None).await;
        assert!(
            r.status == 404 || r.status == 405,
            "{method} /v1/devices/revoke gave {}, expected 404 or 405 (never a 2xx admin route)",
            r.status
        );
        assert!(
            !(200..300).contains(&r.status),
            "{method} /v1/devices/revoke must not succeed"
        );
    }
}

// ===== 11.13 rate limits ==========================================================

#[tokio::test]
async fn a130_the_eleventh_challenge_is_limited() {
    let env = Env::with(|cfg, _| real_limits(cfg)).await;
    let d = Dev::new("x");
    for i in 0..10 {
        let r = env.challenge(&d.id).await;
        assert_eq!(
            r.status, 409,
            "request {i}: the limit comes before the check of the space"
        );
    }
    let limited = env.challenge(&d.id).await;
    limited.err(429, "rate_limited");
    let retry: u64 = limited.header("retry-after").unwrap().parse().unwrap();
    assert!(retry >= 1);
    assert_eq!(limited.json()["retry_after"], retry);
    // It refills: 20 a minute is one every 3 seconds.
    env.clock.advance_secs(3);
    assert_eq!(env.challenge(&d.id).await.status, 409);
    env.challenge(&d.id).await.err(429, "rate_limited");
}

#[tokio::test]
async fn a131_the_forwarded_address_is_trusted_only_from_a_trusted_proxy() {
    let env = Env::with(|cfg, _| real_limits(cfg)).await;
    let d = Dev::new("x");
    let call = |peer: &str, forwarded: String| {
        let req = Request::builder()
            .method("POST")
            .uri("/v1/auth/challenge")
            .header("content-type", "application/json")
            .header("x-forwarded-for", forwarded)
            .body(Body::from(json!({ "device_id": d.id }).to_string()))
            .unwrap();
        let peer: SocketAddr = peer.parse().unwrap();
        env.call_from(peer, req)
    };
    // From an address that is not a trusted proxy, `X-Forwarded-For` is ignored: all the
    // requests count against the TCP address, whatever they claim.
    for i in 0..10 {
        assert_eq!(
            call("203.0.113.9:4000", format!("10.0.0.{i}")).await.status,
            409
        );
    }
    call("203.0.113.9:4000", "10.0.0.99".into())
        .await
        .err(429, "rate_limited");
    // A trusted proxy (127.0.0.1) speaks for the client: each client has its own bucket.
    for i in 0..30 {
        assert_eq!(
            call("127.0.0.1:5000", format!("198.51.100.{i}"))
                .await
                .status,
            409,
            "client {i}"
        );
    }
    // The same client behind the proxy is limited all the same.
    for _ in 0..10 {
        call("127.0.0.1:5000", "192.0.2.77".into()).await;
    }
    call("127.0.0.1:5000", "192.0.2.77".into())
        .await
        .err(429, "rate_limited");
    // The chain is read from the right: a client cannot choose its own bucket by prepending.
    call("127.0.0.1:5000", "1.2.3.4, 192.0.2.77".into())
        .await
        .err(429, "rate_limited");
}

/// R1: a `Pairing`-authenticated request carries no session, so the per-device limit never
/// applies to it. A token that is not valid for its channel (here nobody ever claimed a
/// channel, so no token is) must therefore stay bounded per IP (`rl_ip_public`): otherwise
/// anyone could flood the single SQLite writer with `Authorization: Pairing …` on the pairing
/// routes. A token that *is* valid for its channel is not counted there (§8.5): see A132 to
/// A136 in `tests/pairing.rs`, which also cover every kind of invalid token.
#[tokio::test]
async fn pairing_requests_are_rate_limited_per_ip() {
    let env = Env::with(|cfg, _| real_limits(cfg)).await;
    let mut founder = Dev::new("PC");
    env.create_space(&mut founder).await;
    // A bogus pairing token on a channel nobody opened: the route answers `not_found`, but the
    // request is counted against the address of the caller all the same.
    let call = || {
        let req = Request::builder()
            .method("POST")
            .uri("/v1/pairing/channels/1/messages")
            .header(
                "authorization",
                "Pairing tmp1.bogus-not-a-real-pairing-token",
            )
            .header("content-type", "application/json")
            .body(Body::from(json!({ "body": "AAAA" }).to_string()))
            .unwrap();
        env.call_from("203.0.113.50:7000".parse().unwrap(), req)
    };
    // rl_ip_public: 30/min, burst 10 (the founder above came from another address). The burst
    // is spent without a 429...
    for _ in 0..10 {
        call().await.err(404, "not_found");
    }
    // ...then the flood is stopped (R1 regression).
    call().await.err(429, "rate_limited");
    call().await.err(429, "rate_limited");
}

#[tokio::test]
async fn device_limits_apply_per_device() {
    let env = Env::with(|cfg, _| real_limits(cfg)).await;
    let (devs, _) = env.space_of(2).await;
    // rl_device_items: burst 20.
    let mut ok = 0;
    let mut limited = None;
    for _ in 0..30 {
        let (_, r) = env.send_item(&devs[0], &[]).await;
        if r.status == 429 {
            limited = Some(r);
            break;
        }
        assert_eq!(r.status, 201);
        ok += 1;
    }
    assert_eq!(ok, 20);
    limited.unwrap().err(429, "rate_limited");
    // The other device is not affected.
    env.send_item(&devs[1], &[]).await.1.ok(201);
}

// ===== start-up ===================================================================

#[tokio::test]
async fn a_second_relay_cannot_use_the_same_data_directory() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = Config::for_tests(dir.path());
    let opts = |lock| Options {
        hold_lock: lock,
        background: false,
        ..Options::default()
    };
    let first = open(cfg.clone(), opts(true)).await.unwrap();
    let second = open(cfg.clone(), opts(true)).await;
    assert!(second.is_err());
    assert!(format!("{:#}", second.err().unwrap()).contains("another tomi-relay"));
    drop(first);
}

#[tokio::test]
async fn a_public_listen_address_is_refused_at_start() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::for_tests(dir.path());
    cfg.listen = "0.0.0.0:8080".into();
    let r = open(
        cfg.clone(),
        Options {
            background: false,
            hold_lock: false,
            ..Options::default()
        },
    )
    .await;
    assert!(r.is_err());
    cfg.allow_public_listen = true;
    assert!(open(
        cfg,
        Options {
            background: false,
            hold_lock: false,
            ..Options::default()
        }
    )
    .await
    .is_ok());
}

#[tokio::test]
async fn start_up_reconciles_the_blob_files() {
    let env = Env::new().await;
    let (devs, _) = env.space_of(1).await;
    let d = &devs[0];
    // A part half written when the process stopped: 1500 bytes on disk, 1000 recorded.
    let partial = new_uuid();
    env.create_blob(d, &partial, 4000).await.ok(201);
    env.patch_blob(d, &partial, 0, vec![1u8; 1000])
        .await
        .ok(204);
    let path = tomi_relay::blobstore::path(&env.space().dir, &uuid_bytes(&partial));
    use std::io::Write;
    std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(&[9u8; 500])
        .unwrap();
    // A file that no row knows.
    let orphan = uuid_bytes(&new_uuid());
    let orphan_path = tomi_relay::blobstore::path(&env.space().dir, &orphan);
    std::fs::create_dir_all(orphan_path.parent().unwrap()).unwrap();
    std::fs::write(&orphan_path, b"nobody's").unwrap();
    // A row whose file is shorter than recorded.
    let short = new_uuid();
    env.create_blob(d, &short, 4000).await.ok(201);
    env.patch_blob(d, &short, 0, vec![2u8; 1000]).await.ok(204);
    let short_path = tomi_relay::blobstore::path(&env.space().dir, &uuid_bytes(&short));
    std::fs::OpenOptions::new()
        .write(true)
        .open(&short_path)
        .unwrap()
        .set_len(400)
        .unwrap();

    let env = env.restart().await;
    assert_eq!(
        std::fs::metadata(&path).unwrap().len(),
        1000,
        "the half-written part is cut off"
    );
    assert!(!orphan_path.exists(), "the orphan file is removed");
    let mut d = same_device(d);
    env.login(&mut d, 1, 1).await;
    let st = env
        .get(&d, &format!("/v1/blobs/{short}/status"))
        .await
        .ok(200);
    assert_eq!(st["upload_offset"], 400, "the offset follows the file");
    let st = env
        .get(&d, &format!("/v1/blobs/{partial}/status"))
        .await
        .ok(200);
    assert_eq!(st["upload_offset"], 1000);
}

#[tokio::test]
async fn the_invitation_survives_restarts_until_the_space_exists() {
    let env = Env::new().await;
    let token = env.install_token.clone();
    assert!(!token.is_empty());
    let env = env.restart().await;
    // The relay keeps the hash of the token: the same token still works, and no second
    // invitation is made (a restart is not a fresh install).
    assert!(tomi_relay::install::is_pending(env.state.spaces.routing(), &token).unwrap());
    assert_eq!(
        tomi_relay::app::first_invitation(&env.state).unwrap(),
        None,
        "an invitation is pending: nothing new"
    );
    let mut founder = Dev::new("PC");
    env.create_space(&mut founder).await;
    let env = env.restart().await;
    assert!(
        tomi_relay::install::list(env.state.spaces.routing())
            .unwrap()
            .is_empty(),
        "the invitation went with the creation of the space"
    );
    assert_eq!(
        tomi_relay::app::first_invitation(&env.state).unwrap(),
        None,
        "and a relay that has a space makes none"
    );
    // The space and its session survive the restart: the routing is rebuilt from the databases.
    let mut founder = same_device(&founder);
    env.login(&mut founder, 1, 1).await;
}

#[tokio::test]
async fn a_fresh_install_gets_one_invitation_only_once() {
    let dir = tempfile::tempdir().unwrap();
    let env = Env::in_dir(dir, |_, _| {}).await;
    assert!(!env.install_token.is_empty(), "the first start made one");
    assert_eq!(
        tomi_relay::install::list(env.state.spaces.routing())
            .unwrap()
            .len(),
        1
    );
    let env = env.restart().await;
    assert_eq!(
        tomi_relay::install::list(env.state.spaces.routing())
            .unwrap()
            .len(),
        1,
        "the second start did not make another"
    );
}

#[tokio::test]
async fn the_legacy_install_token_file_becomes_an_invitation() {
    // The layout before the multi-space relay kept the token in clear in a file.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("install-token"), "h7k2-m9qv-d4xr-8tbn\n").unwrap();
    let env = Env::in_dir(dir, |_, _| {}).await;
    assert!(
        !env.data_dir().join("install-token").exists(),
        "the file is gone"
    );
    assert!(
        tomi_relay::install::is_pending(env.state.spaces.routing(), "h7k2m9qvd4xr8tbn").unwrap()
    );
    assert_eq!(env.install_token, "", "no new token: the old one is kept");
    let founder = Dev::new("PC");
    env.create_space_raw(&founder, "H7K2 M9QV D4XR 8TBN")
        .await
        .ok(201);
}

fn same_device(d: &Dev) -> Dev {
    Dev {
        keys: DeviceKeys::from_secrets(&d.keys.ed25519_seed(), &d.keys.x25519_secret()).unwrap(),
        id: d.id.clone(),
        name: d.name.clone(),
        token: d.token.clone(),
    }
}

// ===== the link that creates a space ==================================================

/// `https://<relay>/create#<token>` (or `/create/<token>`): one static page, for every form,
/// that the relay serves without reading the path.
#[tokio::test]
async fn the_create_link_page_is_static_script_free_and_the_same_for_every_path() {
    let env = Env::new().await;
    let token = "zq7createlinktokenmarker9f3a1c";
    let mut bodies = Vec::new();
    for path in [
        "/create".to_owned(),
        "/create/".to_owned(),
        "/create/xyz".to_owned(),
        format!("/create/{token}"),
        "/create/a/b/c".to_owned(),
        "/create?x=1".to_owned(),
    ] {
        let r = env.send("GET", &path, &[], None).await;
        assert_eq!(r.status, 200, "{path}");
        let ct = r.header("content-type").unwrap_or_default();
        assert!(ct.starts_with("text/html"), "{path}: {ct}");
        assert_eq!(
            r.header("content-security-policy").as_deref(),
            Some("default-src 'none'; style-src 'unsafe-inline'"),
            "{path}"
        );
        assert_eq!(r.header("referrer-policy").as_deref(), Some("no-referrer"));
        let body = String::from_utf8(r.body.to_vec()).unwrap();
        bodies.push((path, body));
    }
    let first = &bodies[0].1;
    for (path, body) in &bodies {
        assert_eq!(body, first, "{path}: the page does not depend on the path");
        assert!(!body.contains(token), "{path}: the token is never echoed");
    }
    let lower = first.to_ascii_lowercase();
    assert!(first.contains("Ouvre ce lien avec l'app Tomi pour créer ton espace"));
    // No script of any kind, nothing that reads the fragment, nothing loaded from elsewhere.
    for bad in [
        "<script",
        "location.hash",
        "javascript:",
        " onclick",
        "<img",
        "<iframe",
        "<link",
        "@import",
        "url(",
        "http://",
        "https://",
        "//cdn",
    ] {
        assert!(!lower.contains(bad), "the page must not contain {bad:?}");
    }
    // The only link goes to the installation page of the same relay.
    let hrefs: Vec<&str> = first.split("href=\"").skip(1).collect();
    assert!(!hrefs.is_empty(), "the page offers the download");
    for h in hrefs {
        assert!(h.starts_with("/l/\""), "a first-party link only: {h:.20}");
    }
    // Not a state-changing route.
    env.send("POST", "/create", &[], None)
        .await
        .err(405, "method_not_allowed");
}

#[tokio::test]
async fn the_landing_page_names_the_configured_host_and_escapes_it() {
    let env = Env::with(|cfg, _| cfg.public_url = "https://a<b>.example:8443/x".into()).await;
    let body = String::from_utf8(env.send("GET", "/", &[], None).await.body.to_vec()).unwrap();
    assert!(body.contains("<footer>a&lt;b&gt;.example:8443</footer>"));
    assert!(!body.contains("{{host}}"));
    let env = Env::new().await;
    let body = String::from_utf8(env.send("GET", "/", &[], None).await.body.to_vec()).unwrap();
    assert!(body.contains("<footer>relay.test</footer>"));
}
