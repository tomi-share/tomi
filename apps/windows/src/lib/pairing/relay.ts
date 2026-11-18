// The "Relais" field of the pairing and creation forms (spec/pairing-v1.md §4.4, §10).
//
// A relay read from a link or typed in a field is NOT trusted by itself: whoever shows the QR code
// chooses it. The forms show it in full, the verification screen shows it again next to the code,
// and the core stores nothing before the user confirmed. Only the shape is settled here (a scheme
// is added when the user typed a bare host); the core judges the address.

/** The relay to send, `https://host[:port]`, or `null` when the field is empty. */
export function relayUrlOf(input: string): string | null {
  const value = input.trim();
  if (value === "") return null;
  return value.includes("://") ? value : `https://${value}`;
}

/** What the forms show of a relay: the authority, in full (`relay.example.org:8443`). */
export function relayShown(input: string): string {
  return input.trim().replace(/^https:\/\//i, "").replace(/\/+$/, "");
}
