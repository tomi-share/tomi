//! `api-v1.md` §11.19: the removal of ONE attachment (`POST /v1/attachment-tombstones`, §4.8 bis).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::result_large_err)]

mod common;

use common::*;
use serde_json::json;
use tomi_core::DeletionScope;
use tomi_relay::maintenance;

fn blob_file_exists(env: &Env, blob_id: &str) -> bool {
    tomi_relay::blobstore::path(&env.space().dir, &uuid_bytes(blob_id)).exists()
}

/// An item with two blobs, sent by `devs[0]`.
async fn item_with_two_blobs(env: &Env, d: &Dev) -> (String, String, String) {
    let b0 = env.upload_blob(d, &[1u8; 600], 600).await;
    let b1 = env.upload_blob(d, &[2u8; 700], 700).await;
    let (id, r) = env.send_item(d, &[&b0, &b1]).await;
    r.ok(201);
    (id, b0, b1)
}

#[tokio::test]
async fn a196_one_attachment_goes_and_the_others_stay() {
    let env = Env::new().await;
    let (devs, _) = env.space_of(2).await;
    let (id, b0, b1) = item_with_two_blobs(&env, &devs[0]).await;
    let used_before = env.get(&devs[0], "/v1/space").await.ok(200)["quota"]["used_bytes"]
        .as_i64()
        .unwrap();

    let created = env
        .post_attachment_tombstone(&devs[1], &id, &b1, 1)
        .await
        .ok(201);
    assert_eq!(created["item_id"], id);
    assert_eq!(created["slot"], 1);
    // The blob of slot 1 is gone, the item and the blob of slot 0 stay.
    assert!(!blob_file_exists(&env, &b1));
    assert!(blob_file_exists(&env, &b0));
    env.get(&devs[0], &format!("/v1/items/{id}")).await.ok(200);
    let used_after = env.get(&devs[0], "/v1/space").await.ok(200)["quota"]["used_bytes"]
        .as_i64()
        .unwrap();
    assert!(used_after < used_before, "the quota is freed");

    // One event, with the signed statement.
    let events = env.events_of_type(&devs[0], "attachment_tombstone").await;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["seq"], created["seq"]);
    let data = &events[0]["data"];
    assert_eq!(data["item_id"], id);
    assert_eq!(data["slot"], 1);
    assert_eq!(data["blob_id"], b1);
    assert_eq!(data["device_id"], devs[1].id);
    assert!(data["deleted_at"].is_string());
    assert!(data["statement"].is_string());
    // The `item` event of the log keeps its list of blobs (it is not rewritten).
    assert_eq!(env.events_of_type(&devs[1], "item").await.len(), 1);

    // The same removal again, from any device: the same answer, no new event.
    let head = env.head_seq().await;
    let again = env
        .post_attachment_tombstone(&devs[0], &id, &b1, 1)
        .await
        .ok(200);
    assert_eq!(again["seq"], created["seq"]);
    assert_eq!(env.head_seq().await, head);
    // Another slot of the same item is another removal.
    env.post_attachment_tombstone(&devs[0], &id, &b0, 0)
        .await
        .ok(201);
    assert!(!blob_file_exists(&env, &b0));
    assert_eq!(
        env.sql_count("SELECT count(*) FROM attachment_tombstones"),
        2
    );
}

#[tokio::test]
async fn a197_a_bad_statement_changes_nothing() {
    let env = Env::new().await;
    let (devs, _) = env.space_of(2).await;
    let (id, b0, _b1) = item_with_two_blobs(&env, &devs[0]).await;
    let post = |bytes: Vec<u8>| {
        let (env, d) = (&env, &devs[1]);
        async move {
            env.post(
                d,
                "/v1/attachment-tombstones",
                json!({ "attachment_tombstone": b64(&bytes) }),
            )
            .await
        }
    };
    // Wrong signature.
    let mut forged = env.attachment_tombstone(&devs[1].keys, &id, &b0, 0);
    let last = forged.len() - 1;
    forged[last] ^= 1;
    let r = post(forged).await;
    r.err(422, "validation_failed");
    assert_eq!(r.json()["errors"][0]["field"], "/attachment_tombstone");
    // Signed by another device than the caller.
    post(env.attachment_tombstone(&devs[0].keys, &id, &b0, 0))
        .await
        .err(422, "validation_failed");
    // Another space.
    let foreign = devs[1]
        .keys
        .sign_attachment_tombstone(&new_uuid(), &id, &b0, 0, 1)
        .unwrap()
        .to_bytes()
        .unwrap();
    post(foreign).await.err(422, "validation_failed");
    // An item tombstone is not an attachment removal, nor garbage, nor an empty statement.
    post(env.tombstone(&devs[1].keys, &id, DeletionScope::Everywhere))
        .await
        .err(422, "validation_failed");
    post(vec![1, 2, 3]).await.err(422, "validation_failed");
    post(Vec::new()).await.err(422, "validation_failed");
    // A time above 2^53 - 1 cannot even be signed by the core.
    assert!(devs[1]
        .keys
        .sign_attachment_tombstone(&env.space_id, &id, &b0, 0, 1 << 53)
        .is_err());
    // The field is mandatory and alone.
    env.post(&devs[1], "/v1/attachment-tombstones", json!({}))
        .await
        .err(422, "validation_failed");
    env.post(
        &devs[1],
        "/v1/attachment-tombstones",
        json!({ "attachment_tombstone": "", "extra": 1 }),
    )
    .await
    .err(422, "validation_failed");

    assert!(blob_file_exists(&env, &b0));
    assert_eq!(
        env.sql_count("SELECT count(*) FROM attachment_tombstones"),
        0
    );
    assert!(env
        .events_of_type(&devs[0], "attachment_tombstone")
        .await
        .is_empty());
}

