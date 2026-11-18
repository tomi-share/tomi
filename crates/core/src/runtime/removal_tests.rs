//! Removing one attachment (format-v1 §8.10): engine tests against the in-memory relay.
//! A child of `tests`, so that it shares its `World` and helpers.

use super::*;
use crate::runtime::store::{ItemStatus, OpKind};
use crate::{AttachmentTombstone, CoreError};

fn draft_files(w: &World, from: usize, names: &[&str], with_text: bool) -> OutgoingDraft {
    let mut d = text("avec fichiers");
    if !with_text {
        d.text = None;
        d.url = None;
    }
    for (i, name) in names.iter().enumerate() {
        let src = w.nodes[from].dir.path().join(format!("src{i}.bin"));
        std::fs::write(&src, marked_file(20_000 + i)).unwrap();
        d.attachments.push(DraftAttachment {
            path: src.to_string_lossy().into_owned(),
            name: (*name).into(),
            mime: "application/octet-stream".into(),
            thumbnail: Some(crate::Thumbnail {
                mime: "image/png".into(),
                data: format!("thumb-{name}").into_bytes(),
            }),
        });
    }
    d
}

fn slots_of(e: &Engine, id: &str) -> Vec<u32> {
    match e.get_item(id).unwrap().unwrap().content {
        ItemContentView::Readable { attachments, .. } => {
            attachments.iter().map(|a| a.slot).collect()
        }
        other => panic!("{other:?}"),
    }
}

fn opened(e: &Engine, id: &str) -> Item {
    let a = e.active().unwrap();
    let row = e.store.read(|db| db.item(id)).unwrap().unwrap();
    a.lsk.open_item(row.sealed.as_ref().unwrap(), id).unwrap()
}

fn diag_kinds(e: &Engine) -> Vec<String> {
    e.store
        .read(|db| db.diagnostics())
        .unwrap()
        .into_iter()
        .map(|d| d.1)
        .collect()
}

/// A sends an item with two files; B has it, files not downloaded.
fn two_files() -> (World, String) {
    let w = World::new(2, 2);
    let d = draft_files(&w, 0, &["a.bin", "b.bin"], true);
    let id = send(&w, 0, d);
    w.e(1).sync().unwrap();
    (w, id)
}

#[test]
fn a_local_removal_empties_the_slot_and_sends_nothing() {
    let (w, id) = two_files();
    let blob0 = opened(w.e(1), &id).attachments[0].blob_id.clone();
    let listed = |e: &Engine| -> Vec<String> {
        let row = e.store.read(|db| db.item(&id)).unwrap().unwrap();
        row.blobs.into_iter().map(|b| b.blob_id).collect()
    };
    assert_eq!(listed(w.e(1)).len(), 2);
    assert_eq!(slots_of(w.e(1), &id), vec![0, 1]);
    let r = w.e(1).delete_attachment(&id, 0, false).unwrap();
    assert_eq!(r, AttachmentRemoval::AttachmentRemoved);
    assert_eq!(slots_of(w.e(1), &id), vec![1], "slot 1 keeps its slot");
    // Neither the id nor the encrypted size of the removed blob stays in `items.blobs_json`.
    let after = listed(w.e(1));
    assert_eq!(after.len(), 1);
    assert!(!after.contains(&blob0));
    assert_eq!(w.e(1).attachment_path(&id, 0).unwrap(), None);
    // The other attachment is still fetched, by its slot.
    let p = w.e(1).attachment_path(&id, 1).unwrap().unwrap();
    assert_eq!(std::fs::read(p).unwrap(), marked_file(20_001));
    // As if it had never existed: no name, size, key or thumbnail left in the payload.
    let item = opened(w.e(1), &id);
    let gone = &item.attachments[0];
    assert_eq!(
        (gone.name.as_str(), gone.mime.as_str(), gone.size),
        ("", "", 0)
    );
    assert!(gone.thumbnail.is_none());
    assert_eq!(gone.file_key, vec![0u8; 32]);
    assert_eq!(gone.nonce_prefix, vec![0u8; 19]);
    assert_eq!(item.removed_slots(), vec![0]);
    assert_eq!(item.attachments[1].name, "b.bin");
    // Nothing went to the relay; the other device is untouched.
    w.e(1).sync().unwrap();
    assert_eq!(w.relay.count("post_attachment_tombstone"), 0);
    assert_eq!(slots_of(w.e(0), &id), vec![0, 1]);
    // No file of the removed slot is left.
    assert!(!w
        .e(1)
        .store
        .file_path(&format!("files/{id}_0.enc"))
        .exists());
    // Idempotent, and unknown slots / items do nothing.
    for (item, slot) in [
        (id.as_str(), 0),
        (id.as_str(), 9),
        ("00000000-0000-4000-8000-000000000001", 0),
    ] {
        assert_eq!(
            w.e(1).delete_attachment(item, slot, false).unwrap(),
            AttachmentRemoval::AttachmentRemoved
        );
    }
    assert_eq!(slots_of(w.e(1), &id), vec![1]);
}

