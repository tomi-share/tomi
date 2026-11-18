//! `api-v1.md` §11.5 (items), §11.6 (log), §11.9 (purge) and §11.15 (pin events).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::result_large_err)]

mod common;

use common::*;
use serde_json::{json, Value};
use tomi_relay::maintenance;

fn item_events(events: &[Value]) -> Vec<&Value> {
    events.iter().filter(|e| e["type"] == "item").collect()
}

// ===== 11.5 =======================================================================

#[tokio::test]
async fn a40_an_item_reaches_the_others_unchanged() {
    let env = Env::new().await;
    let (devs, _) = env.space_of(3).await;
    let id = new_uuid();
    let envelope = Env::envelope(&uuid_bytes(&id), 1, 700);
    let body = env.item_body(&id, 1, 7 * DAY_MS, &envelope, &[]);
    let r = env.post(&devs[0], "/v1/items", body).await.ok(201);
    assert_eq!(r["seq"].as_i64().unwrap(), env.head_seq().await);
    assert_eq!(r["item_id"], id);
    assert_eq!(r["created_at"], ts(T0));
    for d in &devs[1..] {
        let events = env.events_of_type(d, "item").await;
        assert_eq!(events.len(), 1);
        let data = &events[0]["data"];
        assert_eq!(unb64(data["envelope"].as_str().unwrap()), envelope);
        assert_eq!(data["envelope_size"], 700);
        assert_eq!(data["sender_device_id"], devs[0].id);
        assert_eq!(data["key_epoch"], 1);
        assert_eq!(data["item_id"], id);
        assert_eq!(events[0]["seq"], r["seq"]);
    }
}

#[tokio::test]
async fn a41_replaying_an_item_changes_nothing() {
    let env = Env::new().await;
    let (devs, _) = env.space_of(2).await;
    let id = new_uuid();
    let body = env.item_body(
        &id,
        1,
        DAY_MS,
        &Env::envelope(&uuid_bytes(&id), 1, 300),
        &[],
    );
    let first = env.post(&devs[0], "/v1/items", body.clone()).await.ok(201);
    let head = env.head_seq().await;
    let again = env.post(&devs[0], "/v1/items", body).await.ok(200);
    assert_eq!(again["seq"], first["seq"]);
    assert_eq!(again["created_at"], first["created_at"]);
    assert_eq!(env.head_seq().await, head);
    assert!(again.get("purged").is_none());
}

#[tokio::test]
async fn a42_the_same_id_with_another_content_conflicts() {
    let env = Env::new().await;
    let (devs, _) = env.space_of(2).await;
    let id = new_uuid();
    let mut env1 = Env::envelope(&uuid_bytes(&id), 1, 300);
    env.post(
        &devs[0],
        "/v1/items",
        env.item_body(&id, 1, DAY_MS, &env1, &[]),
    )
    .await
    .ok(201);
    env1[100] ^= 1;
    env.post(
        &devs[0],
        "/v1/items",
        env.item_body(&id, 1, DAY_MS, &env1, &[]),
    )
    .await
    .err(409, "item_id_conflict");
    // Another device with the same content conflicts too.
    let env1 = Env::envelope(&uuid_bytes(&id), 1, 300);
    env.post(
        &devs[1],
        "/v1/items",
        env.item_body(&id, 1, DAY_MS, &env1, &[]),
    )
    .await
    .err(409, "item_id_conflict");
}

#[tokio::test]
async fn a43_a_purged_item_is_still_idempotent() {
    let env = Env::new().await;
    let (devs, _) = env.space_of(2).await;
    let id = new_uuid();
    let body = env.item_body(
        &id,
        1,
        7 * DAY_MS,
        &Env::envelope(&uuid_bytes(&id), 1, 300),
        &[],
    );
    let first = env.post(&devs[0], "/v1/items", body.clone()).await.ok(201);
    env.post(
        &devs[1],
        &format!("/v1/items/{id}/ack"),
        json!({ "release": true }),
    )
    .await
    .ok(200);
    env.clock.advance_secs(301);
    maintenance::run_once(&env.state).await;
    env.get(&devs[0], &format!("/v1/items/{id}"))
        .await
        .err(404, "not_found");
    let head = env.head_seq().await;
    let again = env.post(&devs[0], "/v1/items", body).await.ok(200);
    assert_eq!(again["purged"], true);
    assert_eq!(again["seq"], first["seq"]);
    assert_eq!(env.head_seq().await, head, "no new event");
}

