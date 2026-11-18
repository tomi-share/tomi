// Typed wrappers over the Tauri commands (src-tauri/src/commands.rs) and events.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type ContentKind = "link" | "text" | "richText" | "image" | "file" | "files";
/** "Aperçu" (styled) or "Markdown" (the source). HTML is never shown as source. */
export type RenderMode = "preview" | "source";
export type SourceFormat = "plain" | "markdown" | "html";
export type ExpiryChoice = "never" | "afterFirstRead" | "hour1" | "day1" | "day7" | "day30";
/** "archives" is the only filter that shows archived items; every other one leaves them out. */
export type ItemFilter = "all" | "links" | "text" | "files" | "secrets" | "archives";
export type DeliveryState = "queued" | "sent" | "delivered" | "read" | "expired" | "failed";
export type DeviceState = "verified" | "unverified" | "revoked";
/** `notFound`: the relay no longer finds the space or this PC (never a proof of anything). */
export type SpaceState = "active" | "revoked" | "updateRequired" | "notFound" | "blocked";
export type TransferPhase = "offered" | "running" | "done" | "failed" | "cancelled" | "declined";

export interface DeviceRef {
  id: string;
  name: string;
}
export interface Device {
  id: string;
  name: string;
  platform: string;
  lastSeen: number | null;
  thisDevice: boolean;
  state: DeviceState;
  /** Whole days since another device last showed up, once past the relay's threshold; else null. */
  inactiveDays: number | null;
}
export interface Attachment {
  name: string;
  mime: string;
  size: number;
  path?: string;
  thumbnail?: string;
}
export interface AttachmentView {
  name: string;
  mime: string;
  size: number;
  thumbnail: string | null;
  /** false when the file cannot be fetched any more. */
  available: boolean;
  /**
   * Position of the file in the payload of its item, stable for its life: what the core is
   * asked for (read, open, remove). NOT its position in the list, which changes once a file is
   * removed (here or on another device).
   */
  slot: number;
}
/** What `deleteAttachment` did. */
export type AttachmentRemoval = "attachmentRemoved" | "itemDeleted";
export interface ItemView {
  id: string;
  kind: ContentKind;
  title: string | null;
  preview: string | null;
  url: string | null;
  rich: boolean;
  /** Has a Markdown representation: only then is the "Markdown" mode offered. */
  hasMarkdown: boolean;
  /** The text is longer than `preview`. */
  previewTruncated: boolean;
  sourceFormat: SourceFormat | null;
  /** Empty for a hidden item: only `attachmentCount` is known until it is revealed. */
  attachments: AttachmentView[];
  attachmentCount: number;
  /** Hidden item ("masqué"): no title, preview, link or file name until revealed. */
  secret: boolean;
  sourceDevice: DeviceRef;
  createdAt: number;
  expiresAt: number | null;
  readPolicy: "normal" | "afterFirstRead";
  read: boolean;
  outgoing: boolean;
  delivery: { device: DeviceRef; state: DeliveryState }[];
  unreadable: boolean;
  /** Why an unreadable item is: `updateRequired` (unknown format) or `other`; `null` when readable. */
  unreadableReason: "updateRequired" | "other" | null;
  /** Pinned on all devices (spec/pin-contract.md). */
  pinned: boolean;
  /** Time of the decision to pin (ms), to sort the "Épinglés" section; null if not pinned. */
  pinnedAt: number | null;
  /**
   * Archived on all devices and not pinned (the pin wins), spec/archive-contract.md. Only shown
   * under the "Archives" filter, or in a search under "Tous".
   */
  archived: boolean;
  /** Time of the decision to archive (ms); null if not archived. */
  archivedAt: number | null;
}
/** What an explicit "Afficher" shows of a hidden item. */
export interface RevealedSecret {
  title: string | null;
  text: string | null;
  url: string | null;
  attachments: { name: string; size: number; slot: number }[];
  /** Read once and not consumed yet: call `consumeRevealed` once the value is really on screen. */
  consumeAfterShow: boolean;
}
export type Rendered = { kind: "plain"; text: string } | { kind: "document"; srcdoc: string };
export interface RenderedItem {
  /** The mode shown: the one asked for if the item supports it, else "preview". */
  mode: RenderMode;
  /** The modes the item offers. */
  modes: RenderMode[];
  rendered: Rendered;
}

