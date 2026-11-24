//! The trash of the relay: a space deleted from the web panel is gone at once for its devices, its
//! folder waits 7 days in `spaces.trash/`, and only the CLI brings it back or purges it.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::result_large_err)]

mod common;

use common::*;
use serde_json::Value;
use tomi_relay::admin::{self, Cmd};
use tomi_relay::spaces::{DeleteOutcome, TRASH_DIR, TRASH_TTL_MS};
use tomi_relay::trash::TrashAction;

async fn trash_cmd(
    env: &Env,
    action: TrashAction,
    space: Option<&str>,
    confirm: Option<&str>,
) -> admin::Output {
    admin::run(
        &env.state,
        Cmd::Trash {
            action,
            space: space.map(str::to_owned),
            confirm: confirm.map(str::to_owned),
            json: false,
        },
    )
    .await
}

async fn delete_to_trash(env: &Env) -> DeleteOutcome {
    let space = env.space();
    let registry = env.state.spaces.lock_registry().await;
    env.state
        .delete_space_keep(&space, registry, None, true)
        .await
}

fn trash_folders(env: &Env) -> Vec<String> {
    match std::fs::read_dir(env.data_dir().join(TRASH_DIR)) {
        Ok(rd) => rd
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect(),
        Err(_) => Vec::new(),
    }
}

#[tokio::test]
async fn a_space_deleted_with_keep_is_gone_for_the_devices_but_its_folder_waits() {
    let env = Env::new().await;
    let (devs, _) = env.space_of(2).await;
    let id = env.space_id.clone();

    assert_eq!(delete_to_trash(&env).await, DeleteOutcome::Deleted);

    // Gone for everyone, at once: no route, an id that is free again.
    assert!(env.state.spaces.is_empty());
    assert!(env.state.spaces.by_device(&devs[0].id).is_none());
    assert!(!env.data_dir().join("spaces").join(&id).exists());

    // The folder is in the trash, recorded with its deadline and size.
    let folders = trash_folders(&env);
    assert_eq!(folders.len(), 1);
    assert!(folders[0].starts_with(&id));
    let json = trash_cmd(&env, TrashAction::List, None, None).await;
    assert_eq!(json.exit, 0, "{}", json.stderr);
    assert!(json.stdout.contains(&id), "{}", json.stdout);
    let rows = env
        .state
        .spaces
        .routing()
        .read(|tx| tomi_relay::trash::rows(tx))
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].purge_after - rows[0].deleted_at, TRASH_TTL_MS);
    assert!(rows[0].size_bytes > 0);
}

#[tokio::test]
async fn restore_brings_the_space_back_with_its_routing_and_its_quota() {
    let env = Env::new().await;
    let (mut devs, _) = env.space_of(2).await;
    let short = env.space_id[..8].to_owned();
    // A quota of the space's own is given back.
    let out = admin::run(
        &env.state,
        Cmd::Quota {
            space: Some(short.clone()),
            value: "5000000".into(),
        },
    )
    .await;
    assert_eq!(out.exit, 0, "{}", out.stderr);

    assert_eq!(delete_to_trash(&env).await, DeleteOutcome::Deleted);
    assert_eq!(env.state.spaces.list().len(), 0);

    let out = trash_cmd(&env, TrashAction::Restore, None, None).await;
    assert_eq!(out.exit, 0, "{}", out.stderr);
    assert!(
        out.stdout.contains("open Tomi on each device"),
        "{}",
        out.stdout
    );

    // Routing rebuilt: the devices log in again with the same keys, the quota is back.
    assert_eq!(env.state.spaces.list().len(), 1);
    assert!(env.state.spaces.by_device(&devs[0].id).is_some());
    env.login(&mut devs[0], 1, 1).await;
    assert_eq!(
        env.state.spaces.quota_override(&env.space().id),
        Some(Some(5_000_000))
    );
    // Nothing is left in the trash.
    assert!(trash_folders(&env).is_empty());
    let out = trash_cmd(&env, TrashAction::List, None, None).await;
    assert!(out.stdout.contains("empty"), "{}", out.stdout);
}

