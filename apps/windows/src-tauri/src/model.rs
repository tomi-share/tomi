//! Composite item model shared by the UI layer and the `TomiCore` trait.
//!
//! Timestamps are Unix milliseconds. It mirrors what the core hands to the apps
//! (`tomi_core::runtime::ItemView`): a hidden item ("masqué", `secret`) is an ordinary
//! item whose title, text, link and file names are part of the secret (the format has no
//! separate secret fields, `spec/format-v1.md` §5.4.5). The view layer redacts all of it
//! in one place (`view.rs`).

use serde::{Deserialize, Serialize};

pub type ItemId = String;

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextBody {
    pub plain: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub html: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub markdown: Option<String>,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Attachment {
    pub name: String,
    pub mime: String,
    pub size: u64,
    /// Local path of the file (original or staged copy of a draft). Never set on the
    /// items of the core: the decrypted copy is asked for with `attachment_path`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Small preview as a `data:` URL (images only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumbnail: Option<String>,
    /// `false` when the file cannot be fetched any more (gone from the relay, never
    /// transferred). Always `true` for a draft.
    #[serde(default = "yes")]
    pub available: bool,
    /// Position of the file in the payload of its item, STABLE for the life of the item: what
    /// the core is asked for (`attachment_path`). Differs from the position in the list once a
    /// file was removed from the item (by another device: this client has no UI to remove
    /// one). `None` for a draft and for the fake core: the position in the list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slot: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum ReadPolicy {
    #[default]
    Normal,
    AfterFirstRead,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SourceFormat {
    Plain,
    Markdown,
    Html,
}

/// Marker printed in place of any user content or secret value.
pub const REDACTED: &str = "<redacted>";

// Debug never prints content, labels or values (docs/DEV.md invariant 3): a stray
// `{:?}` or `dbg!` must not leak a secret into a log.
impl std::fmt::Debug for TextBody {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TextBody")
            .field("plain", &REDACTED)
            .field("has_html", &self.html.is_some())
            .field("has_markdown", &self.markdown.is_some())
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceRef {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DeliveryState {
    /// Stored on this PC, not accepted by the relay yet (offline: it goes out with the
    /// next synchronisation).
    Queued,
    /// Accepted by the relay, not fetched by that device yet.
    Sent,
    Delivered,
    Read,
    /// The relay gave up before the device fetched the item (see D26): shown as
    /// "non livré (conservé 30 j)". Only about delivery to that device; the item's own
    /// expiration is `Item::expires_at`.
    Expired,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Delivery {
    pub device: DeviceRef,
    pub state: DeliveryState,
}

/// Why an item cannot be read (for the sentence of the card and of the toast). Both cases keep
/// the item (invariant 7); only the advice differs. The reasons of the core are folded onto the two
/// texts of the Android app: `UpdateRequired`, and everything else (key not here yet, key of a
/// rotation that replaced it, decryption failed).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum UnreadableReason {
    /// A newer Tomi reads it: the item shows after the update.
    UpdateRequired,
    /// Unreadable for now: a synchronisation or an update may make it readable.
    Other,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    pub id: ItemId,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub text: Option<TextBody>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub attachments: Vec<Attachment>,
    pub secret: bool,
    pub source_device: DeviceRef,
    pub created_at: i64,
    #[serde(default)]
    pub expires_at: Option<i64>,
    #[serde(default)]
    pub read_policy: ReadPolicy,
    pub read: bool,
    /// Sent by this device (as opposed to received).
    pub outgoing: bool,
    /// Came with a history transfer (not a new arrival: no toast).
    #[serde(default)]
    pub imported: bool,
    #[serde(default)]
    pub delivery: Vec<Delivery>,
    #[serde(default)]
    pub source_format: Option<SourceFormat>,
    /// Decryption failed. The item is shown as unreadable, never deleted (invariant 7).
    #[serde(default)]
    pub unreadable: bool,
    /// Why, when `unreadable` (`None` for a readable item).
    #[serde(default)]
    pub unreadable_reason: Option<UnreadableReason>,
    /// Pinned on all devices (`spec/pin-contract.md`).
    #[serde(default)]
    pub pinned: bool,
    /// Time of the decision to pin (ms since the Unix epoch, at most 2^53 - 1), `None`
    /// when not pinned.
    #[serde(default)]
    pub pinned_at: Option<i64>,
    /// Archived on all devices (`spec/archive-contract.md`). Never `true` for a pinned item:
    /// the pin wins.
    #[serde(default)]
    pub archived: bool,
    /// Time of the decision to archive (ms since the Unix epoch, at most 2^53 - 1), `None`
    /// when not archived.
    #[serde(default)]
    pub archived_at: Option<i64>,
}

/// What `delete_attachment` did. Sent to the page, which words the confirmation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AttachmentRemoval {
    /// The attachment is gone (or already was: the call is idempotent); the item stays.
    AttachmentRemoved,
    /// It was the last thing in the item: the whole item was deleted instead, same scope.
    ItemDeleted,
}

impl Item {
    /// True while every delivery of the item is still queued (it has not left this PC): the core
    /// refuses to remove an attachment then, so the UI offers no removal for those few seconds.
    pub fn is_queued(&self) -> bool {
        !self.delivery.is_empty()
            && self
                .delivery
                .iter()
                .all(|d| d.state == DeliveryState::Queued)
    }

    /// Slot of the attachment at `position` in the list (what the core is asked for), `None`
    /// when there is no such attachment. Without a removal the slot is the position.
    pub fn attachment_slot(&self, position: usize) -> Option<usize> {
        self.attachments
            .get(position)
            .map(|a| a.slot.map_or(position, |s| s as usize))
    }

    /// The attachment whose slot is `slot` (what the UI sends: a removal received from another
    /// device never makes it aim at another file), with its position in the list. `None` when
    /// it is not (or no longer) in the item.
    pub fn attachment_by_slot(&self, slot: usize) -> Option<(usize, &Attachment)> {
        self.attachments
            .iter()
            .enumerate()
            .find(|(position, a)| a.slot.map_or(*position, |s| s as usize) == slot)
    }

    /// Whether the UI treats the item as archived (`FeedFiltering.kt`, `isShownAsArchived`).
    /// Pin and archive exclude each other, so a core never reports both; if two devices ever
    /// converge on both, the pin wins and the item
    /// stays visible.
    pub fn is_shown_archived(&self) -> bool {
        self.archived && !self.pinned
    }
}

/// What the app hands to the core to send.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemDraft {
    /// Item id chosen by the queue (UUID v4) so that a retry is idempotent.
    #[serde(default)]
    pub id: Option<String>,
    pub title: Option<String>,
    pub text: Option<TextBody>,
    pub url: Option<String>,
    pub attachments: Vec<Attachment>,
    pub secret: bool,
    pub expires_at: Option<i64>,
    pub read_policy: ReadPolicy,
    pub source_format: Option<SourceFormat>,
}

impl ItemDraft {
    pub fn total_attachment_bytes(&self) -> u64 {
        self.attachments.iter().map(|a| a.size).sum()
    }
}

impl std::fmt::Debug for Item {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Item")
            .field("id", &self.id)
            .field("secret", &self.secret)
            .field("has_title", &self.title.is_some())
            .field("has_text", &self.text.is_some())
            .field("has_url", &self.url.is_some())
            .field("attachments", &self.attachments.len())
            .field("created_at", &self.created_at)
            .field("expires_at", &self.expires_at)
            .field("read_policy", &self.read_policy)
            .field("read", &self.read)
            .field("outgoing", &self.outgoing)
            .field("unreadable", &self.unreadable)
            .field("unreadable_reason", &self.unreadable_reason)
            .field("pinned", &self.pinned)
            .field("archived", &self.archived)
            .finish_non_exhaustive()
    }
}

impl std::fmt::Debug for ItemDraft {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ItemDraft")
            .field("secret", &self.secret)
            .field("has_title", &self.title.is_some())
            .field("has_text", &self.text.is_some())
            .field("has_url", &self.url.is_some())
            .field("attachments", &self.attachments.len())
            .field("expires_at", &self.expires_at)
            .field("read_policy", &self.read_policy)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum DeviceState {
    #[default]
    Verified,
    /// Listed by the relay but not vouched for by the signed chain of trust: nothing is
    /// ever encrypted for it.
    Unverified,
    Revoked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Device {
    pub id: String,
    pub name: String,
    pub platform: String,
    pub last_seen: Option<i64>,
    pub this_device: bool,
    #[serde(default)]
    pub state: DeviceState,
    /// Whole days since this other device last showed up, when that is longer than the relay's
    /// `inactive_after_days` (see `inactivity.rs`). `None` for this PC, for a device that never
    /// showed up, and for an active one. Only a hint: Tomi cannot tell that the app is gone.
    #[serde(default)]
    pub inactive_days: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TransferDirection {
    /// This PC sends its history to another device.
    Send,
    /// Another device offers its history to this PC.
    Receive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TransferPhase {
    /// An incoming history waits for the user's answer (yes or no, nothing in between).
    Offered,
    Running,
    Done,
    Failed,
    Cancelled,
    Declined,
}

/// A history transfer, either way. The whole history goes, secrets and files included
/// (`docs/history-and-expiry-decision.md`, D2): there is no per-type choice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Transfer {
    pub id: String,
    pub direction: TransferDirection,
    pub peer: DeviceRef,
    pub phase: TransferPhase,
    pub items_total: u64,
    pub items_done: u64,
    pub files_total: u64,
    pub files_done: u64,
    /// Machine code of the failure, when there was one.
    pub error: Option<String>,
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(text: &TextBody, draft: &ItemDraft) -> Item {
        Item {
            id: "i1".into(),
            title: draft.title.clone(),
            text: Some(text.clone()),
            url: draft.url.clone(),
            attachments: vec![],
            secret: true,
            source_device: DeviceRef {
                id: "d".into(),
                name: "Pixel".into(),
            },
            created_at: 1,
            expires_at: None,
            read_policy: ReadPolicy::Normal,
            read: false,
            outgoing: false,
            imported: false,
            delivery: vec![],
            source_format: None,
            unreadable: false,
            unreadable_reason: None,
            pinned: false,
            pinned_at: None,
            archived: false,
            archived_at: None,
        }
    }

    fn file(slot: Option<u32>) -> Attachment {
        Attachment {
            name: "f".into(),
            mime: "text/plain".into(),
            size: 1,
            path: None,
            thumbnail: None,
            available: true,
            slot,
        }
    }

    #[test]
    fn the_core_is_asked_for_the_slot_not_the_position() {
        let text = TextBody {
            plain: "x".into(),
            html: None,
            markdown: None,
        };
        let draft = ItemDraft {
            id: None,
            title: None,
            text: None,
            url: None,
            attachments: vec![],
            secret: false,
            expires_at: None,
            read_policy: ReadPolicy::Normal,
            source_format: None,
        };
        let mut i = item(&text, &draft);
        // Slots 0 and 2 of the original payload were removed: the list holds slots 1 and 3.
        i.attachments = vec![file(Some(1)), file(Some(3))];
        assert_eq!(i.attachment_slot(0), Some(1));
        assert_eq!(i.attachment_slot(1), Some(3));
        assert_eq!(i.attachment_slot(2), None);
        // By slot: slot 3 is the SECOND file of the list, slot 0 (removed) is nobody.
        assert_eq!(
            i.attachment_by_slot(3).map(|(position, _)| position),
            Some(1)
        );
        assert!(i.attachment_by_slot(0).is_none());
        // Without a slot (draft, fake core) the position is the slot.
        i.attachments = vec![file(None), file(None)];
        assert_eq!(i.attachment_slot(1), Some(1));
    }

    #[test]
    fn debug_output_never_contains_secret_values_or_content() {
        let text = TextBody {
            plain: "contenu-prive".into(),
            html: Some("<b>html-prive</b>".into()),
            markdown: None,
        };
        let draft = ItemDraft {
            id: None,
            title: Some("titre-prive".into()),
            text: Some(text.clone()),
            url: Some("https://prive.example/".into()),
            attachments: vec![],
            secret: true,
            expires_at: None,
            read_policy: ReadPolicy::Normal,
            source_format: None,
        };
        let item = item(&text, &draft);
        let dump = format!("{text:?} {draft:?} {item:?} {:?}", Some(&item));
        for leaked in [
            "contenu-prive",
            "html-prive",
            "titre-prive",
            "prive.example",
        ] {
            assert!(!dump.contains(leaked), "Debug leaks {leaked}: {dump}");
        }
        assert!(dump.contains(REDACTED));
        assert!(dump.contains("i1"));
    }

    #[test]
    fn an_old_payload_without_the_pin_fields_still_reads_as_not_pinned() {
        let json = r#"{"id":"i","secret":false,"sourceDevice":{"id":"d","name":"n"},
            "createdAt":1,"read":false,"outgoing":false}"#;
        let item: Item = serde_json::from_str(json).unwrap();
        assert!(!item.pinned);
        assert_eq!(item.pinned_at, None);
        assert!(!item.imported);
        assert!(!item.archived);
        assert_eq!(item.archived_at, None);
    }

    #[test]
    fn an_attachment_is_available_unless_the_core_says_otherwise() {
        let a: Attachment =
            serde_json::from_str(r#"{"name":"a","mime":"text/plain","size":1}"#).unwrap();
        assert!(a.available);
    }
}
