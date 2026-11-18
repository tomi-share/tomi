import { api, type AttachmentRemoval, type ItemView } from "./api";
import { feed } from "./feed.svelte";
import { format, fr } from "./i18n/fr";

/**
 * The actions of an item, shared by its card ("⋮" menu, copy icon) and by the toolbar of the
 * detail screen: one place, so that both do exactly the same thing.
 */

export async function copyItem(item: ItemView): Promise<void> {
  const consumed = item.secret && item.readPolicy === "afterFirstRead" && !item.outgoing;
  const ok = await feed.run(
    () => (item.secret ? api.copySecret(item.id) : api.copyItem(item.id, feed.preferredMode ?? undefined)),
    consumed ? fr.card.copiedAndGone : item.secret ? fr.card.copiedSecret : fr.card.copied,
  );
  if (ok) void feed.refresh();
}

export async function openItem(item: ItemView): Promise<void> {
  await feed.run(async () => {
    await api.openItem(item.id);
  });
  void feed.refresh();
}

export async function saveItem(item: ItemView): Promise<void> {
  try {
    const r = await api.saveItem(item.id);
    feed.say(format(fr.card.saved, { dir: r.dir }));
  } catch (e) {
    feed.fail(e);
  }
}

/** Deletes on this device or everywhere. Returns true when the core did it. */
export function deleteItem(item: ItemView, scope: "local" | "everywhere"): Promise<boolean> {
  return feed.run(
    () => api.deleteItem(item.id, scope),
    scope === "local" ? fr.card.deletedLocal : fr.card.deletedEverywhere,
  );
}

/**
 * Removes ONE file of an item (by slot), from this PC or everywhere, and says what happened. The
 * core deletes the whole item, with the same scope, when nothing else is left in it: the message is
 * then the one of an item deletion. Returns the answer of the core, `null` when it failed.
 */
export async function removeAttachment(
  item: ItemView,
  slot: number,
  everywhere: boolean,
  image: boolean,
): Promise<AttachmentRemoval | null> {
  try {
    const removal = await api.deleteAttachment(item.id, slot, everywhere);
    if (removal === "itemDeleted") {
      feed.say(everywhere ? fr.card.deletedEverywhere : fr.card.deletedLocal);
    } else if (image) {
      feed.say(everywhere ? fr.card.imageDeletedEverywhere : fr.card.imageDeletedLocal);
    } else {
      feed.say(everywhere ? fr.card.fileDeletedEverywhere : fr.card.fileDeletedLocal);
    }
    void feed.refresh();
    return removal;
  } catch (e) {
    feed.fail(e);
    return null;
  }
}
