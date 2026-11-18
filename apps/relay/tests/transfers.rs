//! `api-v1.md` §11.14: history transfers and the retention deadline of items.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::result_large_err)]

mod common;

use common::*;
use serde_json::{json, Value};
use tomi_relay::maintenance;

/// A trust store of `sender` that knows `others`, as the sender's own device would hold it.
fn trust_of(env: &Env, sender: &Dev, others: &[&Dev]) -> TrustStore {
    let mut ts = TrustStore::new(&env.space_id, &sender.keys.public(), 1).unwrap();
    for o in others {
        ts.add_verified_peer(&o.keys.public()).unwrap();
    }
    ts
}

/// A signed manifest from `sender` to `recipient` listing `parts`.
fn manifest(
    env: &Env,
    ts: &TrustStore,
    sender: &Dev,
    recipient: &Dev,
    transfer_id: &str,
    as_of_seq: u64,
    parts: &[String],
) -> Vec<u8> {
    let hk = HistoryKey::generate().unwrap();
    let parts: Vec<HistoryPart> = parts
        .iter()
        .enumerate()
        .map(|(i, id)| HistoryPart {
            blob_id: id.clone(),
            nonce_prefix: vec![u8::try_from(i).unwrap() + 1; 19],
        })
        .collect();
    ts.create_history_transfer(
        &sender.keys,
        &recipient.pk(),
        transfer_id,
        as_of_seq,
        env.clock.now_ms() as u64,
        &hk,
        &parts,
    )
    .unwrap()
    .to_bytes()
    .unwrap()
}

async fn post_transfer(
    env: &Env,
    d: &Dev,
    transfer_id: &str,
    manifest: &[u8],
    blobs: &[String],
) -> Resp {
    env.post(
        d,
        "/v1/transfers",
        json!({ "transfer_id": transfer_id, "manifest": b64(manifest), "blob_ids": blobs }),
    )
    .await
}

async fn joined_seq(env: &Env, d: &Dev) -> u64 {
    let list = env.get(d, "/v1/devices").await.ok(200);
    list["devices"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["device_id"] == d.id)
        .unwrap()["joined_seq"]
        .as_u64()
        .unwrap()
}

struct Setup {
    env: Env,
    devs: Vec<Dev>,
    ts: TrustStore,
    /// Two index parts and one file, all uploaded by A.
    blobs: Vec<String>,
}

async fn setup() -> Setup {
    let env = Env::new().await;
    let (devs, ts) = env.space_of(3).await;
    let mut blobs = Vec::new();
    for n in 0..3u8 {
        blobs.push(env.upload_blob(&devs[0], &vec![n + 1; 3000], 3000).await);
    }
    Setup {
        env,
        devs,
        ts,
        blobs,
    }
}

impl Setup {
    /// A valid transfer from A to B: the answer and its id.
    async fn transfer(&self, id: &str, blobs: &[String]) -> Resp {
        let m = manifest(
            &self.env,
            &self.ts,
            &self.devs[0],
            &self.devs[1],
            id,
            self.env.head_seq().await as u64,
            &blobs[..2],
        );
        post_transfer(&self.env, &self.devs[0], id, &m, blobs).await
    }
}

