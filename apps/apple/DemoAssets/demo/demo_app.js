// Tomi demo: filter and search in a list of items.
const ITEMS = [
  { id: 1, title: 'Lien Android', kind: 'link', pinned: true },
  { id: 2, title: 'Courses', kind: 'text', pinned: false },
  { id: 3, title: 'Photo du week-end', kind: 'image', pinned: false },
];

/**
 * Returns the items whose title contains `query` (case-insensitive).
 */
function search(items, query) {
  const needle = query.trim().toLowerCase();
  if (needle === '') return items;
  return items.filter((item) => item.title.toLowerCase().includes(needle));
}

class Feed {
  #items = [];

  constructor(items = []) {
    this.#items = [...items];
  }

  get pinned() {
    return this.#items.filter(({ pinned }) => pinned);
  }

  async refresh(fetchItems) {
    try {
      this.#items = await fetchItems();
    } catch (error) {
      console.error(`sync failed: ${error.message}`);
    }
    return this.#items.length;
  }
}

const feed = new Feed(ITEMS);
console.log(search(feed.pinned, 'android')); // [ { id: 1, ... } ]
console.log(0.1 + 0.2 === 0.3, 42, null, undefined);