export type JobState =
  | { state: "pending"; sendAt: number }
  | { state: "sending"; sentBytes: number; totalBytes: number }
  | { state: "done"; itemId: string }
  /** Kept by the core, not accepted by the relay yet (offline): goes out by itself. */
  | { state: "queued"; itemId: string }
  | { state: "failed"; error: string }
  | { state: "cancelled" };
export type JobView = {
  id: string;
  secret: boolean;
  hasText: boolean;
  hasUrl: boolean;
  fileCount: number;
} & JobState;

export interface Rejection {
  name: string;
  size: number;
  reason: "tooLarge" | "itemTotalExceeded" | "notAFile" | "tooMany";
}
export interface FilesResult {
  accepted: Attachment[];
  rejected: Rejection[];
}
export interface PrefillPayload {
  url: string | null;
  title: string | null;
  text: string | null;
  files: Attachment[];
  rejected: Rejection[];
  truncated: boolean;
  error: string | null;
}
export interface Detected {
  kind: ContentKind;
  sourceFormat: SourceFormat | null;
  url: string | null;
  title: string | null;
  looksLikeSecret: boolean;
}
export interface Settings {
  shortcut: string;
  renderMode: RenderMode | null;
  autostart: boolean;
  notifications: boolean;
  onboardingDone: boolean;
  downloadDir: string | null;
}
export interface CoreStatus {
  paired: boolean;
  space: SpaceState;
  /** Machine code of why the local store cannot be used, when it cannot. */
  unavailable: string | null;
}
export interface AppInfo {
  version: string;
  /** The demo core is in use: only a debug build sends it (the field does not exist in a release). */
  demo?: boolean;
  status: CoreStatus;
  thisDevice: Device;
  settings: Settings;
  shortcutRegistered: boolean;
  unread: number;
  downloadDir: string | null;
}
export interface ExpiryRules {
  options: ExpiryChoice[];
  default: ExpiryChoice;
}
export interface AllExpiryRules {
  normal: ExpiryRules;
  secret: ExpiryRules;
  /** A hidden item with at least one file: no "Après lecture" (it would be erased before the files can be opened). */
  secretWithFiles: ExpiryRules;
}
/** A history transfer, either way: the whole history, no choice per type. */
export interface Transfer {
  id: string;
  direction: "send" | "receive";
  peer: DeviceRef;
  phase: TransferPhase;
  itemsTotal: number;
  itemsDone: number;
  filesTotal: number;
  filesDone: number;
  error: string | null;
}

// ---- pairing (spec/pairing-v1.md) ----

/** What this PC can do about pairing right now (§10.1). */
export interface PairingOptions {
  /** Member of a space: it can invite a device, not create or join one. */
  paired: boolean;
  /** The name this PC will have in the space (the computer name). */
  deviceName: string;
  /**
   * Not paired only. Whether the relay holds a space at all, which says nothing about the space
   * of the person installing this PC (a relay can hold one space per person): "Créer" and
   * "Rejoindre" are always offered. Only `null` is used: the relay does not answer.
   */
  spaceInitialized: boolean | null;
  /** An interrupted pairing left something sealed on this PC and is being settled. */
  pending: boolean;
  /** Not paired only. The relay this PC would use (`host[:port]`); `null` when it has none. */
  relayAuthority: string | null;
  /** What the automatic resumption of an interrupted pairing concluded. */
  resumed: "provisioned" | "discarded" | null;
}
export type PairingStage =
  | "waiting"
  | "verify"
  | "provisioning"
  | "done"
  | "cancelled"
  | "failed";