#[test]
fn a_removal_everywhere_reaches_the_other_device_and_the_relay_blob_goes() {
    let (w, id) = two_files();
    // The relay announced the capability at the session.
    assert!(w.e(1).supports_attachment_removal());
    let blob = opened(w.e(0), &id).attachments[1].blob_id.clone();
    assert!(w.relay.with(|s| s.has_blob(&blob)));
    w.e(1).delete_attachment(&id, 1, true).unwrap();
    assert_eq!(slots_of(w.e(1), &id), vec![0]);
    w.e(1).sync().unwrap();
    assert_eq!(w.relay.count("post_attachment_tombstone"), 1);
    assert!(!w.relay.with(|s| s.has_blob(&blob)));
    w.e(0).sync().unwrap();
    assert_eq!(slots_of(w.e(0), &id), vec![0]);
    assert_eq!(w.e(0).attachment_path(&id, 1).unwrap(), None);
    assert!(!w
        .e(0)
        .store
        .file_path(&format!("files/{id}_1.enc"))
        .exists());
    let own = opened(w.e(0), &id);
    assert_eq!(own.removed_slots(), vec![1]);
    assert!(own.attachments[1].name.is_empty());
    // Replaying the whole log changes nothing and makes no second event.
    w.e(0).store.tx(|db| db.set_cursor(0)).unwrap();
    w.e(0).sync().unwrap();
    assert_eq!(slots_of(w.e(0), &id), vec![0]);
    w.e(1).delete_attachment(&id, 1, true).unwrap();
    w.e(1).sync().unwrap();
    assert_eq!(w.relay.count("post_attachment_tombstone"), 1);
}

#[test]
fn an_item_still_queued_refuses_the_removal_in_both_scopes_and_keeps_its_upload() {
    let w = World::new(2, 2);
    let d = draft_files(&w, 0, &["a.bin", "b.bin"], true);
    let id = w.e(0).enqueue(d).unwrap();
    for everywhere in [true, false] {
        let err = w.e(0).delete_attachment(&id, 0, everywhere).unwrap_err();
        assert!(matches!(err, CoreError::InvalidInput { .. }), "{err:?}");
    }
    assert_eq!(slots_of(w.e(0), &id), vec![0, 1]);
    assert!(opened(w.e(0), &id).removed_slots().is_empty());
    // The upload finds both files; nothing was queued for the relay.
    assert_eq!(w.e(0).dispatch(&id).unwrap(), DispatchOutcome::Sent);
    assert_eq!(w.relay.count("post_attachment_tombstone"), 0);
    for i in 0..2 {
        assert!(w
            .e(0)
            .store
            .file_path(&format!("files/{id}_{i}.enc"))
            .exists());
    }
    // Once sent, the removal is possible.
    w.e(0).delete_attachment(&id, 0, false).unwrap();
    assert_eq!(slots_of(w.e(0), &id), vec![1]);
}

