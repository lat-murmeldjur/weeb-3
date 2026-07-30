# Weeb-3 - A Swarm client for browsers

This project is a work-in-progress Swarm client implementation that relies solely on browser-side technologies. It uses [wasm-pack](https://rustwasm.github.io/wasm-pack/) to build the Rust client to WebAssembly and runs the Swarm networking, retrieval, upload, persistence, service-worker integration, and UI logic inside the browser.

The codebase is still experimental. APIs, persistence formats, supported networks, and browser behavior may change while the implementation is being hardened.

## Building the code

Ensure you have [wasm-pack](https://rustwasm.github.io/wasm-pack/), [protoc](https://grpc.io/docs/protoc-installation/), and [clang](https://clang.llvm.org/) installed.

1. Build the client library:

    ```bash
    RUSTFLAGS='--cfg getrandom_backend="wasm_js"' wasm-pack build --target web --out-dir static --out-name weeb_3
    ```

2. Start the local server to serve the HTML, JavaScript, and Wasm files:

    ```bash
    cargo run
    ```

    The local server uses an insecure self-signed certificate to provide HTTPS. This is enough for loading the local application in many development flows, but it is not necessarily sufficient for enabling Service Workers in browsers such as Chrome. Single-file Swarm resources can still be displayed without the Service Worker, but rendering full Swarm websites requires a Service Worker and therefore a certificate that the browser treats as trusted.

    For a trusted deployment, one option is to serve the static build from GitHub Pages or another HTTPS host with a browser-trusted certificate. A simple workflow is to fork the repository, enable GitHub Pages for the `docs` folder, and copy the latest files from `static` to `docs` after building.

    `Code_One.hx` automates that synchronization after successful Wasm and native builds. Files in `static` are authoritative; the script copies the HTML examples, including `hls-stream-example.html`, and generated runtime assets into `docs`.

3. Open the application URL, for example [`https://localhost:8080/weeb-3/`](https://localhost:8080/weeb-3/), or the GitHub Pages hosted version at [`https://lat-murmeldjur.github.io/weeb-3`](https://lat-murmeldjur.github.io/weeb-3).

## Using the npm package

The `wasm-pack` build generates the browser package files, and the publishing workflow finalizes `static/package.json` so every wrapper asset is included:

- `static/snippets/**`
- `static/service.js`
- `static/weeb_3.js`
- `static/weeb_3_bg.wasm`
- `static/weeb_3.d.ts`
- `static/weeb_3_bg.wasm.d.ts`

After publishing, the package can be used with the same API shape as the examples in `static/example.html`, `static/issue-1-json-sync-example.html`, and `static/hls-stream-example.html`:

```js
import init, { Weeb3No103 } from "@lat-murmeldjur/weeb_3";

await init();

const weeb3node = new Weeb3No103();

// Use the built-in mainnet profile and browser-dialable bootnodes.
weeb3node.start();
console.log(await weeb3node.networkState());

// Switch explicitly between the built-in profiles.
await weeb3node.switchNetwork("testnet");
await weeb3node.switchNetwork("mainnet");

// Or start with explicit browser-dialable bootnodes.
weeb3node.start({
  networkId: "1",
  bootstrapNodes: [
    { multiaddr: "/ip4/example/tcp/443/wss/p2p/examplePeerId", usable: true },
  ],
});

// Or start with the built-in testnet profile.
weeb3node.start({ testnet: true });

const ready = await weeb3node.ready(1, 20_000);
```

The wrapper exposes the browser node as `Weeb3No103`. It can start the runtime, switch between mainnet and testnet with `switchNetwork(mode)`, render the bundled interface into a container, attach a Swarm HLS stream to an application-owned media element, report network and progress state, retrieve BZZ resources, retrieve raw bytes or chunks, upload `File` objects or byte arrays, and publish or read feed updates.

The HLS example creates its own `<video>` and passes it to `attachStream(media, owner, topic, start)`, where `start` is `"beginning"` or `"live"`. The method uses the same runtime boot, connection buildup, Service Worker setup, retrieval, prefetch, player and accounting paths as the standalone application without mounting the rest of its interface.

The publishing workflow defaults to the GitHub repository owner scope. If a different npm scope is needed, set the `NPM_SCOPE` repository variable in GitHub Actions before pushing to `main`.

## [Notes]

### Compatibility - Supported Browsers

- Chrome (on Windows 11)
- Chrome (Android)
- Brave (on Windows 11)
- Edge
- Firefox (on Windows 11)
- Firefox (on Android)

Testing and improving support for other browsers is planned.

### How it works (architectural overview)

The weeb-3 client consists of several logical components:

- The native development server in `src/main.rs`, which serves the embedded browser distribution over local TLS and provides application-shell routes.
- The browser interface, implemented primarily by `static/index.html`, `src/interface.rs`, `src/interface_conventions.rs`, and `src/interface_runtime_conventions.rs`.
- The libp2p / Swarm node, whose main entry point is `src/lib.rs`.
- The Swarm protocol handlers and data pipelines for handshake, peer discovery, pricing, accounting, retrieval, pushsync, pseudosettle, swap, manifests, feeds, erasure-coded uploads and recovery, and generic resource streaming.
- The separate HLS dapp integration in `src/stream_hls.rs`, which follows Swarm feeds and presents their manifests and segments to browser media playback. HLS itself is not part of the Bee / Swarm protocol.
- The Service Worker in `static/service.js`, which provides deterministic browser routes for Swarm content and forwards canonical requests into the Rust runtime.
- The npm / library facade in `src/library.rs`, which wraps the same runtime for embedding in other browser applications.
- Browser persistence, secure local state, network profiles, and on-chain integration implemented by `src/persistence.rs`, `src/secure_vault.rs`, `src/network_profile.rs`, and `src/on_chain.rs`.

Below is a piece-by-piece overview of the current component logic.

#### The interface

The default browser application is instantiated by `static/index.html`, which loads the generated Wasm module and calls `interweeb` from `src/interface.rs`.

`interweeb` first converts the GitHub Pages `404.html` `#/` handoff into its canonical path, then creates a `Weeb3` node, selects the network from that route, and delegates the rest of the UI setup to `mount_interface`. `mount_interface` can either start the runtime itself or attach the interface to a runtime that has already been started by the package wrapper.

The interface layer currently has the following roles:

- Starting the libp2p / Swarm runtime in an async browser task when requested.
- Installing UI conventions and rendering the interface shell.
- Preloading the secure vault module before sensitive upload, feed, stamp, or cheque operations are requested.
- Registering the Service Worker and routing Service Worker messages back to the Rust runtime.
- Reading the configured network profile, network id, and browser-dialable bootnodes, then passing bootnode connection requests to the `Weeb3` node.
- Wiring the navigation input so BZZ references, raw byte routes, chunk routes, and exact `/stream/{owner}/{topic}` or `/live/stream/{owner}/{topic}` HLS share routes can be opened from the UI.
- Wiring upload controls for single files, tar-based collections, optional encryption, Bee redundancy levels, index document selection, optional feed publishing, and postage-stamp reuse or reset.
- Wiring on-chain controls for upload prerequisites, postage batch acquisition, chequebook deployment, cheque signer persistence, and chequebook deposits through the browser wallet.
- Providing runtime controls such as pausing and resuming transfers.
- Rendering retrieved resources, website iframes, streaming media, raw downloads, logs, connection status, network state, and progress rows.

The current interface no longer assumes that requests are handled by a shared worker. The tab owns the `Weeb3` runtime, while the Service Worker acts as a request forwarder between browser fetch events and the active controlled client.

#### The weeb process

The main Swarm client is implemented in `src/lib.rs`. It is compiled only for the `wasm32` target and is designed to run in the browser event loop.

At a high level, `src/lib.rs` does the following:

- Imports the generated protobuf modules for the Swarm protocols the client implements.
- Defines the Swarm protocol names used by the client, including handshake, pricing, hive peer discovery, pseudosettle, retrieval, pushsync, and swap.
- Defines network mode helpers for testnet and mainnet. The built-in profiles currently map Swarm network id `10` to the Sepolia-based testnet profile and Swarm network id `1` to the Gnosis / xDAI mainnet profile.
- Defines the `Weeb3` client, which owns the libp2p `Swarm`, runtime channels, connection state, network id, progress store, transfer pause flag, and peer registry.
- Defines `Wings`, the in-memory peer and accounting registry used to track connected peers, overlay addresses, bootnodes, accounting peers, settlement state, known underlays, and self-observed ephemeral addresses.
- Exposes the runtime functions used by the interface and library wrapper.

The internal `Weeb3` operations used by the interface and npm facade are:

1. Changing the network id and connecting browser-dialable bootnodes.
2. Disconnecting and clearing peer state when the active network profile changes.
3. Uploading a `File` or tar collection, optionally encrypted, with a selected Bee redundancy level, optionally with an index document, and optionally as a feed update.
4. Pushing a raw chunk through pushsync.
5. Resolving and acquiring BZZ resources.
6. Retrieving raw bytes or individual chunks.
7. Reading feed envelopes and feed content with Bee-compatible big-endian sequence indexes.
8. Resetting the active postage stamp state.
9. Reporting logs, connection counts, active network id, and progress snapshots.
10. Pausing or resuming transfers.
11. Running the asynchronous protocol loop.

The `new` function constructs the browser node. In the current implementation it:

- Generates a fresh libp2p identity key for the browser runtime.
- Builds a libp2p `Swarm` with the browser WebSocket / WebSys transport.
- Uses authenticated Noise and Yamux multiplexing for libp2p connections.
- Enables the stream behavior used by the Swarm protocol handlers.
- Creates the peer registry, connection registry, progress store, transfer control flag, and runtime channels.
- Initializes the default Swarm network id to `1` (mainnet).

The `run` function is the long-running runtime loop. It builds a channel-based asynchronous task graph for the browser runtime and coordinates the major subsystems:

- Peer discovery, bootnode dialing, connection retry, and connection cleanup.
- Incoming and outgoing libp2p stream handling.
- Handshake, identify, pricing, and peer promotion.
- Accounting, pseudosettle refreshes, cheque sending, and swap-related settlement messages.
- High-level BZZ resolution and resolved BZZ range retrieval.
- Data-level retrieval and upload requests.
- Chunk-level retrieval and pushsync with bounded concurrency.
- Upload progress reporting.
- Transfer pause and cancellation checks.
- Log forwarding to the interface.

The runtime is heavily asynchronous, but it is still running inside the browser's Wasm execution environment. It uses `spawn_local`, async channels, short queue polling, and protocol-specific retry delays rather than OS threads.

#### Source file map

Every Rust source file belongs to one of the runtime areas below. None of the `.rs` files are generated. `build.rs` compiles the active protocol schemas in `src/etiquette_0.proto` through `src/etiquette_8.proto`, except for the retained but unused `src/etiquette_3.proto`, and `src/lib.rs` includes the resulting active modules.

##### Entrypoints and browser integration

- `src/main.rs` is the native development executable. It binds the HTTPS server on all local interfaces, embeds the browser runtime assets and repository HTML examples, serves the HLS npm example at `/weeb-3/hls-stream-example.html`, and returns the application shell for the exact mainnet beginning and live stream share routes.
- `src/lib.rs` is the Wasm crate root and low-level node runtime. It defines protocol names and generated protobuf modules, owns `Weeb3`, `Wings`, libp2p state, physical connection sessions, channels and workers, and runs the asynchronous protocol event loop. Its stream behavior rejects implicit dialing so protocol streams remain attached to explicitly tracked connections.
- `src/library.rs` is the wasm-bindgen / npm facade over the same `Weeb3` runtime used by the built-in interface. It exposes lifecycle, network, retrieval, upload, feed, HLS media attachment, progress, postage, and wallet methods as `Weeb3No103` and converts Rust results into JavaScript values.
- `src/interface.rs` mounts and orchestrates the built-in browser interface. It starts or attaches to the shared runtime, installs the Service Worker message bridge, wires resource, upload, feed, network, wallet and pause controls, and renders results, logs, connection state, and progress.
- `src/interface_conventions.rs` embeds the static interface shell and contains its DOM construction, styles, labels, collapsible sections, and theme behavior.
- `src/interface_runtime_conventions.rs` contains the operational UI-to-runtime bridge: bootnode and network application, wallet and upload prerequisites, resource views and downloads, progress rendering, and Service Worker registration, control negotiation, and request dispatch.
- `src/events.rs` implements the bounded, revisioned progress store. Upload and retrieval tasks write progress rows there, while both the built-in interface and npm facade read consistent snapshots.
- `src/nav.rs` restores the GitHub Pages `#/` handoff to a canonical `/weeb-3/` path and parses exact locations and user input into network-aware BZZ, bytes, chunk, beginning-HLS, or live-HLS share routes. Unqualified routes default to mainnet, and non-Bee aliases are not retained.

##### Connections and Swarm protocols

- `src/accounting.rs` contains connection-scoped debt, reserve, credit and refreshment accounting, proximity pricing, the 200-peer buildup and dial-concurrency limits, and Bee-compatible reconnect delay calculations.
- `src/addresses.rs` decodes Bee underlay lists, validates their WSS address forms, and converts Bee IP/SNI addresses into browser-dialable DNS WebSocket multiaddresses.
- `src/handlers.rs` implements framing and stream exchanges for handshake, pricing, hive gossip, pseudosettle refreshment, SWAP cheques, retrieval, and pushsync. It binds protocol work to physical connection sessions and reports completion to accounting and the data pipelines.
- `src/conventions.rs` contains shared Swarm protocol primitives, including peer and accounting records, proximity order, BMT / CAC addressing, CAC and SOC validation, cryptographic signing and recovery, handshake helpers, and common reference and resource encodings.

##### Retrieval, manifests, and feeds

- `src/retrieval.rs` is the central download engine. It performs priced peer selection, accounting-safe individual chunk retrieval, validation and decryption, replica fallback, decoded caching, concurrent Bee tree and range joins, erasure-shard acquisition and Reed-Solomon recovery, complete-resource retrieval, and feed-update acquisition.
- `src/retrieval_conventions.rs` provides wrap-safe request-generation ordering, bounded retrieval admission, cancellation of work that has not been sent, and keyed singleflight coordination. Once a network request has been dispatched, observers may detach but the request and its accounting completion are allowed to drain instead of being replayed or abandoned.
- `src/bzz_stream.rs` parses canonical BZZ resources and resolves Mantaray paths, embedded values, and feed indirection into metadata and byte-range targets. It provides lazy range resolution for `src/stream.rs` using the same erasure-capable retrieval join used for complete resources.
- `src/manifest.rs` contains the shared Mantaray wire-format and collection-reading logic: headers and forks, prefix ordering, metadata sizing, encrypted or obfuscated header handling, bounded visit and ancestry guards, and ordered resource joining.
- `src/manifest_upload.rs` constructs and uploads Bee-compatible Mantaray nodes, forks, stubs, collection entries, index and error documents, and encrypted or obfuscated manifests. It delegates protected chunk-tree creation to `src/upload.rs`.
- `src/feed.rs` encodes Bee sequence indexes as fixed-width eight-byte big-endian values, derives feed IDs and update addresses, converts JavaScript indexes without loss, and performs bounded concurrent probing for the latest sequence frontier. Feed reads share retrieval's rule that observer timeouts do not cancel already dispatched accounting work.
- `src/ens.rs` resolves ENS names through the registry and resolver contracts and converts supported content hashes into Swarm references.

##### Upload path

- `src/upload.rs` reads browser files in bounded slices, splits and optionally encrypts resources, builds Bee chunk trees, generates erasure parity and root replicas, stamps chunks, and pushes them with bounded concurrency and accounting. It also handles raw resources, manifests, SOCs, feed updates, receipts, and progress without changing the individual chunk push protocol.
- `src/erasure_coding.rs` implements Bee's redundancy levels and span flags, per-level shard and parity schedules, protected reference layouts and counts, root replicas, cached GF(256) coding matrices, parity generation, and selective Reed-Solomon reconstruction. It also keeps the small upload-level validation, resource-bundle encoding, and bounded file-slice helpers beside the upload coding implementation. Upload writes Bee-compatible layouts and retrieval reads their encoded level, using parity when data shards are unavailable.

##### Streaming and HTTP integration

- `src/stream.rs` is the generic browser resource layer. It translates Service Worker messages into BZZ and raw responses, implements HTTP Range, ETag, metadata, response and singleflight caches, and drives ordinary audio or video range retrieval, seek handling, staged prefetch, retries, and result-view lifetime.
- `src/stream_conventions.rs` contains exact beginning and live share-route validation, their HLS start value, HTTP range and validator helpers, immutable cache identities, device-aware cache limits, and staged regular-media lookahead budgets shared at the browser boundary.
- `src/stream_hls.rs` contains the separate, nonstandard HLS dapp integration rather than generic Bee retrieval logic. It recognizes and rewrites playlists, reconstructs sequence-zero playback for beginning mode, goes directly to the current authenticated feed head for live mode, stabilizes archive and live timelines, maintains segment caches and singleflight requests, plans startup and sustained lookahead, serves internal feed and segment responses, and owns the Rust player lifecycle and recovery logic. It begins the single hls.js module load while Service Worker control is being established so these independent cold-start operations do not run serially. Only the minimal dynamic import hook remains in `static/hls_loader.js`; stream control and prefetching stay in Rust.

##### Persistence, identity, networks, and contracts

- `src/persistence.rs` provides the IndexedDB primitives used for the chequebook signer and address and last issued payouts.
- `src/secure_vault.rs` communicates with the separately hosted secure vault and authorization popup for network-scoped postage state, chunk stamping, feed ownership and signed updates, and cheque signer material. It also handles vault reconnect and resume behavior without exposing sensitive state to ordinary UI code.
- `src/network_profile.rs` defines the built-in mainnet and testnet Swarm IDs, wallet chains, base currencies, token symbols, bootnodes, active-profile switching, and browser-dialable underlay checks.
- `src/on_chain.rs` implements injected-wallet and contract operations for postage batches, token approval, price-oracle reads, chequebook deployment and deposits, swap-token balances, and EIP-712 cheque signing. It embeds the ABI definitions in `src/*.json`.

The protobuf schema files map directly to Bee stream protocols:

- `src/etiquette_0.proto` defines the common protocol header envelope.
- `src/etiquette_1.proto` defines handshake messages and signed BZZ addresses.
- `src/etiquette_2.proto` defines Hive peer-gossip messages.
- `src/etiquette_3.proto` retains the original Ping/Pong schema for reference but is not compiled or wired into the runtime.
- `src/etiquette_4.proto` defines pricing threshold announcements.
- `src/etiquette_5.proto` defines pseudosettle payments and acknowledgements.
- `src/etiquette_6.proto` defines retrieval requests and deliveries.
- `src/etiquette_7.proto` defines pushsync deliveries and receipts.
- `src/etiquette_8.proto` defines SWAP handshakes and cheque messages.

Batch state held by `weeb-3-secure` is requested with the active Swarm network id, so testnet and mainnet use separate batch owners, batch ids, bucket counters, and temporary authorization.

#### Persistence and identity

The browser runtime maintains a mix of ephemeral and persistent state.

The libp2p identity used by a `Weeb3` runtime is generated when the node is created. That makes the live peer identity tab-local and runtime-local. Peer maps, connection attempts, active streams, and accounting state are kept in memory by the `Weeb3` and `Wings` structures.

`src/persistence.rs` stores the chequebook signer and address and last issued payouts across browser sessions. Network-scoped postage state, upload and feed identities, and other sensitive state are routed through the secure vault module instead of being handled directly by ordinary UI code.

Wallet access is requested only for on-chain operations. The browser wallet is used for chain switching, account access, postage purchase flows, chequebook deployment, and deposits. Upload/feed identities and cheque signer keys are managed separately from the wallet account so that Swarm protocol operations do not require signing every action with the injected wallet.

When the network profile changes, the runtime clears the current peer state and increments its connection generation so stale dialing, handshake, and connection events do not leak into the new network session.

### The Service Worker

The Service Worker in `static/service.js` sits between the browser fetch layer and the active weeb-3 page. Its role has expanded beyond the original static cache approach.

Single files can still be displayed without a Service Worker by creating `Blob` object URLs with the correct MIME type. This is enough for images, documents, and other standalone files. It is not enough for full websites, because browser-generated object URLs contain random identifiers and therefore cannot reliably satisfy relative paths for scripts, stylesheets, images, and other website assets.

The Service Worker solves this by providing deterministic application-scoped routes:

- Top-level navigation to `/weeb-3/stream/{owner}/{topic}` loads the mainnet HLS feed from its earliest available update. `/weeb-3/live/stream/{owner}/{topic}` loads its current authenticated feed head and lets the player select the safe live-sync point. The path values are a 20-byte feed owner and a URL-encoded feed topic.
- `GET` and `HEAD` requests below `/bzz/<reference>/<path>` are interpreted as canonical mainnet BZZ resource requests.
- Testnet can be selected from routes with `/testnet`, for example `/weeb-3/testnet` to boot the interface in testnet mode or `/weeb-3/testnet/bzz/<reference>/<path>` for a testnet BZZ link.
- Raw byte and chunk routes below `/bytes/` and `/chunks/` are forwarded to the Rust runtime.
- HLS playback uses `/feeds/<owner>/<topic-hash>` and `/hls/bytes/<reference>` internally for rewritten playlists and segment fetches. These are transport paths between the media loader, Service Worker, and Rust runtime, not additional share-link APIs.
- `POST` requests to the scoped `/bzz` endpoint are forwarded as upload requests, including upload headers such as encryption, collection, and index-document hints.
- Ordinary fetch forwarding selects a top-level client through a fresh, network-aware runtime probe, with concurrent probes for the same client coalesced. Direct HLS feed and segment requests from an in-scope top-level client use that client immediately to avoid repeating the probe on the playback path.
- HLS feed and segment requests are singleflighted by client, method, path, and range. A Service Worker response timeout detaches its `MessagePort` without replaying the request; while the page runtime remains alive, already dispatched Rust work can continue and settle its peer response and accounting once.
- BZZ resources can be answered as full responses, byte-range responses, or streaming responses depending on MIME type, request headers, and resource size.
- Requests outside the explicit weeb-3 route set remain under the host application's normal fetch and cache policy; the packaged worker does not precache or delete host assets.

This design means that rendered Swarm websites can request their own relative assets through ordinary browser fetch/navigation behavior, while the active Rust runtime resolves and retrieves the underlying Swarm data.

The Service Worker is security-sensitive. Browsers only enable it for secure origins, and a trusted certificate is required for normal deployment. Because a Service Worker can intercept requests for its scope, production deployments should treat Service Worker replacement, injected pages, and malicious Swarm-hosted websites as important security boundaries. The current architecture reduces some risk by rendering Swarm websites in iframes and by keeping sensitive state behind the secure vault layer, but security hardening remains an active development area.

### Main dependencies

The weeb-3 project uses the following main Rust crates and browser bindings:

- `libp2p` and `libp2p-stream` for peer identity, transport, multiplexing, stream protocols, identify, ping, and browser WebSocket transport.
- `async-std` and `async-lock` for async runtime primitives that work in the browser Wasm target.
- `wasm-bindgen`, `wasm-bindgen-futures`, `js-sys`, and `web-sys` for JavaScript, DOM, Service Worker, browser API, and Promise integration.
- `web3`, `alloy`, and `ethers` for wallet, signing, ABI, ENS, and on-chain contract interaction.
- `indexed_db_futures` for browser IndexedDB persistence.
- `tar` and `mime_guess` for collection upload handling and MIME inference.
- `getrandom` with the `wasm_js` backend for browser-compatible randomness.
- `base64`, `hex`, and cryptographic helper crates for protocol encoding and Swarm data structures.
- `axum`, `axum-server`, `tokio`, and `tower-http` for the local development server used outside the Wasm target.

### Concurrency and memory limitations

The browser runtime enables a high level of concurrency between Swarm tasks by combining libp2p streams, async channels, local futures, bounded upload and retrieval concurrency, and protocol-specific retry loops. This allows many protocol messages and chunk operations to be in flight at the same time even though the Wasm runtime itself is not using native threads.

The current browser architecture is still constrained by the WebAssembly execution environment. A tab-local runtime shares the browser's single-threaded Wasm event loop unless browser and build settings enable more advanced worker-based execution. Memory is also constrained by the WebAssembly address space and by practical browser limits.

Moving parts of the runtime into dedicated workers could improve isolation, memory headroom, and CPU parallelism in the future. That change would need to preserve browser transport support, Service Worker communication, secure vault boundaries, and compatibility with mobile browsers.

## [Planned development]

- Further hardening of Service Worker replacement, iframe boundaries, route handling, and injected-content attack surfaces.
- Additional security review around loaded websites, IndexedDB access, secure vault access, upload identities, postage state, and cheque signer material.
- Better reliability, error propagation, status reporting, and recovery for long-running retrieval, upload, settlement, and connection processes.
- Improved network profile management, bootnode handling, peer quality tracking, and dial retry behavior.
- More complete and stable JavaScript package documentation and examples for the `Weeb3No103` wrapper.
- Continued improvements to BZZ path handling, streaming media retrieval, byte-range serving, and manifest fork lookup performance.
- Worker-based partitioning or multithreading where browser support and the project architecture make it practical.
- Additional Swarm feature coverage, including ACT and other protocol features not yet fully implemented.
- Continued wallet, postage, chequebook, and swap UX refinements.
