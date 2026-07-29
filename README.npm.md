# Weeb-3 - Browser-side Swarm client library

`weeb-3` is a browser-side Swarm client built in Rust and compiled to WebAssembly.
The main `weeb-3` project is the full released browser client, published at [lat-murmeldjur.github.io/weeb-3](https://lat-murmeldjur.github.io/weeb-3), where the client is used together with its own interface.

This npm package is the library edition of that same client. Projects can use the API directly or mount the bundled browser interface with `renderInterface(container)`.

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
- `BootstrapNode` for defining bootstrap peers
- `Weeb3` for lower-level direct access to the underlying client

The higher-level `Weeb3No103` interface provides the main methods used by the embedding example:

- `start(options?)`
- `connect()`
- `networkState()`
- `switchMainnet()` / `switch_mainnet()`
- `switchTestnet()` / `switch_testnet()`
- `switchNetwork(mode)` / `switch_network(mode)`
- `connectProfile(mode)` / `connect_profile(mode)`
- `retrieve(address)`
- `upload(file, encryption, index_string, add_to_feed, feed_topic)`
- `uploadWithRedundancy(file, encryption, redundancy_level, index_string, add_to_feed, feed_topic)`
- `postUploadBytesWithRedundancy(bytes, mime, filename, encryption, redundancy_level, add_to_feed, feed_topic)`
- `openStreamFeed(owner, topic)`
- `playHlsStream(owner, topic, media_type, index?)`
- `attachHlsStream(media, owner, topic, options)`
- `detachHlsStream()`
- `configureStreamingRoutes(service_worker_url, route_base)`
- `renderInterface(container)`
- `resetStamp()`
- `postPushChunk(data, soc, chunk_address, stamp)`

Sequence-feed indexes use Bee's fixed-width eight-byte big-endian encoding.

## HLS streaming

HLS playback is an optional dapp integration, not a Bee/Swarm standard. `playHlsStream(...)` displays a stream in the bundled interface after `renderInterface(container)`. For an application-owned `<video>` or `<audio>` element, call `attachHlsStream(media, owner, topic, { start: "beginning" })`; use `"current-window"` for a rolling/live presentation. Call `detachHlsStream()` before removing or replacing that element. The Service Worker must be copied from the package to a same-origin URL whose scope contains the page, then configured with `configureStreamingRoutes(...)`. Canonical mainnet links use `/stream/{owner}/{topic}[/{index}]`; testnet inserts `/testnet` before `/stream`.

## Basic usage

Call `init()` once before creating a client instance so the WebAssembly module is loaded.

```js
import init, { Weeb3No103, BootstrapNode } from "@lat-murmeldjur/weeb_3";

await init();

const weeb3node = new Weeb3No103();

// Start with the built-in mainnet profile and browser-dialable bootnodes.
weeb3node.start();
console.log(await weeb3node.networkState());

// Switch explicitly between built-in profiles.
await weeb3node.switchTestnet();
await weeb3node.switchMainnet();

// Or use the generic form. Accepted values include:
// "mainnet", "gnosis", "1", "testnet", "sepolia", and "10".
await weeb3node.switchNetwork("testnet");
await weeb3node.switchNetwork("mainnet");

// You can still start with explicit browser-dialable bootnodes and network id.
const BOOTSTRAP_NODES = [
  new BootstrapNode("/ip4/example/tcp/443/wss/p2p/examplePeerId", true),
];

weeb3node.start({
  networkId: "1",
  bootstrapNodes: BOOTSTRAP_NODES,
});

// Or start with the built-in testnet profile.
weeb3node.start({ testnet: true });
```

## Example corresponding to `example.html`

This is a compact npm-import form of the same usage pattern shown in the project's `example.html`:

```js
import init, { Weeb3No103, BootstrapNode } from "@lat-murmeldjur/weeb_3";

await init();

const weeb3node = new Weeb3No103();

await weeb3node.switchMainnet();

const entries = await weeb3node.retrieve(
  "695fceb3a8c212cd123e2e40d86ec08b52fe4fe6ca46687ce9ea69b8f05471f6aa25b5d4d41bf78b1db3479c048fd5fd8137ba844604821b71786196306b68e7"
);
```

## Erasure-coding selector

Legacy upload methods use Bee's Medium level. Explicit upload levels are `0` None, `1` Medium, `2` Strong, `3` Insane, and `4` Paranoid. The generated TypeScript declaration exposes this as `UploadRedundancyLevel = 0 | 1 | 2 | 3 | 4`.

`renderInterface(container)` includes a Medium-default erasure-coding dropdown. A custom interface can populate its own dropdown from the same canonical metadata:

```js
import init, {
  Weeb3No103,
  defaultUploadRedundancyLevel,
  uploadRedundancyOptions,
} from "@lat-murmeldjur/weeb_3";

await init();
const node = new Weeb3No103();
const choices = uploadRedundancyOptions();
const level = defaultUploadRedundancyLevel();

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
- Use one active `Weeb3No103` node and HLS session per loaded Wasm module.
- The package does not publish the standalone site HTML, but `renderInterface(container)` embeds the same interface shell—including its erasure-coding selector—from the Wasm bundle.
- The full released browser client remains available in the main project repository and on the project site.