/** The other device, as it announced itself on the authenticated channel. */
export interface PairingPeer {
  name: string;
  platform: string;
}
/** A history, in items and bytes: an estimate, to display. */
export interface HistorySize {
  items: number;
  bytes: number;
}
/** The QR code of the pairing link, as one SVG path in module units. */
export interface Qr {
  size: number;
  quiet: number;
  path: string;
}
export interface PairingSession {
  stage: PairingStage;
  /** Invite, while waiting: `4821-crayon-renard`. Secret. */
  code: string | null;
  qr: Qr | null;
  /** Invite, while waiting: when the code stops being valid (ms, clock of this PC). */
  expiresAt: number | null;
  /** A previous code led nowhere and this one replaces it. */
  renewed: boolean;
  /** The verification code in groups of three digits, `482 913`. Secret. Never typed. */
  sas: string | null;
  /** Join, verifying: the relay this PC joins through, to show IN FULL next to the code. */
  relayAuthority: string | null;
  peer: PairingPeer | null;
  /** Join: devices in the space, the inviter included. */
  memberCount: number | null;
  /** Join, while verifying: what the inviter offers (the yes / no question). Invite, done: the history in preparation. */
  history: HistorySize | null;
  /** The relay cannot be reached right now; the pairing goes on when it can. */
  offline: boolean;
  error: AppError | null;
}
/** What a pasted or typed text is (`pairingReadInput`). */
export interface InputRead {
  kind: "pairLink" | "setupLink" | "code" | "token" | "unknown";
  /** `https://host[:port]`, read from a link. Not trusted: shown in full, confirmed by the user. */
  relayUrl: string | null;
  relayAuthority: string | null;
}
/** A pairing link that opened the app. The relay is not trusted. */
export interface PairLinkIn {
  code: string;
  relayUrl: string | null;
}
/** A creation link that opened the app. The relay is not trusted. */
export interface SetupLinkIn {
  token: string;
  relayUrl: string;
}
export type PairingView =
  | { role: "none" }
  | ({ role: "invite" | "join" } & PairingSession);

// ---- departure (spec/api-v1.md 7.9) ----
// The rules (Windows Hello, the 2 s anti-tap, the back-off) live in Rust; the window only draws
// this snapshot and sends the clicks.

export type DepartureKind = "leave" | "deleteSpace";
/** Why a departure still waits. In every case nothing was erased. */
export type PendingReason = "network" | "server" | "notFound" | "notSent";
/** Why the departure was refused for good. */
export type RefusalReason = "lastDevice" | "updateRequired" | "relayTooOld" | "unexpected";
export type LeaveScreen = "hidden" | "confirmLeave" | "confirmDelete" | "eraseWithoutProof";
/** The departure pending on this PC (the banner). */
export interface DepartureView {
  kind: DepartureKind;
  /** A proof was received and the erasure is under way: no button. */
  wiping: boolean;
  sinceMs: number | null;
  /** Reason of the last attempt, `null` before the first one. */
  last: PendingReason | null;
}
/** The dialog on screen. */
export interface LeaveUi {
  screen: LeaveScreen;
  busy: boolean;
  error: RefusalReason | null;
  failed: boolean;
  checked: boolean;
  notAllowed: boolean;
  checking: boolean;
  /** The confirm button is enabled from this instant (ms, clock of this PC). */
  armedAtMs: number | null;
  /**
   * The window of the gesture that opened the dialog (`popover` or `history`): only that window
   * draws it. Set by Rust; null while no dialog is up.
   */
  window: string | null;
}
export interface DepartureSnapshot {
  /** The core can do a departure at all. */
  available: boolean;
  departure: DepartureView | null;
  canEraseWithoutProof: boolean;
  ui: LeaveUi;
  /** An attempt (dialog, "Réessayer", automatic) is running: "Réessayer" is off, "Annuler" is not. */
  working: boolean;
  /** Windows Hello is on screen (nothing was sent yet). */
  authenticating: boolean;
  /** "Annuler" was pressed while an attempt was running: it goes on to its end and erases nothing. */
  cancelling: boolean;
}
export interface Cancelled {
  cancelled: boolean;
  kind: DepartureKind | null;
}

