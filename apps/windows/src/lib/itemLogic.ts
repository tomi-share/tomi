// Pure rules of the item cards, mirroring Android (ui/feed/CardTap.kt, ui/code/CodeFileLogic.kt,
// core/DeliveryStatus.kt). No Tauri call, no state.

import type { AttachmentView, ItemView } from "./api";
import type { IconName } from "./icons";

// ---------------------------------------------------------------- tap on a card

export type CardTapAction =
  | "openLink"
  | "reveal"
  | "hide"
  | "openAttachment"
  | "openCodeFile"
  | "openImage"
  | "openDetail"
  | "none";

/** What the card measured about its own rendering: is a part of the content cut? */
export interface CardOverflow {
  title: boolean;
  body: boolean;
}
export const NO_OVERFLOW: CardOverflow = { title: false, body: false };

/** Only http(s) links are opened (the core and `open_item` check again). */
export function isHttpUrl(url: string | null): boolean {
  if (!url) return false;
  try {
    const u = new URL(url);
    return u.protocol === "http:" || u.protocol === "https:";
  } catch {
    return false;
  }
}

/**
 * A cut content has to be read in the detail. A link keeps its natural action even when its title
 * is cut (a page title is often longer than two lines and the address is what matters): only a cut
 * text body sends a link to the detail.
 */
function needsDetail(item: ItemView, overflow: CardOverflow): boolean {
  if (item.kind === "link") return overflow.body;
  return overflow.title || overflow.body;
}

/**
 * What a click on a card does: the natural action of the type. The detail is the target only when
 * it brings something (a cut content); see the table in CardTap.kt.
 */
export function tapAction(item: ItemView, revealed: boolean, overflow: CardOverflow): CardTapAction {
  if (item.unreadable) return "none";
  if (item.secret) {
    if (!revealed) return "reveal";
    return overflow.title || overflow.body ? "openDetail" : "hide";
  }
  if (needsDetail(item, overflow)) return "openDetail";
  switch (item.kind) {
    case "link":
      return isHttpUrl(item.url) ? "openLink" : "openDetail";
    case "image":
      return item.attachments.length === 1 ? "openImage" : "openDetail";
    case "file":
      if (item.attachments.length !== 1) return "openDetail";
      return isCodeFile(item.attachments[0]) ? "openCodeFile" : "openAttachment";
    case "files":
      return "openDetail";
    case "text":
    case "richText":
      return "none";
  }
}

/** "Lire la suite" shows exactly when the click opens the detail because the content is cut. */
export function showsReadMore(item: ItemView, revealed: boolean, overflow: CardOverflow): boolean {
  return !item.unreadable && (!item.secret || revealed) && needsDetail(item, overflow);
}

/**
 * What an item offers beyond pin and archive (the "⋮" menu of a card, the toolbar of the detail).
 * `revealed`: a hidden item shown; its files exist for "Ouvrir" only then.
 */
export interface ItemActionFlags {
  hasFiles: boolean;
  canOpen: boolean;
  /** Only an address: "Copier le lien" instead of "Copier". */
  linkOnly: boolean;
  canCopy: boolean;
  /** Never for a hidden item: the copy would stay in clear in the Downloads folder. */
  canSave: boolean;
}

export function itemActionFlags(item: ItemView, revealed: boolean): ItemActionFlags {
  const hasFiles = item.attachmentCount > 0 && (!item.secret || revealed);
  return {
    hasFiles,
    canOpen:
      !item.unreadable &&
      ((!item.secret && item.url !== null && item.attachmentCount === 0 && isHttpUrl(item.url)) ||
        (hasFiles && item.attachmentCount === 1)),
    linkOnly: !item.secret && item.url !== null && item.preview === null && item.attachmentCount === 0,
    canCopy:
      !item.unreadable &&
      (item.secret || item.preview !== null || item.url !== null || item.attachmentCount > 0),
    canSave: hasFiles && !item.unreadable && !item.secret,
  };
}

/** The explicit copy icon of a text card. Never for a hidden item, whose copy is gated. */
export function showsCopyButton(item: ItemView): boolean {
  return !item.unreadable && !item.secret && item.preview !== null;
}

// ---------------------------------------------------------------- attachments

/**
 * Every delivery of the item is still queued (it has not left this PC): the core refuses to remove
 * a file then, so the UI offers no removal for those few seconds (Android: `AttachmentRemovals`).
 */
export function isQueued(item: ItemView): boolean {
  return item.delivery.length > 0 && item.delivery.every((d) => d.state === "queued");
}

/** A file of the item may be removed from here: a readable item that has left this PC. */
export function canRemoveAttachment(item: ItemView): boolean {
  return !item.unreadable && item.attachmentCount > 0 && !isQueued(item);
}

/**
 * The rows of the files of the detail get a "Supprimer ce fichier" action. An item with a single
 * file has none: that file IS the item, and "Supprimer…" already says so.
 */