#[test]
fn a_rotation_before_the_send_keeps_every_attachment_in_the_envelope() {
    let w = World::new(3, 3);
    for i in 0..3 {
        w.e(i).sync().unwrap();
    }
    let d = draft_files(&w, 0, &["a.bin", "b.bin"], true);
    let id = w.e(0).enqueue(d).unwrap();
    // The removal is refused while the item waits, so the re-sealed envelope stays whole.
    assert!(w.e(0).delete_attachment(&id, 0, false).is_err());
    w.e(0).revoke_device(&w.nodes[2].id).unwrap();
    assert_eq!(w.relay.with(|s| s.key_epoch), 2);
    assert_eq!(w.e(0).dispatch(&id).unwrap(), DispatchOutcome::Sent);
    w.e(1).sync().unwrap();
    assert_eq!(slots_of(w.e(1), &id), vec![0, 1]);
    assert!(opened(w.e(1), &id).removed_slots().is_empty());
}
#[test]
fn removing_the_last_content_deletes_the_item_with_the_same_scope() {
    let w = World::new(2, 2);
    let mut d = draft_files(&w, 0, &["a.bin"], false);
    d.title = Some("Un titre".into());
    let id = send(&w, 0, d);
    w.e(1).sync().unwrap();
    // The title does not count: only the attachment is left, so the item goes.
    let r = w.e(1).delete_attachment(&id, 0, true).unwrap();
    assert_eq!(r, AttachmentRemoval::ItemDeleted);
    assert!(w.e(1).get_item(&id).unwrap().is_none());
    w.e(1).sync().unwrap();
    assert_eq!(w.relay.count("post_tombstone"), 1);
    assert_eq!(w.relay.count("post_attachment_tombstone"), 0);
    w.e(0).sync().unwrap();
    assert!(w.e(0).get_item(&id).unwrap().is_none());
    // Local scope: the item goes from this device only.
    let d2 = draft_files(&w, 0, &["z.bin"], false);
    let id2 = send(&w, 0, d2);
    w.e(1).sync().unwrap();
    assert_eq!(
        w.e(1).delete_attachment(&id2, 0, false).unwrap(),
        AttachmentRemoval::ItemDeleted
    );
    assert!(w.e(1).get_item(&id2).unwrap().is_none());
    assert!(w.e(0).get_item(&id2).unwrap().is_some());
    // With a text, the item stays.
    let d3 = draft_files(&w, 0, &["y.bin"], true);
    let id3 = send(&w, 0, d3);
    w.e(1).sync().unwrap();
    assert_eq!(
        w.e(1).delete_attachment(&id3, 0, false).unwrap(),
        AttachmentRemoval::AttachmentRemoved
    );
    assert!(w.e(1).get_item(&id3).unwrap().is_some());
}

#[test]
fn a_removal_on_a_received_item_releases_the_relay_when_nothing_is_left_to_fetch() {
    let (w, id) = two_files();
    w.e(1).sync().unwrap();
    let acks = w.relay.count("ack_item");
    w.e(1).delete_attachment(&id, 0, false).unwrap();
    w.e(1).sync().unwrap();
    assert_eq!(w.relay.count("ack_item"), acks, "slot 1 is still to fetch");
    w.e(1).delete_attachment(&id, 1, false).unwrap();
    w.e(1).sync().unwrap();
    assert_eq!(w.relay.count("ack_item"), acks + 1, "release");
}

#[test]
fn a_removal_recorded_before_the_item_applies_when_the_item_arrives() {
    let w = World::new(2, 2);
    let d = draft_files(&w, 0, &["a.bin", "b.bin"], true);
    let id = send(&w, 0, d);
    let blob = opened(w.e(0), &id).attachments[0].blob_id.clone();
    // B learns the removal before it has the item (here: written straight to the registry).
    w.e(1)
        .store
        .tx(|db| db.put_attachment_removal(&id, 0, &blob, true, T0, T0))
        .unwrap();
    w.e(1).sync().unwrap();
    assert_eq!(slots_of(w.e(1), &id), vec![1]);
    let row = w.e(1).store.read(|db| db.item(&id)).unwrap().unwrap();
    assert!(row.blobs.iter().all(|b| b.blob_id != blob) && row.blobs.len() == 1);
    assert!(opened(w.e(1), &id).attachments[0].name.is_empty());
    assert_eq!(w.e(1).attachment_path(&id, 0).unwrap(), None);
    // A removal recorded for another blob of that slot is not applied.
    let d2 = draft_files(&w, 0, &["c.bin"], true);
    let id2 = send(&w, 0, d2);
    w.e(1)
        .store
        .tx(|db| {
            db.put_attachment_removal(
                &id2,
                0,
                "00000000-0000-4000-8000-0000000000ee",
                true,
                T0,
                T0,
            )
        })
        .unwrap();
    w.e(1).sync().unwrap();
    assert_eq!(slots_of(w.e(1), &id2), vec![0]);
}