#[tokio::test]
async fn a140_a_transfer_is_for_its_recipient_only() {
    let s = setup().await;
    let (a, b, c) = (&s.devs[0], &s.devs[1], &s.devs[2]);
    let (addr, stop, _server) = serve(&s.env).await;
    let mut ws_b = Ws::open(addr, b).await;
    let mut ws_c = Ws::open(addr, c).await;
    let id = new_uuid();
    let r = s.transfer(&id, &s.blobs).await.ok(201);
    assert_eq!(r["transfer_id"], id);
    assert_eq!(r["expires_at"], ts(T0 + 7 * 86_400_000));

    // The event: addressed to B.
    let events = s.env.events_of_type(c, "history_transfer").await;
    assert_eq!(events.len(), 1, "the log is common");
    assert_eq!(events[0]["data"]["to_device_id"], b.id);
    assert_eq!(events[0]["data"]["from_device_id"], a.id);
    assert_eq!(events[0]["data"]["total_bytes"], 9000);
    assert_eq!(events[0]["seq"], r["seq"]);
    // The doorbell rings for B only.
    let wake = ws_b.recv_json(1000).await.unwrap();
    assert_eq!(wake["type"], "wake");
    assert!(wake["seq"].as_i64().unwrap() >= r["seq"].as_i64().unwrap());
    assert!(ws_c.recv_json(300).await.is_none(), "C is not rung");

    // B reads the transfer and every blob; C gets 404 on all of it.
    let got = s.env.get(b, &format!("/v1/transfers/{id}")).await.ok(200);
    assert_eq!(got["from_device_id"], a.id);
    assert_eq!(got["to_device_id"], b.id);
    assert_eq!(got["blobs"].as_array().unwrap().len(), 3);
    assert_eq!(got["blobs"][0]["blob_id"], s.blobs[0]);
    assert_eq!(got["blobs"][2]["size"], 3000);
    assert!(got["manifest"].is_string());
    for blob in &s.blobs {
        assert_eq!(s.env.download(b, blob, None).await.status, 200);
        s.env.download(c, blob, None).await.err(404, "not_found");
    }
    s.env
        .get(c, &format!("/v1/transfers/{id}"))
        .await
        .err(404, "not_found");
    // The sender reads its own transfer.
    s.env.get(a, &format!("/v1/transfers/{id}")).await.ok(200);
    let _ = stop.send(());
}

#[tokio::test]
async fn a141_a_bad_manifest_is_refused() {
    let s = setup().await;
    let (a, b, c) = (&s.devs[0], &s.devs[1], &s.devs[2]);
    let head = s.env.head_seq().await as u64;
    let parts = &s.blobs[..2];
    let id = new_uuid();
    let field = |r: &Resp| r.json()["errors"][0]["field"].as_str().unwrap().to_owned();

    // Signed by another device than the caller.
    let ts_c = trust_of(&s.env, c, &[a, b]);
    let by_c = manifest(&s.env, &ts_c, c, b, &id, head, parts);
    let r = post_transfer(&s.env, a, &id, &by_c, &s.blobs).await;
    r.err(422, "validation_failed");
    assert_eq!(field(&r), "/manifest");
    // Another space.
    let other_space = TrustStore::new(&new_uuid(), &a.keys.public(), 1).unwrap();
    let mut other_space = other_space;
    other_space.add_verified_peer(&b.keys.public()).unwrap();
    let foreign = manifest(&s.env, &other_space, a, b, &id, head, parts);
    post_transfer(&s.env, a, &id, &foreign, &s.blobs)
        .await
        .err(422, "validation_failed");
    // The transfer_id of the manifest differs from the body's.
    let m = manifest(&s.env, &s.ts, a, b, &id, head, parts);
    post_transfer(&s.env, a, &new_uuid(), &m, &s.blobs)
        .await
        .err(422, "validation_failed");
    // The recipient is not a member (revoked).
    let (rot, _) = s.env.rotation_for(&s.ts, a, &[c]);
    s.env.post_rotation(a, &rot).await.ok(201);
    let to_revoked = manifest(
        &s.env,
        &s.ts,
        a,
        c,
        &id,
        s.env.head_seq().await as u64,
        parts,
    );
    post_transfer(&s.env, a, &id, &to_revoked, &s.blobs)
        .await
        .err(422, "validation_failed");
    // Bytes that are no manifest.
    post_transfer(&s.env, a, &id, &[1, 2, 3], &s.blobs)
        .await
        .err(422, "validation_failed");
    assert_eq!(s.env.sql_count("SELECT count(*) FROM transfers"), 0);
}

#[tokio::test]
async fn a142_the_snapshot_and_the_parts_are_checked() {
    let s = setup().await;
    let (a, b) = (&s.devs[0], &s.devs[1]);
    let head = s.env.head_seq().await as u64;
    let joined = joined_seq(&s.env, b).await;
    let parts = &s.blobs[..2];
    let id = new_uuid();
    // as_of_seq below the recipient's arrival, above head_seq.
    let m = manifest(&s.env, &s.ts, a, b, &id, joined - 1, parts);
    let r = post_transfer(&s.env, a, &id, &m, &s.blobs).await;
    r.err(422, "validation_failed");
    assert_eq!(r.json()["errors"][0]["field"], "/manifest");
    let m = manifest(&s.env, &s.ts, a, b, &id, head + 1, parts);
    post_transfer(&s.env, a, &id, &m, &s.blobs)
        .await
        .err(422, "validation_failed");
    // A part of the manifest missing from blob_ids.
    let m = manifest(&s.env, &s.ts, a, b, &id, head, parts);
    post_transfer(&s.env, a, &id, &m, &s.blobs[1..])
        .await
        .err(422, "validation_failed");
    // The two bounds are accepted.
    let m = manifest(&s.env, &s.ts, a, b, &id, joined, parts);
    post_transfer(&s.env, a, &id, &m, &s.blobs).await.ok(201);
}