#[tokio::test]
async fn a44_an_item_of_a_replaced_epoch_is_refused() {
    let env = Env::new().await;
    let (devs, ts) = env.space_of(2).await;
    let (statement, _) = env.rotation_for(&ts, &devs[0], &[]);
    env.post_rotation(&devs[0], &statement).await.ok(201);
    let id = new_uuid();
    let r = env
        .post(
            &devs[1],
            "/v1/items",
            env.item_body(
                &id,
                1,
                DAY_MS,
                &Env::envelope(&uuid_bytes(&id), 1, 300),
                &[],
            ),
        )
        .await;
    r.err(409, "stale_key_epoch");
    assert_eq!(r.json()["key_epoch"], 2);
    // The reserved range (>= 2^31) is never the current epoch either.
    let r = env
        .post(
            &devs[1],
            "/v1/items",
            env.item_body(
                &id,
                0x8000_0001,
                DAY_MS,
                &Env::envelope(&uuid_bytes(&id), 0x8000_0001, 300),
                &[],
            ),
        )
        .await;
    r.err(409, "stale_key_epoch");
}

#[tokio::test]
async fn a45_the_retention_deadline_is_bounded() {
    let env = Env::new().await;
    let (devs, _) = env.space_of(1).await;
    let id = new_uuid();
    let envelope = Env::envelope(&uuid_bytes(&id), 1, 300);
    let r = env
        .post(
            &devs[0],
            "/v1/items",
            env.item_body(&id, 1, -1000, &envelope, &[]),
        )
        .await;
    r.err(422, "already_expired");
    assert_eq!(r.json()["server_time"], ts(T0));
    let max = 2_592_000_000i64;
    let r = env
        .post(
            &devs[0],
            "/v1/items",
            env.item_body(&id, 1, max + 1000, &envelope, &[]),
        )
        .await;
    r.err(422, "validation_failed");
    // Exactly at the limit it is accepted.
    env.post(
        &devs[0],
        "/v1/items",
        env.item_body(&id, 1, max, &envelope, &[]),
    )
    .await
    .ok(201);
}

#[tokio::test]
async fn a46_an_envelope_over_the_limit_is_refused() {
    let env = Env::new().await;
    let (devs, _) = env.space_of(1).await;
    let id = new_uuid();
    let too_big = Env::envelope(&uuid_bytes(&id), 1, 1_114_113);
    let r = env
        .post(
            &devs[0],
            "/v1/items",
            env.item_body(&id, 1, DAY_MS, &too_big, &[]),
        )
        .await;
    r.err(413, "payload_too_large");
    assert_eq!(r.json()["limit"], 1_114_112);
    let at_limit = Env::envelope(&uuid_bytes(&id), 1, 1_114_112);
    env.post(
        &devs[0],
        "/v1/items",
        env.item_body(&id, 1, DAY_MS, &at_limit, &[]),
    )
    .await
    .ok(201);
}

#[tokio::test]
async fn a47_the_blobs_of_an_item_are_checked() {
    let env = Env::new().await;
    let (devs, _) = env.space_of(2).await;
    let (a, b) = (&devs[0], &devs[1]);
    // Not complete.
    let partial = new_uuid();
    env.create_blob(a, &partial, 2000).await.ok(201);
    env.patch_blob(a, &partial, 0, vec![1u8; 1000])
        .await
        .ok(204);
    let (_, r) = env.send_item(a, &[&partial]).await;
    r.err(409, "blob_incomplete");
    assert_eq!(r.json()["upload_offset"], 1000);
    // Another device's blob.
    let theirs = env.upload_blob(b, &[2u8; 500], 500).await;
    let (_, r) = env.send_item(a, &[&theirs]).await;
    r.err(404, "not_found");
    // An unknown blob.
    let (_, r) = env.send_item(a, &[&new_uuid()]).await;
    r.err(404, "not_found");
    // Already attached.
    let mine = env.upload_blob(a, &[3u8; 500], 500).await;
    let (_, first) = env.send_item(a, &[&mine]).await;
    first.ok(201);
    let (_, r) = env.send_item(a, &[&mine]).await;
    r.err(409, "blob_attached");
    // Duplicates in the list are a validation error.
    let other = env.upload_blob(a, &[4u8; 500], 500).await;
    let (_, r) = env.send_item(a, &[&other, &other]).await;
    r.err(422, "validation_failed");
}

