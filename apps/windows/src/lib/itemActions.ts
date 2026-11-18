import { api, type ItemView } from "./api";
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