#[tokio::test]
async fn restore_is_refused_when_the_id_is_used_again_and_nothing_is_moved() {
    let env = Env::new().await;
    let (_devs, _) = env.space_of(2).await;
    let id = env.space_id.clone();
    assert_eq!(delete_to_trash(&env).await, DeleteOutcome::Deleted);

    // The same space id is founded again.
    let token = tomi_relay::install::create(env.state.spaces.routing(), env.state.now()).unwrap();
    let founder = Dev::new("PC-2");
    env.create_space_raw(&founder, &token).await.ok(201);
    assert!(env.state.spaces.get(&uuid_bytes(&id)).is_some());

    let out = trash_cmd(&env, TrashAction::Restore, None, None).await;
    assert_eq!(out.exit, 2, "{}", out.stderr);
    assert_eq!(
        trash_folders(&env).len(),
        1,
        "the folder stays in the trash"
    );
}

#[tokio::test]
async fn purge_needs_the_eight_characters_and_deletes_for_good() {
    let env = Env::new().await;
    let (_devs, _) = env.space_of(2).await;
    let short = env.space_id[..8].to_owned();
    assert_eq!(delete_to_trash(&env).await, DeleteOutcome::Deleted);

    let refused = trash_cmd(&env, TrashAction::Purge, None, None).await;
    assert_eq!(refused.exit, 2);
    let wrong = trash_cmd(&env, TrashAction::Purge, None, Some("00000000")).await;
    assert_eq!(wrong.exit, 2);
    assert_eq!(trash_folders(&env).len(), 1);

    let done = trash_cmd(&env, TrashAction::Purge, Some(&short), Some(&short)).await;
    assert_eq!(done.exit, 0, "{}", done.stderr);
    assert!(trash_folders(&env).is_empty());
    let out = trash_cmd(&env, TrashAction::Restore, None, None).await;
    assert_eq!(out.exit, 1, "nothing to restore any more");
}

#[tokio::test]
async fn the_maintenance_purges_after_seven_days_and_not_before() {
    let env = Env::new().await;
    let (_devs, _) = env.space_of(2).await;
    assert_eq!(delete_to_trash(&env).await, DeleteOutcome::Deleted);
    let now = env.state.now();

    assert_eq!(
        tomi_relay::trash::purge_expired(&env.state, now + TRASH_TTL_MS - 1000).await,
        0
    );
    assert_eq!(trash_folders(&env).len(), 1);
    assert_eq!(
        tomi_relay::trash::purge_expired(&env.state, now + TRASH_TTL_MS + 1000).await,
        1
    );
    assert!(trash_folders(&env).is_empty());
    let v: Value = serde_json::from_str(
        &trash_cmd(&env, TrashAction::List, None, None)
            .await
            .stdout
            .replace("the trash is empty", "null"),
    )
    .unwrap_or(Value::Null);
    assert!(v.is_null());
}

#[tokio::test]
async fn a_plain_deletion_still_removes_the_folder() {
    let env = Env::new().await;
    let (_devs, _) = env.space_of(2).await;
    let space = env.space();
    let registry = env.state.spaces.lock_registry().await;
    assert_eq!(
        env.state.delete_space(&space, registry, None).await,
        DeleteOutcome::Deleted
    );
    assert!(trash_folders(&env).is_empty());
    let rows = env
        .state
        .spaces
        .routing()
        .read(|tx| tomi_relay::trash::rows(tx))
        .await
        .unwrap();
    assert!(rows.is_empty());
}

fn trash_row_count(env: &Env) -> usize {
    env.state
        .spaces
        .routing()
        .read_sync(|tx| tomi_relay::trash::rows(tx))
        .unwrap()
        .len()
}

#[tokio::test]
async fn restore_is_refused_when_a_device_founded_another_space_and_nothing_is_moved() {
    let env = Env::new().await;
    let (devs, _) = env.space_of(2).await;
    assert_eq!(delete_to_trash(&env).await, DeleteOutcome::Deleted);

    // The first device of the deleted space founds a different space with the same signing key.
    let token = tomi_relay::install::create(env.state.spaces.routing(), env.state.now()).unwrap();
    let other = new_uuid();
    let cert = devs[0]
        .keys
        .sign_device_cert(&other, &devs[0].keys.public(), env.clock.now_ms() as u64)
        .unwrap()
        .to_bytes()
        .unwrap();
    env.send(
        "POST",
        "/v1/space",
        &[],
        Some(serde_json::json!({
            "install_token": token,
            "space_id": other,
            "name": "PC",
            "platform": "windows",
            "certificate": b64(&cert),
        })),
    )
    .await
    .ok(201);
    assert_eq!(env.state.spaces.list().len(), 1);

    let out = trash_cmd(&env, TrashAction::Restore, None, None).await;
    assert_eq!(out.exit, 2, "{}", out.stderr);
    assert!(out.stderr.contains("founded or joined"), "{}", out.stderr);
    // The folder stays in the trash with its row, nothing came back under `spaces/`.
    assert_eq!(trash_folders(&env).len(), 1);
    assert_eq!(trash_row_count(&env), 1);
    assert_eq!(env.state.spaces.list().len(), 1);
    assert!(!env.data_dir().join("spaces").join(&env.space_id).exists());
}