#[tokio::test]
async fn a143_a_transfer_is_idempotent() {
    let s = setup().await;
    let (a, b) = (&s.devs[0], &s.devs[1]);
    let id = new_uuid();
    let m = manifest(
        &s.env,
        &s.ts,
        a,
        b,
        &id,
        s.env.head_seq().await as u64,
        &s.blobs[..2],
    );
    let first = post_transfer(&s.env, a, &id, &m, &s.blobs).await.ok(201);
    let head = s.env.head_seq().await;
    let again = post_transfer(&s.env, a, &id, &m, &s.blobs).await.ok(200);
    assert_eq!(again["seq"], first["seq"]);
    assert_eq!(again["expires_at"], first["expires_at"]);
    assert_eq!(s.env.head_seq().await, head, "no new event");
    // The blob ids in another order are the same set.
    let mut reordered = s.blobs.clone();
    reordered.reverse();
    post_transfer(&s.env, a, &id, &m, &reordered).await.ok(200);
    // Another manifest, another set of blobs: conflicts.
    let other_m = manifest(
        &s.env,
        &s.ts,
        a,
        b,
        &id,
        s.env.head_seq().await as u64,
        &s.blobs[..2],
    );
    post_transfer(&s.env, a, &id, &other_m, &s.blobs)
        .await
        .err(409, "transfer_id_conflict");
    post_transfer(&s.env, a, &id, &m, &s.blobs[..2])
        .await
        .err(409, "transfer_id_conflict");
}

#[tokio::test]
async fn a144_the_recipient_closes_a_transfer() {
    let s = setup().await;
    let (a, b) = (&s.devs[0], &s.devs[1]);
    let id = new_uuid();
    s.transfer(&id, &s.blobs).await.ok(201);
    let used = s.env.get(a, "/v1/space").await.ok(200)["quota"]["used_bytes"]
        .as_u64()
        .unwrap();
    assert!(used >= 9000);
    s.env
        .bearer(
            "DELETE",
            &format!("/v1/transfers/{id}"),
            b.tok(),
            Some(json!({ "outcome": "imported" })),
        )
        .await
        .ok(204);
    // Blobs gone from the base and the disk; the quota went down by their size.
    assert_eq!(s.env.sql_count("SELECT count(*) FROM blobs"), 0);
    for blob in &s.blobs {
        assert!(!tomi_relay::blobstore::path(&s.env.space().dir, &uuid_bytes(blob)).exists());
    }
    let after = s.env.get(a, "/v1/space").await.ok(200)["quota"]["used_bytes"]
        .as_u64()
        .unwrap();
    assert_eq!(used - after, 9000);
    let closed = s.env.events_of_type(a, "history_transfer_closed").await;
    assert_eq!(closed.len(), 1);
    assert_eq!(closed[0]["data"]["by_device_id"], b.id);
    assert_eq!(closed[0]["data"]["outcome"], "imported");
    s.env
        .get(b, &format!("/v1/transfers/{id}"))
        .await
        .err(404, "not_found");
    s.env
        .download(b, &s.blobs[0], None)
        .await
        .err(404, "not_found");
    // Again: 204, no new event.
    let head = s.env.head_seq().await;
    s.env
        .delete(b, &format!("/v1/transfers/{id}"))
        .await
        .ok(204);
    assert_eq!(s.env.head_seq().await, head);
    // The sender cancels by default; a stranger to the transfer gets 404.
    let id2 = new_uuid();
    let fresh = s.env.upload_blob(a, &[7u8; 100], 100).await;
    let fresh2 = s.env.upload_blob(a, &[8u8; 100], 100).await;
    let m = manifest(
        &s.env,
        &s.ts,
        a,
        b,
        &id2,
        s.env.head_seq().await as u64,
        &[fresh.clone(), fresh2.clone()],
    );
    post_transfer(&s.env, a, &id2, &m, &[fresh, fresh2])
        .await
        .ok(201);
    s.env
        .delete(&s.devs[2], &format!("/v1/transfers/{id2}"))
        .await
        .err(404, "not_found");
    s.env
        .delete(a, &format!("/v1/transfers/{id2}"))
        .await
        .ok(204);
    let closed = s.env.events_of_type(a, "history_transfer_closed").await;
    assert_eq!(closed[1]["data"]["outcome"], "cancelled");
    s.env
        .delete(a, &format!("/v1/transfers/{}", new_uuid()))
        .await
        .err(404, "not_found");
}

