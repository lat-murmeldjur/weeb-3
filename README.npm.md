# Weeb-3 - Browser-side Swarm client library

`weeb-3` is a browser-side Swarm client built in Rust and compiled to WebAssembly.
The main `weeb-3` project is the full released browser client, published at [lat-murmeldjur.github.io/weeb-3](https://lat-murmeldjur.github.io/weeb-3), where the client is used together with its own interface.

This npm package is the library edition of that same client.
It is for projects that want to use the `weeb-3` Swarm client in their own browser application without using the full `weeb-3` site and interface.

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

- `start(bootstrap_nodes, network_id)`
- `connect()`
- `networkState()`
- `switchMainnet()` / `switch_mainnet()`
- `switchTestnet()` / `switch_testnet()`
- `switchNetwork(mode)` / `switch_network(mode)`
- `connectProfile(mode)` / `connect_profile(mode)`
- `retrieve(address)`
- `upload(file, encryption, index_string, add_to_feed, feed_topic)`
- `resetStamp()`
- `postPushChunk(data, soc, chunk_address, stamp)`

## Basic usage

Call `init()` once before creating a client instance so the WebAssembly module is loaded.

```js
import init, { Weeb3No103, BootstrapNode } from "@lat-murmeldjur/weeb_3";

await init();

const weeb3node = new Weeb3No103();

// Connect with the built-in mainnet profile and browser-dialable bootnodes.
await weeb3node.connect();
console.log(await weeb3node.networkState());

// Switch explicitly between built-in profiles.
await weeb3node.switchTestnet();
await weeb3node.switchMainnet();

// Or use the generic form. Accepted values include:
// "mainnet", "gnosis", "1", "testnet", "sepolia", and "10".
await weeb3node.switchNetwork("testnet");
await weeb3node.switchNetwork("mainnet");

// You can still start with explicit browser-dialable bootnodes.
const BOOTSTRAP_NODES = [
  new BootstrapNode("/ip4/example/tcp/443/wss/p2p/examplePeerId", true),
];

weeb3node.start(BOOTSTRAP_NODES, "1");
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

## Notes

- This package is meant for browser applications, not a plain Node.js runtime.
- It does not include the full `weeb-3` interface. It exposes the client so you can build your own interface around it.
- The full released browser client remains available in the main project repository and on the project site.
