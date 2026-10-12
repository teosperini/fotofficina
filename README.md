# Fotofficina

A free, open-source photo library and non-destructive raw developer for macOS, Windows and Linux, written in Rust.
Everything runs on your own computer: no subscription, no account, no internet needed to develop your photos.

Fotofficina is a modified version of **[LightCraft](https://github.com/storytold/lightcraft)** by the ArtCraft team
and contributors, and it keeps their license (MIT OR Apache-2.0). It is **not** made, sponsored or endorsed by the
ArtCraft team, and it does not include their name, wordmark or logos.

## What is different from LightCraft

- Own name and icon; the ArtCraft branding, links and community buttons are removed.
- Works with [RawRefinery](https://github.com/rymuelle/RawRefinery)-style AI raw tools run locally *(in progress)*:
  denoise a photo's original raw from inside the app and get the result back in the library, stacked with the original.

Everything else is LightCraft. See its [README](https://github.com/storytold/lightcraft#readme) and
[docs](https://github.com/storytold/lightcraft/tree/main/docs) for the full feature list, supported cameras,
keyboard shortcuts and the control protocol / MCP server.

## Build and run

You need Rust (stable). From the repository root:

```sh
cargo run --release -p lightcraft
```

On macOS you can build the app bundle and a DMG with `packaging/macos/package.sh --arch aarch64`
(ad-hoc signed: on first launch use right-click → Open).

## Staying up to date with LightCraft

This repository tracks LightCraft: `main` mirrors upstream, and the changes of this project live on the
`teo/fotofficina` branch, which is rebased onto upstream regularly.

## License

Licensed under either of [Apache License 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT), at your option.
See [NOTICE](NOTICE) for the original copyright notices and the third-party material that is bundled.
