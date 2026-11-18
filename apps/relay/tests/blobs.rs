//! `api-v1.md` §11.8: resumable blobs.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::result_large_err)]

mod common;

use axum::body::Body;
use axum::http::Request;
use bytes::Bytes;
use common::*;
use serde_json::json;
use sha2::{Digest, Sha256};
use tomi_relay::maintenance;

fn pattern(len: usize) -> Vec<u8> {
    (0..len).map(|i| ((i * 31 + i / 251) % 256) as u8).collect()
}

fn blob_path(env: &Env, id: &str) -> std::path::PathBuf {
    tomi_relay::blobstore::path(&env.space().dir, &uuid_bytes(id))
}

#[tokio::test]
async fn a80_an_upload_resumes_after_a_broken_part() {
    let env = Env::new().await;
    let (devs, _) = env.space_of(1).await;
    let d = &devs[0];
    let data = pattern(3 * 1024 * 1024);
    let id = new_uuid();
    env.create_blob(d, &id, data.len() as u64).await.ok(201);
    let mib = 1024 * 1024;
    // Part 1 is whole.
    env.patch_blob(d, &id, 0, data[..mib].to_vec())
        .await
        .ok(204);
    // Part 2: the connection breaks in the middle of the body.
    let part2 = Bytes::copy_from_slice(&data[mib..2 * mib]);
    let stream = futures_util::stream::iter(vec![
        Ok::<Bytes, std::io::Error>(part2.slice(..300_000)),
        Err(std::io::Error::new(
            std::io::ErrorKind::ConnectionReset,
            "cut",
        )),
    ]);
    let req = Request::builder()
        .method("PATCH")
        .uri(format!("/v1/blobs/{id}"))
        .header("authorization", format!("Bearer {}", d.tok()))
        .header("content-type", "application/octet-stream")
        .header("content-length", mib.to_string())
        .header("upload-offset", mib.to_string())
        .body(Body::from_stream(stream))
        .unwrap();
    let broken = env.call(req).await;
    assert!(broken.status >= 400, "the part must fail");
    // The status says where to resume, and the file went back to that offset.
    let st = env.get(d, &format!("/v1/blobs/{id}/status")).await.ok(200);
    assert_eq!(st["upload_offset"], mib);
    assert_eq!(st["complete"], false);
    assert_eq!(
        std::fs::metadata(blob_path(&env, &id)).unwrap().len(),
        mib as u64
    );
    // Resume from the offset.
    env.patch_blob(d, &id, mib as u64, data[mib..2 * mib].to_vec())
        .await
        .ok(204);
    let last = env
        .patch_blob(d, &id, 2 * mib as u64, data[2 * mib..].to_vec())
        .await;
    assert_eq!(last.status, 204);
    assert_eq!(
        last.header("upload-offset").as_deref(),
        Some((3 * mib).to_string().as_str())
    );
    let st = env.get(d, &format!("/v1/blobs/{id}/status")).await.ok(200);
    assert_eq!(st["complete"], true);
    // Attach it so another device may read it, and compare the bytes.
    let (_, r) = {
        let item = new_uuid();
        let env_bytes = Env::envelope(&uuid_bytes(&item), 1, 300);
        let r = env
            .post(
                d,
                "/v1/items",
                env.item_body(&item, 1, DAY_MS, &env_bytes, &[&id]),
            )
            .await;
        (item, r)
    };
    r.ok(201);
    let got = env.download(d, &id, None).await;
    assert_eq!(got.status, 200);
    assert_eq!(got.body.as_ref(), data.as_slice());
    assert_eq!(
        got.header("content-length").as_deref(),
        Some((3 * mib).to_string().as_str())
    );
}

