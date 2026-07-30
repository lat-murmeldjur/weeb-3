# Weeb-3 - Browser-side Swarm client library

`weeb-3` is a browser-side Swarm client built in Rust and compiled to WebAssembly.
The main `weeb-3` project is the full released browser client, published at [lat-murmeldjur.github.io/weeb-3](https://lat-murmeldjur.github.io/weeb-3), where the client is used together with its own interface.

This npm package is the library edition of that same client. Projects can use the API directly, attach Swarm HLS playback to their own media element, or mount the bundled browser interface with `renderInterface(container)`.

Project repository: [github.com/lat-murmeldjur/weeb-3](https://github.com/lat-murmeldjur/weeb-3)

Project site: [lat-murmeldjur.github.io/weeb-3](https://lat-murmeldjur.github.io/weeb-3)

## Installation

```shell
npm install @lat-murmeldjur/weeb_3
```

## What this package contains

This package contains the browser-targeted WebAssembly build of the `weeb-3` client together with the JavaScript wrapper needed to initialize and use it from an application.

The main exports are:

- `Weeb3No103` as the higher-level client interface

The higher-level `Weeb3No103` interface provides the main methods used by the embedding example:

- `start(options?)`
- `networkState()`
- `switchNetwork(mode)`
- `retrieve(address)`
- `upload(file, encryption, index_string, add_to_feed, feed_topic)`
- `uploadWithRedundancy(file, encryption, redundancy_level, index_string, add_to_feed, feed_topic)`
- `postUploadBytesWithRedundancy(bytes, mime, filename, encryption, redundancy_level, add_to_feed, feed_topic)`
- `attachStream(media, owner, topic, start)`
- `renderInterface(container)`
- `resetStamp()`
- `postPushChunk(data, soc, chunk_address, stamp)`

Sequence-feed indexes use Bee's fixed-width eight-byte big-endian encoding.

## Basic usage

Call `init()` once before creating a client instance so the WebAssembly module is loaded.

```js
import init, { Weeb3No103 } from "@lat-murmeldjur/weeb_3";

await init();

const weeb3node = new Weeb3No103();

// Start with the built-in mainnet profile and browser-dialable bootnodes.
weeb3node.start();
console.log(await weeb3node.networkState());

// Switch explicitly between built-in profiles.
await weeb3node.switchNetwork("testnet");
await weeb3node.switchNetwork("mainnet");

// You can still start with explicit browser-dialable bootnodes and network id.
const BOOTSTRAP_NODES = [
  { multiaddr: "/ip4/example/tcp/443/wss/p2p/examplePeerId", usable: true },
];

weeb3node.start({
  networkId: "1",
  bootstrapNodes: BOOTSTRAP_NODES,
});

// Or start with the built-in testnet profile.
weeb3node.start({ testnet: true });
```

## Stream playback

The application creates and owns the media element. `"beginning"` reconstructs the earliest available feed timeline, while `"live"` opens the current authenticated feed head and starts at the player's safe live-sync position. Calling `attachStream` again switches the same element between them.

```js
import init, { Weeb3No103 } from "@lat-murmeldjur/weeb_3";

await init();

const node = new Weeb3No103();
const video = document.querySelector("video");
const owner = "6F2728386F8a47ef5EBe323721188e630Ff0FdE9";
const topic = "b347b89b-933c-424f-a3d1-403bdd270b25";

node.start();
await node.attachStream(video, owner, topic, "beginning");

// Later, for example from the application's own Live button:
await node.attachStream(video, owner, topic, "live");
```

Public HLS feeds use mainnet. The embedding page must be below the `/weeb-3/` scope and serve the packaged worker at `/weeb-3/service.js`, or already be controlled by a worker implementing the same forwarding protocol.

## Example corresponding to `example.html`

This is a compact npm-import form of the same usage pattern shown in the project's `example.html`:

```js
import init, { Weeb3No103 } from "@lat-murmeldjur/weeb_3";

await init();

const weeb3node = new Weeb3No103();

await weeb3node.switchNetwork("mainnet");

const entries = await weeb3node.retrieve(
  "695fceb3a8c212cd123e2e40d86ec08b52fe4fe6ca46687ce9ea69b8f05471f6aa25b5d4d41bf78b1db3479c048fd5fd8137ba844604821b71786196306b68e7"
);
```

## Upload API

Legacy upload methods use Bee's Medium level. Explicit upload levels are `0` None, `1` Medium, `2` Strong, `3` Insane, and `4` Paranoid. The generated TypeScript declaration exposes this as `UploadRedundancyLevel = 0 | 1 | 2 | 3 | 4`.

`renderInterface(container)` includes a Medium-default erasure-coding dropdown. A custom interface can pass the selected numeric level directly:

```js
import init, { Weeb3No103 } from "@lat-murmeldjur/weeb_3";

await init();
const node = new Weeb3No103();
const level = 1;

const result = await node.uploadWithRedundancy(
  file,
  true,
  level,
  "",
  false,
  "",
);
```

Retrieval reads the level encoded in the Swarm tree and uses parity when data shards are unavailable.

## Notes

- This package is meant for browser applications, not a plain Node.js runtime.
- `attachStream` uses the same runtime, connection buildup, retrieval, prefetch, accounting, hls.js or native-HLS player, and recovery logic as the built-in interface.
- The package does not publish the standalone site HTML, but `renderInterface(container)` embeds the same interface shell—including its erasure-coding selector—from the Wasm bundle.
- The full released browser client remains available in the main project repository and on the project site.
