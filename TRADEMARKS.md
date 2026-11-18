# Name and branding

The source code of Tomi is licensed under the [Apache License 2.0](LICENSE). That license covers the code. As its
section 6 says, it does **not** grant permission to use the name "Tomi", the logo, the symbol or the other identifying
marks of the project. The name is not a registered trademark; the maintainers keep whatever rights they have in it.

## What you can do

- Use, modify and redistribute the code under the Apache License, including commercially.
- Build the project yourself and run it, for your own use, under its official name and identifier.
- Say truthfully that your work is *based on* or *compatible with* Tomi, for example "a fork of Tomi" or
  "works with the Tomi protocol".
- Distribute the unmodified binaries published in the official releases of this repository under their official name.

## What you must change if you distribute your own build

If you publish a modified or independently built version to other people (a store, a website, a package repository):

1. **Use another name and another icon.** Do not reuse "Tomi", the logo or the sun-and-night symbol, so that nobody mistakes
   your build for the official one. The identity colors and symbol are described in
   [`design/tomi/brand-guidelines.md`](design/tomi/brand-guidelines.md).
2. **Use your own application identifier.** The official one, `eu.shaffe.tomi` (and the App Group, URL scheme and
   background task identifiers derived from it), belongs to the official build. See [`docs/FORKING.md`](docs/FORKING.md)
   for the exact places to change.
3. **Sign with your own keys** and publish under your own developer accounts.
4. **Do not suggest an endorsement**, and do not present your relay as the official one.

Interoperability is fine: a fork that keeps the protocol and the frozen format can talk to official clients and relays.

## Questions

Open an issue if you are unsure whether a use of the name is acceptable. Reports of confusing or misleading uses are
welcome through the same channel.