#[tokio::test]
async fn a81_the_offset_must_match() {
    let env = Env::new().await;
    let (devs, _) = env.space_of(1).await;
    let d = &devs[0];
    let id = new_uuid();
    env.create_blob(d, &id, 3000).await.ok(201);
    env.patch_blob(d, &id, 0, vec![1u8; 1000]).await.ok(204);
    let r = env.patch_blob(d, &id, 500, vec![1u8; 100]).await;
    r.err(409, "upload_offset_mismatch");
    assert_eq!(r.json()["upload_offset"], 1000);
    let r = env.patch_blob(d, &id, 2000, vec![1u8; 100]).await;
    r.err(409, "upload_offset_mismatch");
    // The file was not touched by the refused parts.
    assert_eq!(std::fs::metadata(blob_path(&env, &id)).unwrap().len(), 1000);
}

#[tokio::test]
async fn a82_ranges() {
    let env = Env::new().await;
    let (devs, _) = env.space_of(1).await;
    let d = &devs[0];
    let data = pattern(5000);
    let id = env.upload_blob(d, &data, 5000).await;
    let (_, r) = {
        let item = new_uuid();
        let e = Env::envelope(&uuid_bytes(&item), 1, 300);
        (
            item.clone(),
            env.post(d, "/v1/items", env.item_body(&item, 1, DAY_MS, &e, &[&id]))
                .await,
        )
    };
    r.ok(201);

    let part = env.download(d, &id, Some("bytes=100-199")).await;
    assert_eq!(part.status, 206);
    assert_eq!(part.body.as_ref(), &data[100..200]);
    assert_eq!(
        part.header("content-range").as_deref(),
        Some("bytes 100-199/5000")
    );
    assert_eq!(part.header("accept-ranges").as_deref(), Some("bytes"));
    assert_eq!(
        part.header("etag").as_deref(),
        Some(format!("\"{id}\"").as_str())
    );
    assert_eq!(part.header("cache-control").as_deref(), Some("no-store"));
    assert_eq!(
        part.header("content-type").as_deref(),
        Some("application/octet-stream")
    );

    let tail = env.download(d, &id, Some("bytes=4900-")).await;
    assert_eq!((tail.status, tail.body.as_ref()), (206, &data[4900..]));
    let suffix = env.download(d, &id, Some("bytes=-50")).await;
    assert_eq!((suffix.status, suffix.body.as_ref()), (206, &data[4950..]));

    let multi = env.download(d, &id, Some("bytes=0-1,5-6")).await;
    multi.err(416, "range_not_satisfiable");
    assert_eq!(
        multi.header("content-range").as_deref(),
        Some("bytes */5000")
    );
    env.download(d, &id, Some("bytes=5000-"))
        .await
        .err(416, "range_not_satisfiable");

    // `If-Range` with the ETag honours the range, with anything else it sends it all.
    let auth = format!("Bearer {}", d.tok());
    let with = |if_range: String| {
        Request::builder()
            .method("GET")
            .uri(format!("/v1/blobs/{id}"))
            .header("authorization", auth.clone())
            .header("range", "bytes=0-9")
            .header("if-range", if_range)
            .body(Body::empty())
            .unwrap()
    };
    assert_eq!(env.call(with(format!("\"{id}\""))).await.status, 206);
    let full = env.call(with("\"other\"".into())).await;
    assert_eq!((full.status, full.body.len()), (200, 5000));

    // HEAD: the headers of the download and no body.
    let head = env
        .call(
            Request::builder()
                .method("HEAD")
                .uri(format!("/v1/blobs/{id}"))
                .header("authorization", auth)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!((head.status, head.body.len()), (200, 0));
    assert_eq!(head.header("content-length").as_deref(), Some("5000"));
}

#[tokio::test]
async fn a83_an_abandoned_blob_is_collected_after_24_hours() {
    let env = Env::new().await;
    let (devs, _) = env.space_of(1).await;
    let d = &devs[0];
    let before = env.get(d, "/v1/space").await.ok(200)["quota"]["used_bytes"]
        .as_u64()
        .unwrap();
    let id = new_uuid();
    let created = env.create_blob(d, &id, 4000).await.ok(201);
    assert_eq!(created["expires_at"], ts(T0 + 86_400_000));
    env.patch_blob(d, &id, 0, vec![1u8; 1000]).await.ok(204);
    assert_eq!(
        env.get(d, "/v1/space").await.ok(200)["quota"]["used_bytes"]
            .as_u64()
            .unwrap(),
        before + 4000
    );
    env.clock.advance_secs(86_400 - 1);
    maintenance::run_once(&env.state).await;
    assert!(blob_path(&env, &id).exists());
    env.clock.advance_secs(2);
    let report = maintenance::run_once(&env.state).await;
    assert_eq!(report.blobs_collected, 1);
    assert!(!blob_path(&env, &id).exists());
    assert_eq!(env.sql_count("SELECT count(*) FROM blobs"), 0);
    let mut d2 = Dev::new("x");
    d2.token = d.token.clone();
    // The session of one hour has expired by now; log in again to read the quota.
    let mut again = Dev {
        keys: DeviceKeys::from_secrets(&d.keys.ed25519_seed(), &d.keys.x25519_secret()).unwrap(),
        id: d.id.clone(),
        name: d.name.clone(),
        token: None,
    };
    env.login(&mut again, 1, 1).await;
    assert_eq!(
        env.get(&again, "/v1/space").await.ok(200)["quota"]["used_bytes"]
            .as_u64()
            .unwrap(),
        before
    );
    let _ = d2;
}

#[tokio::test]
async fn a84_sizes_and_quota() {
    let env = Env::with(|cfg, _| cfg.max_space_bytes = Some(10_000)).await;
    let (devs, _) = env.space_of(1).await;
    let d = &devs[0];
    // Over the size limit of a blob.
    let r = env.create_blob(d, &new_uuid(), 26_279_937).await;
    r.err(413, "payload_too_large");
    assert_eq!(r.json()["limit"], 26_279_936);
    env.create_blob(d, &new_uuid(), 0)
        .await
        .err(422, "validation_failed");
    // The quota: 10 000 bytes in all.
    env.create_blob(d, &new_uuid(), 6000).await.ok(201);
    let r = env.create_blob(d, &new_uuid(), 5000).await;
    r.err(507, "relay_full");
    let body = r.json();
    assert_eq!(body["max_bytes"], 10_000);
    assert_eq!(body["used_bytes"], 6000);
    env.create_blob(d, &new_uuid(), 4000).await.ok(201);
    // The disk is checked too.
    let env = Env::with(|cfg, _| cfg.min_free_disk_bytes = 1_000_000).await;
    let (devs, _) = env.space_of(1).await;
    env.disk.set(1_500_000);
    env.create_blob(&devs[0], &new_uuid(), 400_000)
        .await
        .ok(201);
    env.create_blob(&devs[0], &new_uuid(), 600_000)
        .await
        .err(507, "relay_full");
}

#[tokio::test]
async fn a85_twenty_five_mebibytes_in_twenty_five_parts() {
    let env = Env::new().await;
    let (devs, _) = env.space_of(1).await;
    let d = &devs[0];
    let data = pattern(25 * 1024 * 1024);
    let id = env.upload_blob(d, &data, 1024 * 1024).await;
    let item = new_uuid();
    let e = Env::envelope(&uuid_bytes(&item), 1, 300);
    env.post(d, "/v1/items", env.item_body(&item, 1, DAY_MS, &e, &[&id]))
        .await
        .ok(201);
    let got = env.download(d, &id, None).await;
    assert_eq!(got.status, 200);
    assert_eq!(Sha256::digest(got.body.as_ref()), Sha256::digest(&data));
    // The handler copies the body to the file chunk by chunk (see `routes/blobs.rs::patch`:
    // `Body::into_data_stream` -> `File::write_all`, never a buffer of the size of a part).
}

#[tokio::test]
async fn the_headers_of_a_part_are_checked_in_order() {
    let env = Env::new().await;
    let (devs, _) = env.space_of(2).await;
    let (d, other) = (&devs[0], &devs[1]);
    let id = new_uuid();
    env.create_blob(d, &id, 3 * 1024 * 1024).await.ok(201);
    let base = |ct: Option<&str>, len: Option<usize>, off: Option<&str>, body: Vec<u8>| {
        let mut b = Request::builder()
            .method("PATCH")
            .uri(format!("/v1/blobs/{id}"))
            .header("authorization", format!("Bearer {}", d.tok()));
        if let Some(c) = ct {
            b = b.header("content-type", c);
        }
        if let Some(l) = len {
            b = b.header("content-length", l.to_string());
        }
        if let Some(o) = off {
            b = b.header("upload-offset", o);
        }
        b.body(Body::from(body)).unwrap()
    };
    // 415, 411 (also for chunked), 413, 422.
    env.call(base(
        Some("application/json"),
        Some(10),
        Some("0"),
        vec![0; 10],
    ))
    .await
    .err(415, "unsupported_media_type");
    let no_len = Request::builder()
        .method("PATCH")
        .uri(format!("/v1/blobs/{id}"))
        .header("authorization", format!("Bearer {}", d.tok()))
        .header("content-type", "application/octet-stream")
        .header("upload-offset", "0")
        .body(Body::from_stream(futures_util::stream::iter(vec![Ok::<
            _,
            std::io::Error,
        >(
            Bytes::from_static(b"abc"),
        )])))
        .unwrap();
    env.call(no_len).await.err(411, "length_required");
    let chunked = Request::builder()
        .method("PATCH")
        .uri(format!("/v1/blobs/{id}"))
        .header("authorization", format!("Bearer {}", d.tok()))
        .header("content-type", "application/octet-stream")
        .header("transfer-encoding", "chunked")
        .header("upload-offset", "0")
        .body(Body::from("abc"))
        .unwrap();
    env.call(chunked).await.err(411, "length_required");
    env.call(base(
        Some("application/octet-stream"),
        Some(1_048_577),
        Some("0"),
        vec![0; 1_048_577],
    ))
    .await
    .err(413, "payload_too_large");
    env.call(base(
        Some("application/octet-stream"),
        Some(0),
        Some("0"),
        vec![],
    ))
    .await
    .err(422, "validation_failed");
    env.call(base(
        Some("application/octet-stream"),
        Some(5),
        None,
        vec![0; 5],
    ))
    .await
    .err(422, "validation_failed");
    // Past the declared size.
    env.patch_blob(d, &id, 0, vec![1u8; 1_048_576])
        .await
        .ok(204);
    env.patch_blob(d, &id, 1_048_576, vec![1u8; 1_048_576])
        .await
        .ok(204);
    env.patch_blob(d, &id, 2_097_152, vec![1u8; 1_048_577 - 1])
        .await
        .ok(204);
    let small = new_uuid();
    env.create_blob(d, &small, 10).await.ok(201);
    env.patch_blob(d, &small, 0, vec![1u8; 11])
        .await
        .err(413, "payload_too_large");
    // Only the owner writes; a complete blob takes no more parts.
    env.patch_blob(other, &id, 0, vec![1u8; 5])
        .await
        .err(404, "not_found");
    env.patch_blob(d, &id, 3 * 1024 * 1024, vec![1u8; 5])
        .await
        .err(409, "blob_attached");
    // Creation is idempotent, and a conflict is a conflict.
    env.post(d, "/v1/blobs", json!({ "blob_id": small, "size": 10 }))
        .await
        .ok(200);
    env.post(d, "/v1/blobs", json!({ "blob_id": small, "size": 11 }))
        .await
        .err(409, "blob_id_conflict");
    env.post(other, "/v1/blobs", json!({ "blob_id": small, "size": 10 }))
        .await
        .err(409, "blob_id_conflict");
    // The status is for the owner only; so is the deletion of an unattached blob.
    env.get(other, &format!("/v1/blobs/{small}/status"))
        .await
        .err(404, "not_found");
    env.delete(other, &format!("/v1/blobs/{small}"))
        .await
        .err(404, "not_found");
    env.delete(d, &format!("/v1/blobs/{small}")).await.ok(204);
    assert!(!blob_path(&env, &small).exists());
}