#[tokio::test]
async fn a145_a_transfer_expires_and_dies_with_its_devices() {
    // After seven days.
    let s = setup().await;
    let id = new_uuid();
    s.transfer(&id, &s.blobs).await.ok(201);
    s.env.clock.advance_secs(7 * 86_400 - 1);
    maintenance::run_once(&s.env.state).await;
    assert_eq!(
        s.env
            .sql_count("SELECT count(*) FROM transfers WHERE closed_at IS NULL"),
        1
    );
    s.env.clock.advance_secs(2);
    let report = maintenance::run_once(&s.env.state).await;
    assert_eq!(report.transfers_closed, 1);
    assert_eq!(s.env.sql_count("SELECT count(*) FROM blobs"), 0);
    // Sessions have expired meanwhile.
    let mut a = Dev {
        keys: DeviceKeys::from_secrets(
            &s.devs[0].keys.ed25519_seed(),
            &s.devs[0].keys.x25519_secret(),
        )
        .unwrap(),
        id: s.devs[0].id.clone(),
        name: "a".into(),
        token: None,
    };
    s.env.login(&mut a, 1, 1).await;
    let closed = s.env.events_of_type(&a, "history_transfer_closed").await;
    assert_eq!(closed.len(), 1);
    assert!(closed[0]["data"]["by_device_id"].is_null());
    assert_eq!(closed[0]["data"]["outcome"], "expired");

    // The revocation of the recipient, then of the sender, purges in the same transaction.
    for revoke_sender in [false, true] {
        let s = setup().await;
        let id = new_uuid();
        s.transfer(&id, &s.blobs).await.ok(201);
        let (issuer, target) = if revoke_sender {
            (&s.devs[1], &s.devs[0])
        } else {
            (&s.devs[0], &s.devs[1])
        };
        let ts_issuer = trust_of(
            &s.env,
            issuer,
            &s.devs
                .iter()
                .filter(|d| d.id != issuer.id)
                .collect::<Vec<_>>(),
        );
        let (rot, _) = s.env.rotation_for(&ts_issuer, issuer, &[target]);
        s.env.post_rotation(issuer, &rot).await.ok(201);
        assert_eq!(s.env.sql_count("SELECT count(*) FROM blobs"), 0);
        let closed = s
            .env
            .events_of_type(issuer, "history_transfer_closed")
            .await;
        assert_eq!(closed.len(), 1, "revoke_sender = {revoke_sender}");
        assert_eq!(closed[0]["data"]["outcome"], "revoked");
        assert!(closed[0]["data"]["by_device_id"].is_null());
        // The `history_transfer_closed` event precedes the `device_revoked` one.
        let events = s.env.all_events(issuer).await;
        let pos = |kind: &str| events.iter().position(|e| e["type"] == kind).unwrap();
        assert!(pos("history_transfer_closed") < pos("device_revoked"));
    }
}