#[tokio::test]
async fn a48_receipts_make_one_event_each() {
    let env = Env::new().await;
    let (devs, _) = env.space_of(2).await;
    let (id, r) = env.send_item(&devs[0], &[]).await;
    r.ok(201);
    let acked = env
        .post(&devs[1], &format!("/v1/items/{id}/ack"), json!({}))
        .await
        .ok(200);
    assert_eq!(acked["released"], false);
    let received_at = acked["received_at"].clone();
    env.clock.advance_secs(5);
    let again = env
        .post(&devs[1], &format!("/v1/items/{id}/ack"), json!({}))
        .await
        .ok(200);
    assert_eq!(again["received_at"], received_at, "the first date is kept");
    let events = env.all_events(&devs[0]).await;
    assert_eq!(events.iter().filter(|e| e["type"] == "ack").count(), 1);

    let read = env
        .post(&devs[1], &format!("/v1/items/{id}/read"), json!({}))
        .await
        .ok(200);
    assert!(read["read_at"].is_string());
    env.post(&devs[1], &format!("/v1/items/{id}/read"), json!({}))
        .await
        .ok(200);
    let events = env.all_events(&devs[0]).await;
    assert_eq!(
        events.iter().filter(|e| e["type"] == "ack").count(),
        1,
        "no second ack"
    );
    assert_eq!(events.iter().filter(|e| e["type"] == "read").count(), 1);
    // GET shows the receipt.
    let item = env.get(&devs[0], &format!("/v1/items/{id}")).await.ok(200);
    assert_eq!(item["receipts"][0]["device_id"], devs[1].id);
    assert!(item["receipts"][0]["read_at"].is_string());
}

#[tokio::test]
async fn a48b_a_read_without_a_prior_ack_makes_ack_then_read() {
    let env = Env::new().await;
    let (devs, _) = env.space_of(2).await;
    let (id, _) = env.send_item(&devs[0], &[]).await;
    env.post(&devs[1], &format!("/v1/items/{id}/read"), json!({}))
        .await
        .ok(200);
    let events = env.all_events(&devs[0]).await;
    let kinds: Vec<&str> = events.iter().map(|e| e["type"].as_str().unwrap()).collect();
    let ack = kinds.iter().position(|k| *k == "ack").unwrap();
    let read = kinds.iter().position(|k| *k == "read").unwrap();
    assert!(ack < read);
}

#[tokio::test]
async fn a49_the_sender_makes_no_receipt_of_its_own_item() {
    let env = Env::new().await;
    let (devs, _) = env.space_of(2).await;
    let (id, _) = env.send_item(&devs[0], &[]).await;
    let head = env.head_seq().await;
    let ack = env
        .post(&devs[0], &format!("/v1/items/{id}/ack"), json!({}))
        .await
        .ok(200);
    assert_eq!(ack["released"], false);
    assert!(ack.get("received_at").is_none());
    let read = env
        .post(&devs[0], &format!("/v1/items/{id}/read"), json!({}))
        .await
        .ok(200);
    assert!(read.get("read_at").is_none());
    assert_eq!(env.head_seq().await, head, "no event");
}

#[tokio::test]
async fn a50_a_later_device_does_not_see_earlier_items() {
    let env = Env::new().await;
    let (devs, _) = env.space_of(2).await;
    let blob = env.upload_blob(&devs[0], &[5u8; 800], 800).await;
    let (id, r) = env.send_item(&devs[0], &[&blob]).await;
    let item_seq = r.ok(201)["seq"].as_i64().unwrap();
    let mut late = Dev::new("Late");
    env.add_device(&devs[0], &mut late).await;
    env.get(&late, &format!("/v1/items/{id}"))
        .await
        .err(404, "not_found");
    env.post(&late, &format!("/v1/items/{id}/ack"), json!({}))
        .await
        .err(404, "not_found");
    env.post(&late, &format!("/v1/items/{id}/read"), json!({}))
        .await
        .err(404, "not_found");
    env.download(&late, &blob, None).await.err(404, "not_found");
    let record = env.get(&late, "/v1/devices").await.ok(200);
    let me = record["devices"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["device_id"] == late.id)
        .unwrap();
    let joined = me["joined_seq"].as_i64().unwrap();
    assert!(joined >= item_seq);
    let events = env.all_events(&late).await;
    assert!(events.iter().all(|e| e["seq"].as_i64().unwrap() > joined));
    assert!(item_events(&events).is_empty());
    // The others still see it.
    env.get(&devs[1], &format!("/v1/items/{id}")).await.ok(200);
}

// ===== 11.6 =======================================================================

#[tokio::test]
async fn a60_the_log_is_paged_by_seq() {
    let env = Env::new().await;
    let (devs, _) = env.space_of(1).await;
    // 249 renames + the founder's `device_added` = 250 events.
    for i in 0..249 {
        env.send(
            "PATCH",
            &format!("/v1/devices/{}", devs[0].id),
            &[("authorization", &format!("Bearer {}", devs[0].tok()))],
            Some(json!({ "name": format!("name-{i}") })),
        )
        .await
        .ok(200);
    }
    let mut after = 0i64;
    let mut seen = Vec::new();
    let mut pages = 0;
    loop {
        let page = env.events(&devs[0], after, 100).await;
        pages += 1;
        for e in page["events"].as_array().unwrap() {
            seen.push(e["seq"].as_i64().unwrap());
        }
        after = page["next_after"].as_i64().unwrap();
        if !page["has_more"].as_bool().unwrap() {
            assert_eq!(after, page["head_seq"].as_i64().unwrap());
            break;
        }
    }
    assert_eq!((seen.len(), pages), (250, 3));
    assert!(
        seen.windows(2).all(|w| w[0] < w[1]),
        "ordered, without duplicate"
    );
}