export function canRemoveFromBatch(item: ItemView): boolean {
  return canRemoveAttachment(item) && item.attachmentCount >= 2;
}

export function isImage(a: Pick<AttachmentView, "mime">): boolean {
  return a.mime.startsWith("image/");
}

/** Extensions the code screen opens. A highlighter exists for the ones in `highlight.ts`. */
const CODE_EXTENSIONS = new Set([
  "rs", "js", "mjs", "cjs", "jsx", "ts", "mts", "cts", "tsx", "css", "html", "htm", "json", "py",
  "java", "sh", "bash", "zsh", "go", "c", "h", "yaml", "yml", "md", "markdown", "php", "phtml",
  "kt", "kts", "rb", "cpp", "cc", "cxx", "hpp", "hh", "sql", "toml", "gradle", "swift", "cs", "lua",
  "dart", "scala", "svelte", "vue", "xml", "ps1", "bat", "ini",
]);

/** Lower-case extension without the dot; null when there is none (`Makefile`, `.env`). */
export function extensionOf(fileName: string): string | null {
  const name = fileName.split(/[\\/]/).pop() ?? fileName;
  const dot = name.lastIndexOf(".");
  if (dot <= 0 || dot === name.length - 1) return null;
  return name.slice(dot + 1).toLowerCase();
}

/** An image is never a code file, whatever its name. */
export function isCodeFile(a: Pick<AttachmentView, "mime" | "name">): boolean {
  if (isImage(a)) return false;
  const ext = extensionOf(a.name);
  return ext !== null && CODE_EXTENSIONS.has(ext);
}

/** One attachment row's click: images and code files open in Tomi, the others are inert. */
export function rowAction(a: AttachmentView, hidden: boolean): "openImage" | "openCodeFile" | "none" {
  // A hidden item's files open with "Ouvrir" once it is shown.
  if (hidden || !a.available) return "none";
  if (isImage(a)) return "openImage";
  if (isCodeFile(a)) return "openCodeFile";
  return "none";
}

// ---------------------------------------------------------------- attachment tiles
// Mirror of Android's ui/feed/AttachmentLogic.kt.

/** Attachments a feed card shows before "N autres fichiers" (the detail shows them all). */
export const MAX_VISIBLE_FILES = 3;
/** Longest extension still shown as such in the badge of a tile ("PDF", "WEBP"); "drawio" gets an icon. */
const MAX_BADGE_EXTENSION = 4;
/** An "extension" longer than that is the end of a sentence, not an extension. */
const MAX_EXTENSION_LENGTH = 10;

/**
 * `Courrier banque.pdf` -> radical "Courrier banque" + extension ".pdf" (dot included). No extension
 * (empty) for a name without dot, with a leading dot only (".gitignore"), ending with a dot, or
 * whose "extension" has a space or is too long.
 */
export function splitExtension(name: string): { radical: string; extension: string } {
  const dot = name.lastIndexOf(".");
  if (dot <= 0) return { radical: name, extension: "" };
  const extension = name.slice(dot);
  if (extension.length === 1 || extension.length > MAX_EXTENSION_LENGTH || /\s/.test(extension)) {
    return { radical: name, extension: "" };
  }
  return { radical: name.slice(0, dot), extension };
}

/** The upper-case extension shown in the badge of a tile ("PDF"), or null: none, or over 4 letters. */
export function badgeExtension(name: string): string | null {
  const extension = splitExtension(name).extension.slice(1);
  if (extension === "" || extension.length > MAX_BADGE_EXTENSION) return null;
  if (!/^[\p{L}\p{N}]+$/u.test(extension)) return null;
  return extension.toUpperCase();
}

/**
 * The name cut BEFORE its extension so that it fits ("Courrier banque fonds de rési….pdf"). `fits`
 * says whether a candidate fits (the views measure the text in the width it really has); it is
 * assumed monotonic, which lets the search be a dichotomy. A name that fits is returned as is;
 * without extension, the end of the name is cut. Never cuts a surrogate pair (an emoji).
 */
export function fitFileName(name: string, fits: (candidate: string) => boolean): string {
  if (fits(name)) return name;
  const { radical, extension } = splitExtension(name);
  const candidate = (length: number): string => {
    let end = length;
    const unit = radical.charCodeAt(end - 1);
    if (end > 0 && end < radical.length && unit >= 0xd800 && unit <= 0xdbff) end -= 1;
    return radical.slice(0, end).trimEnd() + "…" + extension;
  };
  // The longest radical length that still fits; 0 ("…" + extension) is the floor, even if it does not fit either.
  let low = 0;
  let high = radical.length - 1;
  while (low < high) {
    const middle = Math.ceil((low + high) / 2);
    if (fits(candidate(middle))) low = middle;
    else high = middle - 1;
  }
  return candidate(low);
}