#[tokio::test]
async fn a198_a_blob_that_is_not_the_one_of_that_position_is_refused() {
    let env = Env::new().await;
    let (devs, _) = env.space_of(2).await;
    let (id, b0, b1) = item_with_two_blobs(&env, &devs[0]).await;
    let (other, o0, _o1) = item_with_two_blobs(&env, &devs[0]).await;
    // The right blob, the wrong position.
    env.post_attachment_tombstone(&devs[1], &id, &b0, 1)
        .await
        .err(422, "validation_failed");
    // The blob of another item.
    env.post_attachment_tombstone(&devs[1], &id, &o0, 0)
        .await
        .err(422, "validation_failed");
    // Another blob at that position of that item.
    env.post_attachment_tombstone(&devs[1], &other, &b1, 1)
        .await
        .err(422, "validation_failed");
    let unknown = new_uuid();
    env.post_attachment_tombstone(&devs[1], &id, &unknown, 0)
        .await
        .err(422, "validation_failed");
    for b in [&b0, &b1, &o0] {
        assert!(blob_file_exists(&env, b));
    }
    assert_eq!(
        env.sql_count("SELECT count(*) FROM attachment_tombstones"),
        0
    );
    let _ = other;
}

#[tokio::test]
async fn a199_an_unknown_purged_or_tombstoned_item_is_accepted() {
    let env = Env::new().await;
    let (devs, _) = env.space_of(2).await;
    // Unknown item, unknown blob: nothing to delete, the event goes out all the same.
    let (ghost, ghost_blob) = (new_uuid(), new_uuid());
    env.post_attachment_tombstone(&devs[0], &ghost, &ghost_blob, 3)
        .await
        .ok(201);
    // Item deleted everywhere, then a removal of one of its attachments: no special case.
    let (id, b0, _b1) = item_with_two_blobs(&env, &devs[0]).await;
    env.post_tombstone(&devs[0], &id).await.ok(201);
    assert!(!blob_file_exists(&env, &b0));
    env.post_attachment_tombstone(&devs[1], &id, &b0, 0)
        .await
        .ok(201);
    assert_eq!(
        env.events_of_type(&devs[1], "attachment_tombstone")
            .await
            .len(),
        2
    );
}

#[tokio::test]
async fn a200_the_purge_of_an_item_keeps_its_removals_and_the_compaction_forgets_them() {
    let env = Env::new().await;
    let (mut devs, _) = env.space_of(2).await;
    let (id, b0, b1) = item_with_two_blobs(&env, &devs[0]).await;
    let created = env
        .post_attachment_tombstone(&devs[0], &id, &b1, 1)
        .await
        .ok(201);
    // The item leaves the relay (deleted everywhere): the removal stays, a replay is not
    // a second event.
    env.post_tombstone(&devs[1], &id).await.ok(201);
    let head = env.head_seq().await;
    let replay = env
        .post_attachment_tombstone(&devs[1], &id, &b1, 1)
        .await
        .ok(200);
    assert_eq!(replay["seq"], created["seq"]);
    assert_eq!(env.head_seq().await, head);
    assert!(!blob_file_exists(&env, &b0));
    assert_eq!(
        env.sql_count("SELECT count(*) FROM attachment_tombstones"),
        1
    );
    // 45 days later the event is compacted, and its row with it.
    env.clock.advance_secs(46 * 86_400);
    for d in &mut devs {
        env.login(d, 1, 1).await;
    }
    maintenance::compact(&env.state).await;
    assert_eq!(
        env.sql_count("SELECT count(*) FROM attachment_tombstones"),
        0
    );
}

#[tokio::test]
async fn a201_the_capability_is_announced_and_a_v1_database_gains_the_table() {
    let env = Env::new().await;
    let (devs, _) = env.space_of(1).await;
    let hello = env.send("GET", "/v1/hello", &[], None).await.ok(200);
    assert_eq!(hello["features"], json!(["attachment_tombstones"]));
    let space = env.get(&devs[0], "/v1/space").await.ok(200);
    assert_eq!(space["features"], json!(["attachment_tombstones"]));

    // A database of the previous relay (schema v1, no such table) is opened as it is and
    // gains the table; a second open changes nothing.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("relay.db");
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(tomi_relay::db::SCHEMA).unwrap();
        conn.pragma_update(None, "user_version", 1).unwrap();
    }
    for _ in 0..2 {
        let db = tomi_relay::db::Db::open(&path).unwrap();
        let n = db
            .read_sync(|tx| {
                Ok(tx.query_row(
                    "SELECT count(*) FROM sqlite_master WHERE name = 'attachment_tombstones'",
                    [],
                    |r| r.get::<_, i64>(0),
                )?)
            })
            .unwrap();
        assert_eq!(n, 1);
        db.close();
    }
}

#[tokio::test]
async fn a202_only_a_member_removes_an_attachment() {
    let env = Env::new().await;
    let (devs, _) = env.space_of(2).await;
    let (id, b0, _b1) = item_with_two_blobs(&env, &devs[0]).await;
    let body = json!({ "attachment_tombstone": b64(&env.attachment_tombstone(&devs[1].keys, &id, &b0, 0)) });
    env.send("POST", "/v1/attachment-tombstones", &[], Some(body))
        .await
        .err(401, "unauthorized");
    assert!(blob_file_exists(&env, &b0));
}