export interface DraftInput {
  title?: string | null;
  url?: string | null;
  text?: { plain: string; html?: string | null; markdown?: string | null } | null;
  attachments: Attachment[];
  /** Hidden item: its title, text, link and file names are part of the secret. */
  secret: boolean;
  expiry: ExpiryChoice;
  sourceFormat?: SourceFormat | null;
}

/** Error as serialised by the Rust side (`AppError`). */
export interface AppError {
  code: string;
  params: Record<string, unknown>;
}

export function asAppError(e: unknown): AppError {
  if (e && typeof e === "object" && "code" in e && typeof (e as AppError).code === "string") {
    const { code, params } = e as AppError;
    return { code, params: params ?? {} };
  }
  return { code: "error.internal", params: {} };
}

export const api = {
  listItems: (filter?: ItemFilter, query?: string) =>
    invoke<ItemView[]>("list_items", { filter, query }),
  itemView: (id: string) => invoke<ItemView>("item_view", { id }),
  renderItem: (id: string, mode?: RenderMode, dark?: boolean) =>
    invoke<RenderedItem>("render_item", { id, mode, dark }),
  /** The text of a code file (2 Mo at most). Never for a hidden item. */
  readCodeFile: (id: string, slot: number) =>
    invoke<{ name: string; text: string }>("read_code_file", { id, slot }),
  /** One image as a data: URL (16 Mo at most). Never for a hidden item. */
  readImage: (id: string, slot: number) =>
    invoke<{ name: string; dataUrl: string }>("read_image", { id, slot }),
  copyItem: (id: string, mode?: RenderMode) => invoke<void>("copy_item", { id, mode }),
  copySecret: (id: string) => invoke<void>("copy_secret", { id }),
  revealSecret: (id: string) => invoke<RevealedSecret>("reveal_secret", { id }),
  pinItem: (id: string) => invoke<void>("pin_item", { id }),
  unpinItem: (id: string) => invoke<void>("unpin_item", { id }),
  archiveItem: (id: string) => invoke<void>("archive_item", { id }),
  unarchiveItem: (id: string) => invoke<void>("unarchive_item", { id }),
  openItem: (id: string) => invoke<void>("open_item", { id }),
  saveItem: (id: string) => invoke<{ dir: string; paths: string[] }>("save_item", { id }),
  revealInFolder: (path: string) => invoke<void>("reveal_in_folder", { path }),
  deleteItem: (id: string, scope: "local" | "everywhere") =>
    invoke<void>("delete_item", { id, scope }),
  /**
   * Removes ONE file of an item (by slot), from this PC or everywhere. `itemDeleted`: it was the
   * last thing in the item, which the core deleted instead (same scope).
   */
  deleteAttachment: (id: string, slot: number, everywhere: boolean) =>
    invoke<AttachmentRemoval>("delete_attachment", { id, slot, everywhere }),
  /** The relay takes "remove this file everywhere"; otherwise only "from this PC" is offered. */
  attachmentRemovalEverywhere: () => invoke<boolean>("attachment_removal_everywhere"),
  markRead: (id: string) => invoke<void>("mark_read", { id }),
  /** Consumes a read-once hidden item after its value was displayed (Windows Hello window open). */
  consumeRevealed: (id: string) => invoke<void>("consume_revealed", { id }),
  startDrag: (id: string) => invoke<void>("start_drag", { id }),

  detectContent: (plain: string, html?: string | null, fileMimes?: string[]) =>
    invoke<Detected>("detect_content", { plain, html, fileMimes }),
  expiryRules: () => invoke<AllExpiryRules>("expiry_rules"),
  describeFiles: (paths: string[], existingBytes: number, existingCount: number) =>
    invoke<FilesResult>("describe_files", { paths, existingBytes, existingCount }),
  pickFiles: (existingBytes: number, existingCount: number) =>
    invoke<FilesResult>("pick_files", { existingBytes, existingCount }),
  stageBlob: (name: string, bytes: Uint8Array) =>
    invoke<Attachment>("stage_blob", bytes, { headers: { "x-name": encodeURIComponent(name) } }),
  discardStaged: (path: string) => invoke<void>("discard_staged", { path }),
  sendDraft: (input: DraftInput) => invoke<JobView>("send_draft", { input }),
  cancelJob: (id: string) => invoke<boolean>("cancel_job", { id }),
  retryJob: (id: string) => invoke<boolean>("retry_job", { id }),
  dismissJob: (id: string) => invoke<void>("dismiss_job", { id }),
  activeJobs: () => invoke<JobView[]>("active_jobs"),
  takePrefill: () => invoke<PrefillPayload | null>("take_prefill"),

  appInfo: () => invoke<AppInfo>("app_info"),
  devices: () => invoke<Device[]>("devices"),
  /** Renames a device of the space, this PC included. The core checks the name (`error.device_name_invalid`). */
  renameDevice: (id: string, name: string) => invoke<void>("rename_device", { id, name }),
  /** Removes a device and renews the space key. Never this PC (`error.revoke_self`). */
  revokeDevice: (id: string) => invoke<void>("revoke_device", { id }),
  syncNow: () => invoke<void>("sync_now"),
  transfers: () => invoke<Transfer[]>("transfers"),
  sendHistory: (deviceId: string) => invoke<void>("send_history", { deviceId }),
  answerHistory: (transferId: string, accept: boolean) =>
    invoke<void>("answer_history", { transferId, accept }),
  cancelTransfer: (transferId: string) => invoke<void>("cancel_transfer", { transferId }),

  departureState: () => invoke<DepartureSnapshot>("departure_state"),
  /** Menu entry "Se déconnecter": opens its confirmation. */
  departureOpenLeave: () => invoke<DepartureSnapshot>("departure_open_leave"),
  departureConfirmLeave: () => invoke<DepartureSnapshot>("departure_confirm_leave"),
  /** Armed after 2 s, then Windows Hello, or the account password when Hello is unavailable (`error.strong_auth_*` otherwise). */
  departureConfirmDelete: () => invoke<DepartureSnapshot>("departure_confirm_delete"),
  /** "Effacer quand même": the core is asked whether it allows it. */
  departureOpenErase: () => invoke<DepartureSnapshot>("departure_open_erase"),
  departureSetEraseChecked: (checked: boolean) =>
    invoke<DepartureSnapshot>("departure_set_erase_checked", { checked }),
  /** Checkbox, 2 s, allowed by the core, then Windows Hello. */
  departureConfirmErase: () => invoke<DepartureSnapshot>("departure_confirm_erase"),
  departureDismiss: () => invoke<DepartureSnapshot>("departure_dismiss"),
  departureRetry: () => invoke<DepartureSnapshot>("departure_retry"),
  departureCancel: () => invoke<Cancelled>("departure_cancel"),
  /** "Annuler" in a dialog whose attempt runs: the dialog closes, the core does not wait. */
  departureCancelRunning: () => invoke<Cancelled>("departure_cancel_running"),
  /** The window came back: a pending departure is looked at again (at most once a minute). */
  departurePoke: () => invoke<void>("departure_poke"),

  pairingOptions: () => invoke<PairingOptions>("pairing_options"),
  pairingState: () => invoke<PairingView>("pairing_state"),
  /** First device. The install token is used for this call only. */
  pairingCreateSpace: (
    installTokenOrLink: string,
    deviceName?: string | null,
    relayUrl?: string | null,
  ) => invoke<void>("pairing_create_space", { installToken: installTokenOrLink, deviceName, relayUrl }),
  pairingInvite: () => invoke<PairingView>("pairing_invite"),
  /** `relayUrl`: the "Relais" field (`https://host`), absent when the PC has its relay or the link names one. */
  pairingJoin: (codeOrLink: string, deviceName?: string | null, relayUrl?: string | null) =>
    invoke<PairingView>("pairing_join", { codeOrLink, deviceName, relayUrl }),
  /** What a pasted text is and the relay it names. Local: nothing is sent, no secret comes back. */
  pairingReadInput: (text: string) => invoke<InputRead>("pairing_read_input", { text }),
  /** The codes are the same. `importHistory` answers the history question (join only). */
  pairingConfirm: (importHistory: boolean) => invoke<void>("pairing_confirm", { importHistory }),
  /** The user refuses or cancels, at any stage. */
  pairingCancel: () => invoke<void>("pairing_cancel"),
  /** The screen was left: abandons a code on show or codes being compared, not a confirmed pairing. */
  pairingLeave: () => invoke<void>("pairing_leave"),
  pairingDismiss: () => invoke<void>("pairing_dismiss"),
  /** The word of the code list that the first letters typed so far name (four are enough). */
  pairingCompleteWord: (prefix: string) =>
    invoke<string | null>("pairing_complete_word", { prefix }),
  /** The code (and relay) of a pairing link that opened the app, once. */
  takePairLink: () => invoke<PairLinkIn | null>("take_pair_link"),
  /** The token and relay of a creation link that opened the app, once. */
  takeSetupLink: () => invoke<SetupLinkIn | null>("take_setup_link"),

  setShortcut: (spec: string) => invoke<string>("set_shortcut", { spec }),
  setNotifications: (enabled: boolean) => invoke<Settings>("set_notifications", { enabled }),
  setAutostart: (enabled: boolean) => invoke<Settings>("set_autostart", { enabled }),
  setRenderMode: (mode: RenderMode | null) => invoke<Settings>("set_render_mode", { mode }),
  chooseDownloadDir: (reset = false) => invoke<Settings>("choose_download_dir", { reset }),
  completeOnboarding: () => invoke<Settings>("complete_onboarding"),

  setPinned: (pinned: boolean) => invoke<void>("set_pinned", { pinned }),
  hidePopover: () => invoke<void>("hide_popover"),
  openWindow: (tab?: string) => invoke<void>("open_window", { tab }),
  quitApp: () => invoke<void>("quit_app"),
};