#[test]
fn a_removal_during_a_download_leaves_no_file_and_no_row() {
    let (w, id) = two_files();
    let blob = opened(w.e(1), &id).attachments[0].blob_id.clone();
    // The removal lands in the registry while the file is being fetched.
    w.e(1)
        .store
        .tx(|db| db.put_attachment_removal(&id, 0, &blob, true, T0, T0))
        .unwrap();
    assert_eq!(w.e(1).attachment_path(&id, 0).unwrap(), None);
    let files = w.e(1).store.read(|db| db.files(&id)).unwrap();
    assert!(
        files.iter().all(|f| f.idx != 0),
        "no stale Remote row: {files:?}"
    );
    assert!(!w
        .e(1)
        .store
        .file_path(&format!("files/{id}_0.enc"))
        .exists());
    assert!(holding(&w.nodes[1], MARKER).is_empty());
}

#[test]
fn a_malformed_or_foreign_event_is_a_diagnostic_and_never_blocks_the_sync() {
    let (w, id) = two_files();
    let blob = opened(w.e(0), &id).attachments[0].blob_id.clone();
    let push = |statement: Vec<u8>| {
        w.relay.with(|s| {
            s.push(
                EventData::AttachmentTombstone {
                    item_id: id.clone(),
                    slot: 0,
                    blob_id: blob.clone(),
                    device_id: "ffffffffffffffffffffffffffffffff".into(),
                    deleted_at_ms: T0,
                    statement,
                },
                None,
            )
        })
    };
    // Garbage, empty, a stranger's signature, a tombstone of an item, a wrong blob.
    push(vec![1, 2, 3]);
    push(Vec::new());
    let stranger = DeviceKeys::generate().unwrap();
    push(
        stranger
            .sign_attachment_tombstone(&w.space_id, &id, &blob, 0, T0 as u64)
            .unwrap()
            .to_bytes()
            .unwrap(),
    );
    push(
        w.nodes[0]
            .keys()
            .sign_tombstone(
                &w.space_id,
                &id,
                T0 as u64,
                crate::DeletionScope::Everywhere,
            )
            .unwrap()
            .to_bytes()
            .unwrap(),
    );
    let wrong: AttachmentTombstone = w.nodes[0]
        .keys()
        .sign_attachment_tombstone(
            &w.space_id,
            &id,
            "00000000-0000-4000-8000-0000000000ee",
            0,
            T0 as u64,
        )
        .unwrap();
    let head = push(wrong.to_bytes().unwrap());
    let r = w.e(1).sync().unwrap();
    assert!(!r.offline);
    assert_eq!(w.e(1).store.read(|db| db.cursor()).unwrap(), head);
    assert_eq!(slots_of(w.e(1), &id), vec![0, 1], "nothing removed");
    let refused = diag_kinds(w.e(1))
        .iter()
        .filter(|k| *k == "attachment_tombstone_refused")
        .count();
    assert_eq!(refused, 5);
    // A removal for an item that was deleted is ignored.
    w.e(1).delete_local(&id).unwrap();
    let good = w.nodes[0]
        .keys()
        .sign_attachment_tombstone(&w.space_id, &id, &blob, 0, T0 as u64)
        .unwrap();
    w.relay.with(|s| {
        s.push(
            EventData::AttachmentTombstone {
                item_id: id.clone(),
                slot: 0,
                blob_id: blob.clone(),
                device_id: w.nodes[0].id.clone(),
                deleted_at_ms: T0,
                statement: good.to_bytes().unwrap(),
            },
            None,
        )
    });
    w.e(1).sync().unwrap();
    assert!(w.e(1).get_item(&id).unwrap().is_none());
    assert_eq!(
        w.e(1)
            .store
            .read(|db| db.attachment_removals(&id))
            .unwrap()
            .len(),
        0
    );
}