#[tokio::test]
async fn a62_a_purged_item_leaves_a_hole_that_is_not_an_error() {
    let env = Env::new().await;
    let (devs, _) = env.space_of(2).await;
    let h0 = env.head_seq().await;
    let (i1, _) = env.send_item(&devs[0], &[]).await;
    let (i2, _) = env.send_item(&devs[0], &[]).await;
    let (i3, _) = env.send_item(&devs[0], &[]).await;
    env.post_tombstone(&devs[0], &i2).await.ok(201);
    let page = env.events(&devs[1], h0, 100).await;
    let items: Vec<&str> = page["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["type"] == "item")
        .map(|e| e["data"]["item_id"].as_str().unwrap())
        .collect();
    assert_eq!(items, [i1.as_str(), i3.as_str()]);
    assert_eq!(page["next_after"], page["head_seq"]);
    assert_eq!(page["has_more"], false);
}

#[tokio::test]
async fn a63_the_log_is_compacted_after_45_days() {
    let env = Env::new().await;
    let (mut devs, _) = env.space_of(2).await;
    env.clock.advance_secs(46 * 86_400);
    for d in &mut devs {
        env.login(d, 1, 1).await;
    }
    maintenance::compact(&env.state).await;
    let space = env.get(&devs[0], "/v1/space").await.ok(200);
    let head = space["head_seq"].as_i64().unwrap();
    let r = env.get(&devs[0], "/v1/events?after=0").await;
    r.err(410, "cursor_expired");
    let floor = r.json()["retention_floor_seq"].as_i64().unwrap();
    assert!(floor > 0 && floor <= head);
    let ok = env
        .get(&devs[0], &format!("/v1/events?after={floor}"))
        .await
        .ok(200);
    assert_eq!(ok["events"], json!([]));
    // The state is still readable without the log.
    assert_eq!(
        env.get(&devs[0], "/v1/devices").await.ok(200)["devices"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

#[tokio::test]
async fn a64_a_cursor_ahead_of_the_log() {
    let env = Env::new().await;
    let (devs, _) = env.space_of(1).await;
    let head = env.head_seq().await;
    let r = env
        .get(&devs[0], &format!("/v1/events?after={}", head + 1000))
        .await;
    r.err(409, "cursor_ahead");
    assert_eq!(r.json()["head_seq"], head);
    env.get(&devs[0], &format!("/v1/events?after={head}"))
        .await
        .ok(200);
    // The parameters are validated.
    env.get(&devs[0], "/v1/events")
        .await
        .err(422, "validation_failed");
    env.get(&devs[0], "/v1/events?after=0&limit=501")
        .await
        .err(422, "validation_failed");
    env.get(&devs[0], "/v1/events?after=abc")
        .await
        .err(422, "validation_failed");
}

#[tokio::test]
async fn a65_a_page_stops_before_two_mebibytes_of_envelopes() {
    let env = Env::new().await;
    let (devs, _) = env.space_of(2).await;
    let h0 = env.head_seq().await;
    for _ in 0..10 {
        let id = new_uuid();
        let big = Env::envelope(&uuid_bytes(&id), 1, 409_600);
        env.post(
            &devs[0],
            "/v1/items",
            env.item_body(&id, 1, DAY_MS, &big, &[]),
        )
        .await
        .ok(201);
    }
    let page = env.events(&devs[1], h0, 100).await;
    let events = page["events"].as_array().unwrap();
    assert_eq!(events.iter().filter(|e| e["type"] == "item").count(), 5);
    assert_eq!(page["has_more"], true);
    // The next page continues where this one stopped.
    let next = env
        .events(&devs[1], page["next_after"].as_i64().unwrap(), 100)
        .await;
    assert_eq!(
        next["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| e["type"] == "item")
            .count(),
        5
    );
    assert_eq!(next["has_more"], false);
}

#[tokio::test]
async fn a66_an_unknown_event_type_is_served_as_it_is() {
    let env = Env::new().await;
    let (devs, _) = env.space_of(2).await;
    let h0 = env.head_seq().await;
    // An event of a type this relay never produces, as a future version could.
    let seq = env
        .db()
        .write_sync(|tx| {
            let seq = tomi_relay::store::add_event(
                tx,
                T0,
                "future_kind",
                None,
                None,
                &json!({ "x": 1 }),
            )?;
            Ok(tomi_relay::db::Tx::Commit(seq))
        })
        .unwrap();
    let (_, r) = env.send_item(&devs[0], &[]).await;
    r.ok(201);
    let page = env.events(&devs[1], h0, 100).await;
    let kinds: Vec<&str> = page["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["type"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["future_kind", "item"]);
    assert_eq!(page["events"][0]["seq"], seq);
    assert_eq!(page["events"][0]["data"], json!({ "x": 1 }));
    // The cursor goes past it without any error. (The reference client, `tomi-core`, maps an
    // unknown `type` to `EventData::Unknown` and advances: see its `http` module tests.)
    assert_eq!(page["next_after"], page["head_seq"]);
}

// ===== 11.9 =======================================================================

fn blob_file_exists(env: &Env, blob_id: &str) -> bool {
    tomi_relay::blobstore::path(&env.space().dir, &uuid_bytes(blob_id)).exists()
}

#[tokio::test]
async fn a90_an_item_goes_after_the_last_release_plus_the_grace() {
    let env = Env::new().await;
    let (devs, _) = env.space_of(3).await;
    let (a, b, c) = (&devs[0], &devs[1], &devs[2]);
    let blob = env.upload_blob(a, &[1u8; 600], 600).await;
    let (id, r) = env.send_item(a, &[&blob]).await;
    r.ok(201);
    assert!(blob_file_exists(&env, &blob));
    env.post(
        b,
        &format!("/v1/items/{id}/ack"),
        json!({ "release": true }),
    )
    .await
    .ok(200);
    env.clock.advance_secs(10);
    env.post(
        c,
        &format!("/v1/items/{id}/ack"),
        json!({ "release": true }),
    )
    .await
    .ok(200);
    // 299 s after the last release: still there.
    env.clock.advance_secs(299);
    maintenance::run_once(&env.state).await;
    env.get(a, &format!("/v1/items/{id}")).await.ok(200);
    assert!(blob_file_exists(&env, &blob));
    // 300 s: purged.
    env.clock.advance_secs(1);
    let report = maintenance::run_once(&env.state).await;
    assert_eq!(report.items_purged, 1);
    env.get(a, &format!("/v1/items/{id}"))
        .await
        .err(404, "not_found");
    assert!(!blob_file_exists(&env, &blob), "the blob file is gone");
    let events = env.all_events(b).await;
    assert!(
        item_events(&events).is_empty(),
        "the item event left the log"
    );
    assert_eq!(env.sql_count("SELECT count(*) FROM blobs"), 0);
}

#[tokio::test]
async fn a91_an_inactive_device_does_not_hold_the_purge() {
    let env = Env::new().await;
    let (mut devs, _) = env.space_of(3).await;
    env.clock.advance_secs(31 * 86_400);
    // C stays silent; A and B come back.
    env.login(&mut devs[0], 1, 1).await;
    env.login(&mut devs[1], 1, 1).await;
    let (id, r) = env.send_item(&devs[0], &[]).await;
    r.ok(201);
    env.post(
        &devs[1],
        &format!("/v1/items/{id}/ack"),
        json!({ "release": true }),
    )
    .await
    .ok(200);
    env.clock.advance_secs(301);
    maintenance::run_once(&env.state).await;
    env.get(&devs[0], &format!("/v1/items/{id}"))
        .await
        .err(404, "not_found");
}

#[tokio::test]
async fn a92_without_release_the_item_stays_until_its_deadline() {
    let env = Env::new().await;
    let (mut devs, _) = env.space_of(2).await;
    let id = new_uuid();
    let body = env.item_body(
        &id,
        1,
        2 * DAY_MS,
        &Env::envelope(&uuid_bytes(&id), 1, 300),
        &[],
    );
    env.post(&devs[0], "/v1/items", body).await.ok(201);
    env.post(&devs[1], &format!("/v1/items/{id}/ack"), json!({}))
        .await
        .ok(200);
    env.clock.advance_secs(86_400);
    for d in &mut devs {
        env.login(d, 1, 1).await;
    }
    maintenance::run_once(&env.state).await;
    env.get(&devs[0], &format!("/v1/items/{id}")).await.ok(200);
    env.clock.advance_secs(86_400 + 60);
    for d in &mut devs {
        env.login(d, 1, 1).await;
    }
    maintenance::run_once(&env.state).await;
    env.get(&devs[0], &format!("/v1/items/{id}"))
        .await
        .err(404, "not_found");
}

#[tokio::test]
async fn a93_a_tombstone_purges_at_once() {
    let env = Env::new().await;
    let (devs, _) = env.space_of(2).await;
    let blob = env.upload_blob(&devs[0], &[1u8; 600], 600).await;
    let (id, r) = env.send_item(&devs[0], &[&blob]).await;
    r.ok(201);
    let created = env.post_tombstone(&devs[1], &id).await.ok(201);
    assert_eq!(created["item_id"], id);
    env.get(&devs[0], &format!("/v1/items/{id}"))
        .await
        .err(404, "not_found");
    assert!(!blob_file_exists(&env, &blob));
    let events = env.all_events(&devs[0]).await;
    let tomb: Vec<_> = events.iter().filter(|e| e["type"] == "tombstone").collect();
    assert_eq!(tomb.len(), 1);
    assert_eq!(tomb[0]["seq"], created["seq"]);
    assert!(tomb[0]["data"]["tombstone"].is_string());
    assert_eq!(tomb[0]["data"]["device_id"], devs[1].id);
    assert!(item_events(&events).is_empty());
    // A second tombstone, from any device: the same answer, no new event.
    let head = env.head_seq().await;
    let second = env.post_tombstone(&devs[0], &id).await.ok(200);
    assert_eq!(second["seq"], created["seq"]);
    assert_eq!(env.head_seq().await, head);
}

#[tokio::test]
async fn a94_a_tombstone_for_an_unknown_item_is_accepted() {
    let env = Env::new().await;
    let (devs, _) = env.space_of(2).await;
    let id = new_uuid();
    env.post_tombstone(&devs[0], &id).await.ok(201);
    assert_eq!(env.events_of_type(&devs[1], "tombstone").await.len(), 1);
}

#[tokio::test]
async fn a95_a_bad_tombstone_purges_nothing() {
    let env = Env::new().await;
    let (devs, _) = env.space_of(2).await;
    let (id, r) = env.send_item(&devs[0], &[]).await;
    r.ok(201);
    let post = |bytes: Vec<u8>| {
        let (env, d) = (&env, &devs[1]);
        async move {
            env.post(d, "/v1/tombstones", json!({ "tombstone": b64(&bytes) }))
                .await
        }
    };
    // Wrong signature.
    let mut forged = env.tombstone(&devs[1].keys, &id, DeletionScope::Everywhere);
    let last = forged.len() - 1;
    forged[last] ^= 1;
    let r = post(forged).await;
    r.err(422, "validation_failed");
    assert_eq!(r.json()["errors"][0]["field"], "/tombstone");
    // Local scope.
    post(env.tombstone(&devs[1].keys, &id, DeletionScope::Local))
        .await
        .err(422, "validation_failed");
    // Signed by another device than the caller.
    post(env.tombstone(&devs[0].keys, &id, DeletionScope::Everywhere))
        .await
        .err(422, "validation_failed");
    // Another space.
    let foreign = devs[1]
        .keys
        .sign_tombstone(&new_uuid(), &id, 1, DeletionScope::Everywhere)
        .unwrap()
        .to_bytes()
        .unwrap();
    post(foreign).await.err(422, "validation_failed");
    post(vec![1, 2, 3]).await.err(422, "validation_failed");
    env.get(&devs[0], &format!("/v1/items/{id}")).await.ok(200);
    assert_eq!(env.sql_count("SELECT count(*) FROM tombstones"), 0);
}

#[tokio::test]
async fn a96_a_single_device_space_purges_after_the_grace() {
    let env = Env::new().await;
    let (devs, _) = env.space_of(1).await;
    let (id, r) = env.send_item(&devs[0], &[]).await;
    r.ok(201);
    env.clock.advance_secs(299);
    maintenance::run_once(&env.state).await;
    env.get(&devs[0], &format!("/v1/items/{id}")).await.ok(200);
    env.clock.advance_secs(2);
    maintenance::run_once(&env.state).await;
    env.get(&devs[0], &format!("/v1/items/{id}"))
        .await
        .err(404, "not_found");
}

#[tokio::test]
async fn a97_a_purge_frees_the_quota() {
    let env = Env::new().await;
    let (devs, _) = env.space_of(1).await;
    let before = env.get(&devs[0], "/v1/space").await.ok(200)["quota"]["used_bytes"]
        .as_u64()
        .unwrap();
    let blob = env.upload_blob(&devs[0], &[1u8; 1000], 1000).await;
    let (id, r) = env.send_item(&devs[0], &[&blob]).await;
    r.ok(201);
    let used = env.get(&devs[0], "/v1/space").await.ok(200)["quota"]["used_bytes"]
        .as_u64()
        .unwrap();
    assert_eq!(used - before, 300 + 1000, "envelope and blob");
    env.post_tombstone(&devs[0], &id).await.ok(201);
    let after = env.get(&devs[0], "/v1/space").await.ok(200)["quota"]["used_bytes"]
        .as_u64()
        .unwrap();
    assert_eq!(after, before);
}

// ===== 11.15 ======================================================================

/// A real sealed pin event built by `tomi-core` for `dev`.
fn real_pin(
    env: &Env,
    dev: &Dev,
    key: &SpaceKey,
    item_id: &str,
    pinned: bool,
) -> tomi_core::OutgoingItemState {
    let trust = TrustStore::new(&env.space_id, &dev.keys.public(), key.epoch()).unwrap();
    PinBook::new()
        .set_pinned(
            &dev.keys,
            &trust,
            key,
            item_id,
            pinned,
            env.clock.now_ms() as u64,
        )
        .unwrap()
        .unwrap()
}

async fn post_state(env: &Env, d: &Dev, event_id: &str, epoch: u32, envelope: &[u8]) -> Resp {
    env.post(
        d,
        "/v1/item-states",
        json!({ "event_id": event_id, "key_epoch": epoch, "envelope": b64(envelope) }),
    )
    .await
}

fn state_envelope(event_id: &str, epoch: u32, len: usize) -> Vec<u8> {
    let mut e = vec![0x5au8; len];
    e[..4].copy_from_slice(&1u32.to_be_bytes());
    e[4..8].copy_from_slice(&epoch.to_be_bytes());
    e[8..24].copy_from_slice(&uuid_bytes(event_id));
    e
}

#[tokio::test]
async fn a151_a_pin_travels_without_the_relay_learning_the_item() {
    let env = Env::new().await;
    let (devs, _) = env.space_of(2).await;
    let key = SpaceKey::generate(1).unwrap();
    let item_id = new_uuid();
    let out = real_pin(&env, &devs[0], &key, &item_id, true);
    assert_eq!(out.envelope.len(), 320);
    let r = post_state(&env, &devs[0], &out.event_id, 1, &out.envelope)
        .await
        .ok(201);
    assert_eq!(r["event_id"], out.event_id);
    let seq = r["seq"].as_i64().unwrap();
    assert_eq!(seq, env.head_seq().await);

    let events = env.events_of_type(&devs[1], "item_state").await;
    assert_eq!(events.len(), 1);
    let data = &events[0]["data"];
    assert_eq!(data["event_id"], out.event_id);
    assert_eq!(data["device_id"], devs[0].id);
    assert_eq!(data["key_epoch"], 1);
    assert_eq!(unb64(data["envelope"].as_str().unwrap()), out.envelope);
    assert_eq!(data["envelope_size"], 320);
    assert_eq!(events[0]["seq"], r["seq"]);

    // The relay holds the item id nowhere: not in a column, not in the files of the database.
    assert_eq!(
        env.sql_count(
            "SELECT count(*) FROM events WHERE type = 'item_state' AND item_id IS NOT NULL"
        ),
        0
    );
    let raw = uuid_bytes(&item_id);
    let hex = hex::encode(raw);
    for name in ["relay.db", "relay.db-wal"] {
        let bytes = std::fs::read(env.space().dir.join(name)).unwrap_or_default();
        let has = |needle: &[u8]| bytes.windows(needle.len()).any(|w| w == needle);
        assert!(!has(&raw), "raw item id found in {name}");
        assert!(!has(item_id.as_bytes()), "item id text found in {name}");
        assert!(!has(hex.as_bytes()), "item id hex found in {name}");
    }
}

#[tokio::test]
async fn a152_a_pin_is_idempotent() {
    let env = Env::new().await;
    let (devs, _) = env.space_of(2).await;
    let key = SpaceKey::generate(1).unwrap();
    let out = real_pin(&env, &devs[0], &key, &new_uuid(), true);
    let first = post_state(&env, &devs[0], &out.event_id, 1, &out.envelope)
        .await
        .ok(201);
    let head = env.head_seq().await;
    let again = post_state(&env, &devs[0], &out.event_id, 1, &out.envelope)
        .await
        .ok(200);
    assert_eq!(again["seq"], first["seq"]);
    assert_eq!(env.head_seq().await, head);
    // Same event_id, another envelope.
    let mut other = out.envelope.clone();
    other[100] ^= 1;
    post_state(&env, &devs[0], &out.event_id, 1, &other)
        .await
        .err(409, "event_id_conflict");
    // Same event_id and envelope, another device.
    post_state(&env, &devs[1], &out.event_id, 1, &out.envelope)
        .await
        .err(409, "event_id_conflict");
}

#[tokio::test]
async fn a153_the_envelope_of_a_pin_is_checked_like_the_clients_do() {
    let env = Env::new().await;
    let (devs, ts) = env.space_of(2).await;
    let (statement, _) = env.rotation_for(&ts, &devs[0], &[]);
    env.post_rotation(&devs[0], &statement).await.ok(201); // epoch 2

    // Stale epoch: nothing is recorded, and the same event_id is accepted afterwards.
    let id = new_uuid();
    let r = post_state(&env, &devs[0], &id, 1, &state_envelope(&id, 1, 320)).await;
    r.err(409, "stale_key_epoch");
    assert_eq!(r.json()["key_epoch"], 2);
    assert_eq!(env.sql_count("SELECT count(*) FROM item_states"), 0);
    post_state(&env, &devs[0], &id, 2, &state_envelope(&id, 2, 320))
        .await
        .ok(201);

    // The header must say what the body says.
    let id = new_uuid();
    let mut wrong_epoch = state_envelope(&id, 2, 320);
    wrong_epoch[7] = 9;
    let r = post_state(&env, &devs[0], &id, 2, &wrong_epoch).await;
    r.err(422, "validation_failed");
    assert_eq!(r.json()["errors"][0]["field"], "/envelope");
    let mut wrong_id = state_envelope(&id, 2, 320);
    wrong_id[8] ^= 1;
    post_state(&env, &devs[0], &id, 2, &wrong_id)
        .await
        .err(422, "validation_failed");

    // Lengths: 48 + 256*k + 16 with 1 <= k <= 8.
    for bad in [24usize, 319, 321, 577, 2368] {
        let id = new_uuid();
        post_state(&env, &devs[0], &id, 2, &state_envelope(&id, 2, bad))
            .await
            .err(422, "validation_failed");
    }
    let before = env.sql_count("SELECT count(*) FROM item_states");
    for good in [320usize, 2112] {
        let id = new_uuid();
        post_state(&env, &devs[0], &id, 2, &state_envelope(&id, 2, good))
            .await
            .ok(201);
    }
    assert_eq!(
        env.sql_count("SELECT count(*) FROM item_states"),
        before + 2,
        "the refused ones left nothing"
    );
    // Over max_item_state_bytes.
    let id = new_uuid();
    let r = post_state(&env, &devs[0], &id, 2, &state_envelope(&id, 2, 4097)).await;
    r.err(413, "payload_too_large");
    assert_eq!(r.json()["limit"], 4096);
    // A format the relay does not know: no other check than the size.
    let id = new_uuid();
    let mut future = state_envelope(&id, 2, 100);
    future[..4].copy_from_slice(&7u32.to_be_bytes());
    post_state(&env, &devs[0], &id, 2, &future).await.ok(201);
}

#[tokio::test]
async fn a154_a_pinned_item_is_purged_like_any_other() {
    let env = Env::new().await;
    let (devs, _) = env.space_of(2).await;
    let (id, r) = env.send_item(&devs[0], &[]).await;
    r.ok(201);
    let key = SpaceKey::generate(1).unwrap();
    let out = real_pin(&env, &devs[1], &key, &id, true);
    post_state(&env, &devs[1], &out.event_id, 1, &out.envelope)
        .await
        .ok(201);
    env.post(
        &devs[1],
        &format!("/v1/items/{id}/ack"),
        json!({ "release": true }),
    )
    .await
    .ok(200);
    env.clock.advance_secs(301);
    maintenance::run_once(&env.state).await;
    env.get(&devs[0], &format!("/v1/items/{id}"))
        .await
        .err(404, "not_found");
    // The state event stays in the log until event_retention_days.
    assert_eq!(env.events_of_type(&devs[0], "item_state").await.len(), 1);
    assert_eq!(env.sql_count("SELECT count(*) FROM item_states"), 1);
}

#[tokio::test]
async fn a155_a_revoked_device_cannot_pin_and_its_old_pins_are_refused_by_clients() {
    let env = Env::new().await;
    let (devs, mut ts) = env.space_of(3).await;
    let (a, b) = (&devs[0], &devs[1]);
    // A pin signed by B before its revocation.
    let item_id = new_uuid();
    let signed_by_b = b
        .keys
        .sign_pin(&env.space_id, &item_id, T0 as u64, true)
        .unwrap();
    let (statement, rotation) = env.rotation_for(&ts, a, &[b]);
    env.post_rotation(a, &statement).await.ok(201);
    // The relay refuses B from now on.
    let key = SpaceKey::generate(2).unwrap();
    let out = real_pin(&env, b, &key, &item_id, true);
    post_state(&env, b, &out.event_id, 2, &out.envelope)
        .await
        .err(403, "revoked");
    // A reference client applying the rotation ignores the old pin: `revoked_device`.
    assert!(
        ts.verify_pin(&signed_by_b).is_ok(),
        "before the rotation B is trusted"
    );
    ts.apply_rotation(&rotation, &a.keys).unwrap();
    assert_eq!(
        ts.verify_pin(&signed_by_b).unwrap_err().code(),
        "revoked_device"
    );
}