#[tokio::test]
async fn a146_limits_of_a_transfer() {
    // Total size.
    let env = Env::with(|cfg, _| cfg.max_transfer_bytes = 1000).await;
    let (devs, ts) = env.space_of(2).await;
    let b1 = env.upload_blob(&devs[0], &[1u8; 600], 600).await;
    let b2 = env.upload_blob(&devs[0], &[2u8; 600], 600).await;
    let id = new_uuid();
    let m = manifest(
        &env,
        &ts,
        &devs[0],
        &devs[1],
        &id,
        env.head_seq().await as u64,
        std::slice::from_ref(&b1),
    );
    let r = post_transfer(&env, &devs[0], &id, &m, &[b1, b2]).await;
    r.err(413, "payload_too_large");
    assert_eq!(r.json()["limit"], 1000);

    // Quota exceeded when the blobs are created.
    let env = Env::with(|cfg, _| cfg.max_space_bytes = Some(1500)).await;
    let (devs, _) = env.space_of(1).await;
    env.create_blob(&devs[0], &new_uuid(), 1000).await.ok(201);
    env.create_blob(&devs[0], &new_uuid(), 600)
        .await
        .err(507, "relay_full");

    // A third open transfer towards the same device.
    let s = setup().await;
    let mut extra = Vec::new();
    for n in 0..4u8 {
        extra.push(s.env.upload_blob(&s.devs[0], &[n; 100], 100).await);
    }
    let (t1, t2, t3) = (new_uuid(), new_uuid(), new_uuid());
    for (t, pair) in [(&t1, &extra[0..2]), (&t2, &extra[2..4])] {
        let m = manifest(
            &s.env,
            &s.ts,
            &s.devs[0],
            &s.devs[1],
            t,
            s.env.head_seq().await as u64,
            pair,
        );
        post_transfer(&s.env, &s.devs[0], t, &m, pair).await.ok(201);
    }
    let m = manifest(
        &s.env,
        &s.ts,
        &s.devs[0],
        &s.devs[1],
        &t3,
        s.env.head_seq().await as u64,
        &s.blobs[..2],
    );
    let r = post_transfer(&s.env, &s.devs[0], &t3, &m, &s.blobs).await;
    r.err(409, "transfer_limit");
    assert_eq!(r.json()["transfer_max_open"], 2);
    // Towards another device it is fine.
    let m = manifest(
        &s.env,
        &s.ts,
        &s.devs[0],
        &s.devs[2],
        &t3,
        s.env.head_seq().await as u64,
        &s.blobs[..2],
    );
    post_transfer(&s.env, &s.devs[0], &t3, &m, &s.blobs)
        .await
        .ok(201);
    // Bounds of blob_ids.
    let t4 = new_uuid();
    let m = manifest(
        &s.env,
        &s.ts,
        &s.devs[0],
        &s.devs[2],
        &t4,
        s.env.head_seq().await as u64,
        &s.blobs[..2],
    );
    post_transfer(&s.env, &s.devs[0], &t4, &m, &[])
        .await
        .err(422, "validation_failed");
}

#[tokio::test]
async fn a147_a_blob_has_one_owner_item_or_transfer() {
    let s = setup().await;
    let (a, b) = (&s.devs[0], &s.devs[1]);
    let id = new_uuid();
    s.transfer(&id, &s.blobs).await.ok(201);
    // Attached to a transfer: no item, no deletion.
    let (_, r) = s.env.send_item(a, &[&s.blobs[0]]).await;
    r.err(409, "blob_attached");
    s.env
        .delete(a, &format!("/v1/blobs/{}", s.blobs[0]))
        .await
        .err(409, "blob_attached");
    // Attached to an item: not in a transfer.
    let file = s.env.upload_blob(a, &[9u8; 100], 100).await;
    let part1 = s.env.upload_blob(a, &[3u8; 100], 100).await;
    let (_, r) = s.env.send_item(a, &[&file]).await;
    r.ok(201);
    let t2 = new_uuid();
    let m = manifest(
        &s.env,
        &s.ts,
        a,
        b,
        &t2,
        s.env.head_seq().await as u64,
        std::slice::from_ref(&part1),
    );
    post_transfer(&s.env, a, &t2, &m, &[part1, file])
        .await
        .err(409, "blob_attached");
    // An incomplete blob, and the blob of another device.
    let partial = new_uuid();
    s.env.create_blob(a, &partial, 500).await.ok(201);
    let ok = s.env.upload_blob(a, &[1u8; 50], 50).await;
    let m = manifest(
        &s.env,
        &s.ts,
        a,
        b,
        &t2,
        s.env.head_seq().await as u64,
        std::slice::from_ref(&ok),
    );
    post_transfer(&s.env, a, &t2, &m, &[ok.clone(), partial])
        .await
        .err(409, "blob_incomplete");
    let theirs = s.env.upload_blob(b, &[1u8; 50], 50).await;
    post_transfer(&s.env, a, &t2, &m, &[ok, theirs])
        .await
        .err(404, "not_found");
}