#[test]
fn a_relay_without_the_route_keeps_the_operation_and_it_goes_later() {
    let (w, id) = two_files();
    w.relay.with(|s| s.features.clear());
    w.e(1).delete_attachment(&id, 0, true).unwrap();
    w.e(1).sync().unwrap();
    assert_eq!(w.relay.count("post_attachment_tombstone"), 1);
    let ops = w.e(1).store.read(|db| db.pending_ops()).unwrap();
    let op = ops
        .iter()
        .find(|o| o.kind == OpKind::AttachmentTombstone)
        .expect("still queued");
    assert_eq!(op.item_id, None);
    assert!(op.next_attempt_at_ms > T0 + 3_600_000, "long delay");
    // The relay is upgraded; a long time later the operation goes through.
    w.relay
        .with(|s| s.features = vec!["attachment_tombstones".into()]);
    w.clock.advance(7 * 3_600_000);
    w.e(1).sync().unwrap();
    assert_eq!(w.relay.count("post_attachment_tombstone"), 2);
    assert!(w
        .e(1)
        .store
        .read(|db| db.pending_ops())
        .unwrap()
        .iter()
        .all(|o| o.kind != OpKind::AttachmentTombstone));
    w.e(0).sync().unwrap();
    assert_eq!(slots_of(w.e(0), &id), vec![1]);
}

#[test]
fn the_pending_removal_survives_the_deletion_of_its_item() {
    let (w, id) = two_files();
    w.e(1).delete_attachment(&id, 0, true).unwrap();
    w.e(1).delete_local(&id).unwrap();
    w.e(1).sync().unwrap();
    assert_eq!(w.relay.count("post_attachment_tombstone"), 1);
    w.e(0).sync().unwrap();
    assert_eq!(slots_of(w.e(0), &id), vec![1]);
}

#[test]
fn the_history_transfer_carries_removed_slots_as_removed() {
    let w = World::new(2, 1);
    w.e(0).sync().unwrap();
    let d = draft_files(&w, 0, &["a.bin", "b.bin"], true);
    let id = send(&w, 0, d);
    w.e(0).delete_attachment(&id, 0, false).unwrap();
    w.join(1);
    w.e(0).start_export(&w.nodes[1].id).unwrap();
    w.e(1).sync().unwrap();
    let bt = w.e(1).transfers().unwrap();
    assert_eq!(bt[0].state, "done", "{bt:?}");
    assert_eq!(slots_of(w.e(1), &id), vec![1]);
    assert!(opened(w.e(1), &id).attachments[0].name.is_empty());
    assert_eq!(opened(w.e(1), &id).removed_slots(), vec![0]);
    assert_eq!(w.e(1).attachment_path(&id, 0).unwrap(), None);
    let p = w.e(1).attachment_path(&id, 1).unwrap().unwrap();
    assert_eq!(std::fs::read(p).unwrap(), marked_file(20_001));
    // One file row only: the removed slot was never fetched nor listed as unavailable.
    let files = w.e(1).store.read(|db| db.files(&id)).unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].idx, 1);
}

#[test]
fn the_registry_goes_with_the_item_and_is_wiped_with_everything() {
    let (w, id) = two_files();
    w.e(1).delete_attachment(&id, 0, false).unwrap();
    assert_eq!(super::super::store::SCHEMA_VERSION, 4);
    let counts = w.e(1).store.row_counts();
    let reg = counts
        .iter()
        .find(|(n, _)| n == "attachment_removals")
        .unwrap();
    assert_eq!(reg.1, 1);
    w.e(1).delete_local(&id).unwrap();
    assert_eq!(
        w.e(1)
            .store
            .read(|db| db.attachment_removals(&id))
            .unwrap()
            .len(),
        0
    );
}

