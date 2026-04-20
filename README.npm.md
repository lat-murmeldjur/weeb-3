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

const BOOTSTRAP_NODES = [
  new BootstrapNode(
    "/ip4/167.235.96.31/tcp/32535/tls/sni/167-235-96-31.k2k4r8n9x80nshvozftjmg4klymgjtdflwxiovfx63yc6917dlrteva4.libp2p.direct/ws/p2p/QmYkyg5ZU3DzxhqfGyLYLVbk9DMdBagxe9q1AmHKNgt8ps",
    true
  ),
];

weeb3node.start(BOOTSTRAP_NODES, "10");
```

## Example corresponding to `example.html`

This is the npm-import form of the same usage pattern shown in the project's `example.html`:

```js
import init, { Weeb3No103, BootstrapNode } from "@lat-murmeldjur/weeb_3";

await init();

const weeb3node = new Weeb3No103();

const BOOTSTRAP_NODES = [
  new BootstrapNode(
    "/ip4/167.235.96.31/tcp/32535/tls/sni/167-235-96-31.k2k4r8n9x80nshvozftjmg4klymgjtdflwxiovfx63yc6917dlrteva4.libp2p.direct/ws/p2p/QmYkyg5ZU3DzxhqfGyLYLVbk9DMdBagxe9q1AmHKNgt8ps",
    true
  ),
  new BootstrapNode(
    "/ip4/49.12.172.37/tcp/32530/tls/sni/49-12-172-37.k2k4r8kibjadgpqco81quegou963p7lbcd9ti0bw8lrcc95ystm6by9d.libp2p.direct/ws/p2p/QmRHeoLCHjHoMur8PQpuV8acNJMmKPT61c3ZMLpTqY7og4",
    true
  ),
  new BootstrapNode(
    "/ip4/49.12.172.37/tcp/32533/tls/sni/49-12-172-37.k2k4r8pnvqpufzwaf4ic1o1fo0onfh4p9b37gp0rdxzdte2kcd7ewp4w.libp2p.direct/ws/p2p/QmfCwr7FVxbYz1GPQ2NN2r5iduXSQDLefqzkBAB9JfZYgF",
    true
  ),
  new BootstrapNode(
    "/ip4/167.235.96.31/tcp/32536/tls/sni/167-235-96-31.k2k4r8omeryzle2ywg941xs6vgwlq4cr0b3qe83ub7rn9n8ysmcwfqru.libp2p.direct/ws/p2p/QmcPvejw1r1BQ6aUuK6Y18mcLAcYyg9iEmiD1TRpyaox7s",
    true
  ),
  new BootstrapNode(
    "/ip4/49.12.172.37/tcp/32531/tls/sni/49-12-172-37.k2k4r8l8l5hzyp48440rjqlfdjpr03jfgioal93akbigy0tomtft4w44.libp2p.direct/ws/p2p/QmTFvqc5wMkbsXjqnTxQbVss5t8T1292BupJZ9VyU1GMRV",
    true
  ),
  new BootstrapNode(
    "/ip4/49.12.172.37/tcp/32532/tls/sni/49-12-172-37.k2k4r8pr3m3aug5nudg2y039qfj2gxw6wnlx0e0ghzxufcn38soyp9z4.libp2p.direct/ws/p2p/QmfSx1ujzboapD5h2CiqTJqUy46FeTDwXBszB3XUCfKEEj",
    true
  ),
  new BootstrapNode(
    "/ip4/167.235.96.31/tcp/32537/tls/sni/167-235-96-31.k2k4r8m6hc1wyzz789uubmz6cxmeuquzfi5b06zdh4l7e5ve199oay7j.libp2p.direct/ws/p2p/QmVoPN964YuoGpqc6BGJpLGmUn2goaqRm5vkCi5e7H9w98",
    true
  ),
  new BootstrapNode(
    "/ip4/167.235.96.31/tcp/32538/tls/sni/167-235-96-31.k2k4r8nqwaetj1eljpu4qzeebnnujzu997pdh1i1ia2dcpcjjv9gc1s0.libp2p.direct/ws/p2p/Qma2pmuYLCzcmsFHHLyWRPxxt7eN9MKqhnJaShKomn2zEK",
    true
  ),
];

weeb3node.start(BOOTSTRAP_NODES, "10");

const entries = await weeb3node.retrieve(
  "695fceb3a8c212cd123e2e40d86ec08b52fe4fe6ca46687ce9ea69b8f05471f6aa25b5d4d41bf78b1db3479c048fd5fd8137ba844604821b71786196306b68e7"
);
```

## Notes

- This package is meant for browser applications, not a plain Node.js runtime.
- It does not include the full `weeb-3` interface. It exposes the client so you can build your own interface around it.
- The full released browser client remains available in the main project repository and on the project site.