/** Events emitted by the Rust side. */
export const events = {
  itemsChanged: (cb: (unread: number) => void): Promise<UnlistenFn> =>
    listen<number>("items-changed", (e) => cb(e.payload)),
  /** The devices, a history transfer or the state of the space changed. */
  spaceChanged: (cb: () => void): Promise<UnlistenFn> => listen("space-changed", () => cb()),
  jobChanged: (cb: (job: JobView) => void): Promise<UnlistenFn> =>
    listen<JobView>("job-changed", (e) => cb(e.payload)),
  popoverShown: (cb: () => void): Promise<UnlistenFn> => listen("popover-shown", () => cb()),
  focusItem: (cb: (id: string) => void): Promise<UnlistenFn> =>
    listen<string>("focus-item", (e) => cb(e.payload)),
  prefillAvailable: (cb: () => void): Promise<UnlistenFn> =>
    listen("prefill-available", () => cb()),
  navigate: (cb: (tab: string) => void): Promise<UnlistenFn> =>
    listen<string>("navigate", (e) => cb(e.payload)),
  /** The departure (banner or dialog) changed: read `departureState` again. */
  departureChanged: (cb: () => void): Promise<UnlistenFn> =>
    listen("departure-changed", () => cb()),
  /** The pairing screen has something new: read `pairingState` again. */
  pairingChanged: (cb: () => void): Promise<UnlistenFn> => listen("pairing-changed", () => cb()),
  /** A `tomi-share://pair#` link arrived: `takePairLink` has its code. */
  pairLinkAvailable: (cb: () => void): Promise<UnlistenFn> =>
    listen("pair-link-available", () => cb()),
  /** A creation link arrived: `takeSetupLink` has its token. */
  setupLinkAvailable: (cb: () => void): Promise<UnlistenFn> =>
    listen("setup-link-available", () => cb()),
  notice: (cb: (err: AppError) => void): Promise<UnlistenFn> =>
    listen<AppError>("notice", (e) => cb(asAppError(e.payload))),
};