#[test]
fn orphan_removals_are_forgotten_after_45_days_those_of_an_item_stay() {
    let (w, id) = two_files();
    let blob = opened(w.e(1), &id).attachments[0].blob_id.clone();
    w.e(1).delete_attachment(&id, 0, false).unwrap();
    let ghost = "00000000-0000-4000-8000-0000000000dd";
    w.e(1)
        .store
        .tx(|db| db.put_attachment_removal(ghost, 3, &blob, true, T0, T0))
        .unwrap();
    w.clock.advance(46 * 86_400_000);
    w.e(1).maintenance().unwrap();
    assert_eq!(
        w.e(1)
            .store
            .read(|db| db.attachment_removals(ghost))
            .unwrap()
            .len(),
        0
    );
    assert_eq!(
        w.e(1)
            .store
            .read(|db| db.attachment_removals(&id))
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn a_removal_recorded_before_an_import_applies_to_the_imported_item() {
    let w = World::new(2, 1);
    w.e(0).sync().unwrap();
    let d = draft_files(&w, 0, &["a.bin", "b.bin"], true);
    let id = send(&w, 0, d);
    // Nothing is removed on the sender: it exports both files.
    let blob = opened(w.e(0), &id).attachments[0].blob_id.clone();
    w.join(1);
    // The removal reached the new device before the import did.
    w.e(1)
        .store
        .tx(|db| db.put_attachment_removal(&id, 0, &blob, true, T0, T0))
        .unwrap();
    w.e(0).start_export(&w.nodes[1].id).unwrap();
    w.e(1).sync().unwrap();
    let bt = w.e(1).transfers().unwrap();
    assert_eq!(bt[0].state, "done", "{bt:?}");
    assert_eq!(slots_of(w.e(1), &id), vec![1]);
    let item = opened(w.e(1), &id);
    assert!(item.attachments[0].name.is_empty());
    assert_eq!(item.removed_slots(), vec![0]);
    assert_eq!(w.e(1).attachment_path(&id, 0).unwrap(), None);
    let files = w.e(1).store.read(|db| db.files(&id)).unwrap();
    assert_eq!(files.len(), 1, "no row, no download for the removed slot");
    assert_eq!(files[0].idx, 1);
    let p = w.e(1).attachment_path(&id, 1).unwrap().unwrap();
    assert_eq!(std::fs::read(p).unwrap(), marked_file(20_001));
}

/// B holds an item of epoch 2 with two attachments, sealed with a key it does not have yet:
/// `KeyPending`. The key (`k2`) is handed to B by [`open_key_pending`].
fn key_pending_item() -> (World, String, Vec<String>, crate::SpaceKey) {
    let w = World::new(2, 2);
    for i in 0..2 {
        w.e(i).sync().unwrap();
    }
    let k2 = crate::SpaceKey::generate(2).unwrap();
    let id = crate::new_item_id().unwrap();
    let blobs: Vec<String> = (0..2).map(|_| crate::new_item_id().unwrap()).collect();
    let item = Item {
        id: id.clone(),
        source_device: w.nodes[0].id.clone(),
        created_at_ms: T0,
        expires_at_ms: None,
        read_policy: ReadPolicy::UntilExpiry,
        secret: false,
        title: None,
        text: Some(crate::ItemText {
            plain: "texte".into(),
            html: None,
            markdown: None,
            source_format: SourceFormat::Plain,
        }),
        url: None,
        attachments: blobs
            .iter()
            .enumerate()
            .map(|(i, b)| crate::Attachment {
                blob_id: b.clone(),
                name: format!("f{i}.bin"),
                mime: "application/octet-stream".into(),
                size: 10,
                file_key: vec![7; 32],
                nonce_prefix: vec![8; 19],
                thumbnail: None,
            })
            .collect(),
        raw_payload: None,
    };
    let envelope = crate::seal_item(&item, &k2).unwrap();
    w.relay.with(|s| {
        s.push(
            EventData::Item {
                item_id: id.clone(),
                sender_device_id: w.nodes[0].id.clone(),
                created_at_ms: T0,
                expires_at_ms: T0 + 100_000,
                key_epoch: 2,
                envelope,
                blobs: blobs
                    .iter()
                    .map(|b| crate::runtime::transport::BlobRef {
                        blob_id: b.clone(),
                        size: 42,
                    })
                    .collect(),
                ring: false,
            },
            None,
        );
    });
    w.e(1).sync().unwrap();
    let row = w.e(1).store.read(|db| db.item(&id)).unwrap().unwrap();
    assert_eq!(
        row.status,
        ItemStatus::Unreadable(UnreadableReason::KeyPending)
    );
    (w, id, blobs, k2)
}

/// The rotation reaches B: its ring gets `k2` and the `KeyPending` items are retried.
fn open_key_pending(w: &World, k2: crate::SpaceKey) {
    let now = w.clock_now();
    w.e(1)
        .with_crypto(|c, db, a| {
            c.ring
                .add_rotation_key(k2, 0, u64::try_from(now).unwrap_or(0))?;
            w.e(1).retry_key_pending(c, db, a)
        })
        .unwrap();
    w.e(1).sync().unwrap();
}

#[test]
fn a_key_pending_item_whose_attachments_were_all_removed_is_released_when_it_opens() {
    let (w, id, blobs, k2) = key_pending_item();
    assert_eq!(w.relay.count("ack_item"), 1, "ack of the arrival");
    for (slot, blob) in blobs.iter().enumerate() {
        w.e(1)
            .store
            .tx(|db| db.put_attachment_removal(&id, slot as u32, blob, true, T0, T0))
            .unwrap();
    }
    open_key_pending(&w, k2);
    // Both slots are emptied, nothing is left to fetch: the release goes.
    assert_eq!(slots_of(w.e(1), &id), Vec::<u32>::new());
    let row = w.e(1).store.read(|db| db.item(&id)).unwrap().unwrap();
    assert!(row.blobs.is_empty(), "the relay's blob list is forgotten");
    assert!(w.e(1).store.read(|db| db.files(&id)).unwrap().is_empty());
    assert_eq!(w.relay.count("ack_item"), 2, "release");
}

#[test]
fn a_key_pending_item_with_one_attachment_left_is_not_released() {
    let (w, id, blobs, k2) = key_pending_item();
    w.e(1)
        .store
        .tx(|db| db.put_attachment_removal(&id, 0, &blobs[0], true, T0, T0))
        .unwrap();
    open_key_pending(&w, k2);
    assert_eq!(slots_of(w.e(1), &id), vec![1]);
    let row = w.e(1).store.read(|db| db.item(&id)).unwrap().unwrap();
    assert_eq!(row.blobs.len(), 1);
    assert_eq!(row.blobs[0].blob_id, blobs[1]);
    assert_eq!(w.relay.count("ack_item"), 1, "slot 1 is still to fetch");
}

#[test]
fn a_removal_signed_by_a_revoked_device_is_a_diagnostic_and_applies_nothing() {
    let w = World::new(3, 3);
    for i in 0..3 {
        w.e(i).sync().unwrap();
    }
    let d = draft_files(&w, 0, &["a.bin", "b.bin"], true);
    let id = send(&w, 0, d);
    let blob = opened(w.e(0), &id).attachments[0].blob_id.clone();
    w.e(1).sync().unwrap();
    w.e(0).revoke_device(&w.nodes[2].id).unwrap();
    // C, revoked, signs a valid removal of slot 0 and the relay (here, the double) relays it.
    let from_c = w.nodes[2]
        .keys()
        .sign_attachment_tombstone(&w.space_id, &id, &blob, 0, T0 as u64)
        .unwrap();
    w.relay.with(|s| {
        s.push(
            EventData::AttachmentTombstone {
                item_id: id.clone(),
                slot: 0,
                blob_id: blob.clone(),
                device_id: w.nodes[2].id.clone(),
                deleted_at_ms: T0,
                statement: from_c.to_bytes().unwrap(),
            },
            None,
        )
    });
    w.e(1).sync().unwrap();
    assert_eq!(slots_of(w.e(1), &id), vec![0, 1], "nothing removed");
    assert!(opened(w.e(1), &id).removed_slots().is_empty());
    let diags = w.e(1).store.read(|db| db.diagnostics()).unwrap();
    assert!(
        diags
            .iter()
            .any(|(_, k, d)| k == "attachment_tombstone_refused" && d == "revoked_device"),
        "{diags:?}"
    );
    // Not even recorded in the registry.
    assert!(w
        .e(1)
        .store
        .read(|db| db.attachment_removal(&id, 0))
        .unwrap()
        .is_none());
}