#[tokio::test]
async fn a148_unknown_fields_are_ignored_there_is_no_partial_transfer() {
    let s = setup().await;
    let (a, b) = (&s.devs[0], &s.devs[1]);
    let id = new_uuid();
    let m = manifest(
        &s.env,
        &s.ts,
        a,
        b,
        &id,
        s.env.head_seq().await as u64,
        &s.blobs[..2],
    );
    let r = s
        .env
        .post(
            a,
            "/v1/transfers",
            json!({ "transfer_id": id, "manifest": b64(&m), "blob_ids": s.blobs, "include_secrets": false, "include_files": false }),
        )
        .await;
    r.ok(201);
}

#[tokio::test]
async fn a149_no_item_of_the_old_epoch_follows_the_rotation() {
    let env = Env::new().await;
    let (devs, ts) = env.space_of(3).await;
    let (rotation, _) = env.rotation_for(&ts, &devs[0], &[]);
    let mut posts = Vec::new();
    for i in 0..24 {
        let sender = &devs[1 + i % 2];
        let env = &env;
        posts.push(async move {
            let (_, r) = env.send_item_epoch(sender, 1).await;
            r
        });
    }
    let rotate = env.post_rotation(&devs[0], &rotation);
    let (results, rot) = tokio::join!(futures_util::future::join_all(posts), rotate);
    let rot = rot.ok(201);
    let rotation_seq = rot["seq"].as_i64().unwrap();
    let mut accepted = 0;
    for r in results {
        match r.status {
            201 => {
                accepted += 1;
                assert!(
                    r.json()["seq"].as_i64().unwrap() < rotation_seq,
                    "an epoch-1 item after the rotation"
                );
            }
            409 => assert_eq!(r.code(), "stale_key_epoch"),
            other => panic!("unexpected status {other}"),
        }
    }
    // And the log agrees: every `item` event of epoch 1 precedes the `rotation` event.
    for e in env.all_events(&devs[0]).await {
        if e["type"] == "item" && e["data"]["key_epoch"] == 1 {
            assert!(e["seq"].as_i64().unwrap() < rotation_seq);
        }
    }
    let _ = accepted;
}

#[tokio::test]
async fn a150_the_deadline_the_reference_client_sends_is_accepted() {
    let env = Env::new().await;
    let (devs, _) = env.space_of(1).await;
    let hello: Value = env.send("GET", "/v1/hello", &[], None).await.ok(200);
    let server_ms = tomi_relay::wire::ts_parse(hello["server_time"].as_str().unwrap()).unwrap();
    let max_ttl = hello["limits"]["max_ttl_secs"].as_u64().unwrap();

    // No expiration wanted by the user: now + max_ttl - 60 s.
    let none = tomi_core::relay_expires_at_ms(None, server_ms, max_ttl);
    assert!((none - (server_ms + 2_592_000_000 - 60_000)).abs() <= 1000);
    let id = new_uuid();
    let envelope = Env::envelope(&uuid_bytes(&id), 1, 300);
    let r = env
        .post(&devs[0], "/v1/items", json!({ "item_id": id, "expires_at": ts(none), "key_epoch": 1, "envelope": b64(&envelope) }))
        .await
        .ok(201);
    assert_eq!(r["expires_at"], ts(none));

    // An expiration wanted in one hour is sent as it is.
    let wanted = server_ms + 3_600_000;
    let one_hour = tomi_core::relay_expires_at_ms(Some(wanted), server_ms, max_ttl);
    assert_eq!(one_hour, wanted);
    let id = new_uuid();
    let envelope = Env::envelope(&uuid_bytes(&id), 1, 300);
    let r = env
        .post(&devs[0], "/v1/items", json!({ "item_id": id, "expires_at": ts(one_hour), "key_epoch": 1, "envelope": b64(&envelope) }))
        .await
        .ok(201);
    assert_eq!(r["expires_at"], ts(wanted));
}

impl Env {
    /// Sends an item sealed for `epoch` (the content is filler).
    pub async fn send_item_epoch(&self, d: &Dev, epoch: u32) -> (String, Resp) {
        let id = new_uuid();
        let env = Env::envelope(&uuid_bytes(&id), epoch, 300);
        let r = self
            .post(
                d,
                "/v1/items",
                self.item_body(&id, epoch, DAY_MS, &env, &[]),
            )
            .await;
        (id, r)
    }
}