#[tokio::test]
async fn a_trash_row_that_cannot_be_written_puts_the_space_back_instead_of_losing_it() {
    let env = Env::new().await;
    let (mut devs, _) = env.space_of(2).await;
    env.state
        .spaces
        .inject_destroy_fault(&[tomi_relay::spaces::DestroyStep::ForgetRouting]);
    // The deletion is not reported as a success and nothing is orphaned in the trash.
    assert_eq!(delete_to_trash(&env).await, DeleteOutcome::NotDeleted);
    assert!(trash_folders(&env).is_empty());
    assert_eq!(trash_row_count(&env), 0);
    assert_eq!(env.state.spaces.list().len(), 1);
    assert!(env.data_dir().join("spaces").join(&env.space_id).exists());
    env.state.spaces.inject_destroy_fault(&[]);
    env.login(&mut devs[0], 1, 1).await;
    // Once the fault is gone the same deletion works.
    assert_eq!(delete_to_trash(&env).await, DeleteOutcome::Deleted);
    assert_eq!(trash_row_count(&env), 1);
}

#[tokio::test]
async fn a_folder_without_a_row_is_listed_again_by_the_maintenance() {
    let env = Env::new().await;
    let (_devs, _) = env.space_of(2).await;
    assert_eq!(delete_to_trash(&env).await, DeleteOutcome::Deleted);
    env.state
        .spaces
        .routing()
        .write_sync(|tx| {
            tx.execute("DELETE FROM space_trash", [])?;
            Ok(tomi_relay::db::Tx::Commit(()))
        })
        .unwrap();
    assert_eq!(trash_row_count(&env), 0);
    let now = env.state.now();
    assert_eq!(tomi_relay::trash::reconcile(&env.state, now).await, 1);
    assert_eq!(trash_row_count(&env), 1);
    let out = trash_cmd(&env, TrashAction::Restore, None, None).await;
    assert_eq!(out.exit, 0, "{}", out.stderr);
    assert_eq!(env.state.spaces.list().len(), 1);
}

#[tokio::test]
async fn a_row_left_after_a_restore_is_not_reported_as_a_purge() {
    let env = Env::new().await;
    let (_devs, _) = env.space_of(2).await;
    let short = env.space_id[..8].to_owned();
    assert_eq!(delete_to_trash(&env).await, DeleteOutcome::Deleted);
    let row = env
        .state
        .spaces
        .routing()
        .read_sync(|tx| tomi_relay::trash::rows(tx))
        .unwrap()
        .remove(0);
    let out = trash_cmd(&env, TrashAction::Restore, None, None).await;
    assert_eq!(out.exit, 0, "{}", out.stderr);
    // The row stays by mistake.
    env.state
        .spaces
        .routing()
        .write_sync(|tx| {
            tx.execute(
                "INSERT INTO space_trash
                 (dir_name, space_id, deleted_at, purge_after, size_bytes, quota_set, quota_bytes)
                 VALUES (?1, ?2, ?3, ?4, 0, 0, NULL)",
                rusqlite::params![row.dir_name, row.space_id, row.deleted_at, row.purge_after],
            )?;
            Ok(tomi_relay::db::Tx::Commit(()))
        })
        .unwrap();
    let out = trash_cmd(&env, TrashAction::Purge, Some(&short), Some(&short)).await;
    assert_eq!(out.exit, 0, "{}", out.stderr);
    assert!(!out.stdout.contains("for good"), "{}", out.stdout);
    assert!(out.stdout.contains("live"), "{}", out.stdout);
    assert_eq!(trash_row_count(&env), 0);
    assert_eq!(env.state.spaces.list().len(), 1, "the live space is intact");
    let purged = env
        .state
        .spaces
        .routing()
        .read_sync(|tx| {
            Ok(tx.query_row(
                "SELECT count(*) FROM admin_web_audit WHERE kind IN ('trash_purged','trash_expired')",
                [],
                |r| r.get::<_, i64>(0),
            )?)
        })
        .unwrap();
    assert_eq!(purged, 0);
}