export type AttachmentLook = "tile" | "frame";
export type AttachmentPlan =
  /** Images only (2 or more), all with a thumbnail: `shown` cells (2 or 3), "+more" on the last one. */
  | { kind: "mosaic"; shown: number; more: number }
  /** Tiles and frames in order, then "N autres fichiers" when `more` > 0. */
  | { kind: "items"; entries: { index: number; look: AttachmentLook }[]; more: number };

/**
 * What a card shows for its attachments. Detail (`expanded`): everything in order, an image with a
 * thumbnail is a frame, the rest tiles. Feed: 3 at most; images only (2 or more, with thumbnails)
 * make a mosaic; a lone image with a thumbnail is a frame; everything else is tiles (an image gets
 * a mini-thumbnail in its badge). No thumbnail at all when `showThumbnails` is false.
 */
export function attachmentPlan(
  list: Pick<AttachmentView, "mime" | "thumbnail">[],
  expanded: boolean,
  showThumbnails: boolean,
): AttachmentPlan {
  const framed = (a: Pick<AttachmentView, "mime" | "thumbnail">) =>
    showThumbnails && isImage(a) && !!a.thumbnail;
  if (expanded) {
    return {
      kind: "items",
      entries: list.map((a, index) => ({ index, look: framed(a) ? "frame" : "tile" })),
      more: 0,
    };
  }
  const visible = list.slice(0, MAX_VISIBLE_FILES);
  const more = list.length - visible.length;
  if (list.length >= 2 && visible.every(framed) && list.every(isImage)) {
    return { kind: "mosaic", shown: visible.length, more };
  }
  const single = list.length === 1 && framed(list[0]);
  return {
    kind: "items",
    entries: visible.map((a, index) => ({ index, look: single && framed(a) ? "frame" : "tile" })),
    more,
  };
}

// ---------------------------------------------------------------- read-once secrets

/**
 * A read-once hidden item that carries files is NOT consumed by showing, opening or copying it.
 * Such an item can only come from an older device: the composer no longer offers "Après lecture"
 * with files, and "Enregistrer" (which used to consume it) is refused for a hidden item, so it
 * stays until it is deleted or expires. Mirrors `reveal_must_not_consume` on the Rust side.
 */
export function keptUntilSave(item: ItemView): boolean {
  return (
    item.secret && item.readPolicy === "afterFirstRead" && !item.outgoing && item.attachmentCount > 0
  );
}

/** The item is already erased in the core but still shown (read once, no files, revealed). */
export function consumedButShown(item: ItemView, revealed: boolean): boolean {
  return (
    revealed &&
    item.secret &&
    item.readPolicy === "afterFirstRead" &&
    !item.outgoing &&
    !keptUntilSave(item)
  );
}

// ---------------------------------------------------------------- icons

export function kindIcon(item: ItemView): IconName {
  if (item.unreadable) return "error";
  if (item.secret) return "visibility_off";
  switch (item.kind) {
    case "image":
      return "image";
    case "file":
    case "files":
      return "draft";
    case "link":
      return "link";
    case "richText":
      return item.hasMarkdown ? "markdown" : "notes";
    case "text":
      return "notes";
  }
}

// ---------------------------------------------------------------- delivery

export type DeliverySummary =
  | { state: "received" }
  | { state: "queued" }
  | { state: "sent" }
  | { state: "failed" }
  | { state: "delivered"; delivered: number; total: number }
  | { state: "expired"; delivered: number; devices: string[] };

/** Summary of the per-device deliveries of a sent item (core/DeliveryStatus.kt). */
export function deliverySummary(item: ItemView): DeliverySummary {
  if (!item.outgoing) return { state: "received" };
  const d = item.delivery;
  if (d.length === 0) return { state: "sent" };
  if (d.some((x) => x.state === "failed")) return { state: "failed" };
  if (d.some((x) => x.state === "queued")) return { state: "queued" };
  const delivered = d.filter((x) => x.state === "delivered" || x.state === "read").length;
  const expired = d.filter((x) => x.state === "expired");
  if (expired.length > 0) {
    return { state: "expired", delivered, devices: expired.map((x) => x.device.name) };
  }
  if (delivered > 0) return { state: "delivered", delivered, total: d.length };
  return { state: "sent" };
}

// ---------------------------------------------------------------- archive

/** What the menu entry of a card asks the core to do (Android: ArchiveAction). */
export type ArchiveAction = "archive" | "unarchive";

/**
 * An outgoing item that is still queued or failed stays where it is: a failed send must remain
 * visible, and the queue entry belongs to the send
 * flow (Android: ArchiveSwipe.canArchive). Every other item can be archived, an unreadable one too.
 */
export function canArchive(item: ItemView): boolean {
  return !(item.outgoing && item.delivery.some((d) => d.state === "queued" || d.state === "failed"));
}

/** "Désarchiver" for an archived item, "Archiver" for any other (Android: ArchiveSwipe.menuAction). */
export function archiveAction(item: ItemView): ArchiveAction {
  return item.archived ? "unarchive" : "archive";
}
