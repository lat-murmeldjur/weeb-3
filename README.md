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

3. Open the application URL, for example [`https://localhost:8080/weeb-3`](https://localhost:8080/weeb-3), or the GitHub Pages hosted version at [`https://lat-murmeldjur.github.io/weeb-3`](https://lat-murmeldjur.github.io/weeb-3).

## Using the npm package

The `wasm-pack` build prepares the generated `static/package.json` for publishing to npm together with the wrapper assets required by the browser package:

- `static/snippets/web3-0742d85b024bb6f5/inline0.js`
- `static/weeb_3.js`
- `static/weeb_3_bg.wasm`

After publishing, the package can be used with the same API shape as the examples in `static/example.html` and `static/issue-1-json-sync-example.html`:

```js
import init, { Weeb3No103, BootstrapNode } from "@lat-murmeldjur/weeb_3";

await init();

const weeb3node = new Weeb3No103();

// Use the built-in network profile and bootnodes.
await weeb3node.connect();

// Or start with explicit browser-dialable bootnodes.
weeb3node.start([
  new BootstrapNode("/ip4/example/tcp/443/wss/p2p/examplePeerId", true),
], "10");

const ready = await weeb3node.ready(1, 20_000);
```

The wrapper exposes the browser node as `Weeb3No103`. It can start the runtime, connect to network profiles, render the bundled interface into a container, report network and progress state, retrieve BZZ resources, retrieve raw bytes or chunks, upload `File` objects or byte arrays, publish and read feed updates, and expose feed identity helpers.

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

- The browser interface, implemented primarily by `static/index.html`, `src/interface.rs`, `src/interface_conventions.rs`, and `src/interface_runtime_conventions.rs`.
- The libp2p / Swarm node, whose main entry point is `src/lib.rs`.
- The Swarm protocol handlers and data pipelines for handshake, peer discovery, pricing, accounting, retrieval, pushsync, pseudosettle, swap, manifests, feeds, streaming, and uploads.
- The Service Worker in `static/service.js`, which provides deterministic browser routes for Swarm content and forwards canonical requests into the Rust runtime.
- The npm / library facade in `src/library.rs`, which wraps the same runtime for embedding in other browser applications.
- Browser persistence, secure local state, network profiles, and on-chain integration implemented by `src/persistence.rs`, `src/secure_vault.rs`, `src/network_profile.rs`, and `src/on_chain.rs`.

Below is a piece-by-piece overview of the current component logic.

#### The interface

The default browser application is instantiated by `static/index.html`, which loads the generated Wasm module and calls `interweeb` from `src/interface.rs`.

`interweeb` creates a `Weeb3` node, clears legacy hash-based paths through `src/nav.rs`, and delegates the rest of the UI setup to `mount_interface`. `mount_interface` can either start the runtime itself or attach the interface to a runtime that has already been started by the package wrapper.

The interface layer currently has the following roles:

- Starting the libp2p / Swarm runtime in an async browser task when requested.
- Installing UI conventions and rendering the interface shell.
- Preloading the secure vault module before sensitive upload, feed, stamp, or cheque operations are requested.
- Registering the Service Worker and routing Service Worker messages back to the Rust runtime.
- Reading the configured network profile, network id, and browser-dialable bootnodes, then passing bootnode connection requests to the `Weeb3` node.
- Wiring the navigation input so BZZ references, raw byte routes, and chunk routes can be opened from the UI.
- Wiring upload controls for single files, tar-based collections, optional encryption, index document selection, optional feed publishing, and postage-stamp reuse or reset.
- Wiring on-chain controls for upload prerequisites, postage batch acquisition, chequebook deployment, cheque signer persistence, and chequebook deposits through the browser wallet.
- Providing runtime controls such as pausing and resuming transfers.
- Rendering retrieved resources, website iframes, streaming media, raw downloads, logs, connection status, network state, and progress rows.

The current interface no longer assumes that requests are handled by a shared worker. The tab owns the `Weeb3` runtime, while the Service Worker acts as a request forwarder between browser fetch events and the active controlled client.

#### The weeb process

The main Swarm client is implemented in `src/lib.rs`. It is compiled only for the `wasm32` target and is designed to run in the browser event loop.

At a high level, `src/lib.rs` does the following:

- Imports the generated protobuf protocol modules from `etiquette_0` through `etiquette_8`.
- Defines the Swarm protocol names used by the client, including handshake, pricing, hive peer discovery, pseudosettle, retrieval, pushsync, and swap.
- Defines network mode helpers for testnet and mainnet. The built-in profiles currently map Swarm network id `10` to the Sepolia-based testnet profile and Swarm network id `1` to the Gnosis / xDAI mainnet profile.
- Defines the `Weeb3` client, which owns the libp2p `Swarm`, runtime channels, connection state, network id, progress store, transfer pause flag, and peer registry.
- Defines `Wings`, the in-memory peer and accounting registry used to track connected peers, overlay addresses, bootnodes, accounting peers, settlement state, known underlays, and self-observed ephemeral addresses.
- Exposes the runtime functions used by the interface and library wrapper.

The most important public `Weeb3` operations are:

1. Changing the network id and bootnode address.
2. Disconnecting and clearing peer state when the active network profile changes.
3. Uploading a `File` or tar collection, optionally encrypted, optionally with an index document, and optionally as a feed update.
4. Pushing a raw chunk through pushsync.
5. Resolving and acquiring BZZ resources.
6. Retrieving raw bytes or individual chunks.
7. Reading feed envelopes and feed content.
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
- Initializes the default Swarm network id to `10`.

The `run` function is the long-running runtime loop. It builds a channel-based asynchronous task graph for the browser runtime and coordinates the major subsystems:

- Peer discovery, bootnode dialing, connection retry, and connection cleanup.
- Incoming and outgoing libp2p stream handling.
- Handshake, identify, pricing, and peer promotion.
- Accounting, pseudosettle refreshes, cheque sending, and swap-related settlement messages.
- High-level BZZ resolution, range preparation, and BZZ range retrieval.
- Data-level retrieval and upload requests.
- Chunk-level retrieval and pushsync with bounded concurrency.
- Upload progress reporting.
- Transfer pause and cancellation checks.
- Log forwarding to the interface.

The runtime is heavily asynchronous, but it is still running inside the browser's Wasm execution environment. It uses `spawn_local`, async channels, short queue polling, and protocol-specific retry delays rather than OS threads.

#### The Swarm Client Subcomponents

The main runtime depends on several focused modules:

- `src/handlers.rs` implements the libp2p stream handlers for Swarm protocol traffic such as handshake, hive, pricing, pseudosettle, retrieval, pushsync, and swap.
- `src/accounting.rs` implements local accounting, price calculations, reservations, peer credit / debit tracking, refresh triggers, and settlement coordination.
- `src/addresses.rs` normalizes and validates browser-dialable underlays, including WebSocket and secure WebSocket multiaddresses.
- `src/retrieval.rs` implements chunk retrieval, peer selection, validation, decryption, data joining, and request coordination.
- `src/upload.rs` implements file splitting, optional encryption, chunk creation, postage stamp use, pushsync, manifest creation, SOC creation, and feed upload support.
- `src/manifest.rs` interprets Swarm manifests.
- `src/manifest_upload.rs` creates manifests for uploads and collections.
- `src/bzz_stream.rs` parses canonical BZZ resources, resolves manifests and paths, prepares range trees, and retrieves byte ranges for BZZ resources.
- `src/streaming_player.rs` integrates range retrieval with browser fetch requests and streaming media playback.
- `src/nav.rs` normalizes browser paths and extracts BZZ route references from the location bar.
- `src/ens.rs` resolves ENS content hashes to Swarm references.
- `src/events.rs` stores progress rows and progress revisions for the UI and package wrapper.
- `src/persistence.rs` stores browser-side data in IndexedDB.
- `src/secure_vault.rs` manages sensitive local state such as upload identities, postage-stamp state, feed ownership, and cheque signer material.
- `src/network_profile.rs` defines the built-in testnet and mainnet profiles, wallet chain ids, token symbols, and bootnodes.
- `src/on_chain.rs` implements browser wallet and contract interactions for postage batches, price oracle access, chequebook operations, swap token operations, and related state.
- `src/interface_conventions.rs` and `src/interface_runtime_conventions.rs` contain DOM helpers, UI rendering, route parsing, network controls, and Service Worker runtime integration.
- `src/library.rs` exposes the Wasm runtime to JavaScript as `Weeb3No103` and `BootstrapNode`.
- `src/conventions.rs` and `src/interface_conventions.rs` collect common encoding, decoding, hashing, resource, UI, and protocol helper logic.

The ABI files in `src/*.json` are consumed by the on-chain module and cover contracts such as the postage stamp contract, price oracle, factory, sBZZ token, and simple swap contract.

#### Persistence and identity

The browser runtime maintains a mix of ephemeral and persistent state.

The libp2p identity used by a `Weeb3` runtime is generated when the node is created. That makes the live peer identity tab-local and runtime-local. Peer maps, connection attempts, active streams, and accounting state are kept in memory by the `Weeb3` and `Wings` structures.

Browser persistence is used for state that should survive page reloads or browser sessions, such as retrieved chunks, chequebook data, signer material, postage-stamp state, and other runtime settings. Sensitive state is routed through the secure vault module instead of being handled directly by ordinary UI code.

Wallet access is requested only for on-chain operations. The browser wallet is used for chain switching, account access, postage purchase flows, chequebook deployment, and deposits. Upload/feed identities and cheque signer keys are managed separately from the wallet account so that Swarm protocol operations do not require signing every action with the injected wallet.

When the network profile changes, the runtime clears the current peer state and increments its connection generation so stale dialing, handshake, and connection events do not leak into the new network session.

### The Service Worker

The Service Worker in `static/service.js` sits between the browser fetch layer and the active weeb-3 page. Its role has expanded beyond the original static cache approach.

Single files can still be displayed without a Service Worker by creating `Blob` object URLs with the correct MIME type. This is enough for images, documents, and other standalone files. It is not enough for full websites, because browser-generated object URLs contain random identifiers and therefore cannot reliably satisfy relative paths for scripts, stylesheets, images, and other website assets.

The Service Worker solves this by providing deterministic application-scoped routes:

- `GET` and `HEAD` requests below `/bzz/<reference>/<path>` are interpreted as canonical BZZ resource requests.
- Raw byte and chunk routes below `/bytes/`, `/chunks/`, and `/chunk/` are forwarded to the Rust runtime.
- `POST` requests to the scoped `/bzz` endpoint are forwarded as upload requests, including upload headers such as encryption, collection, and index-document hints.
- Fetch requests are forwarded to the active controlled client through `postMessage` and `MessageChannel`.
- BZZ resources can be answered as full responses, byte-range responses, or streaming responses depending on MIME type, request headers, and resource size.
- The app shell is cached with a network-first strategy so that the interface can continue to load when a cached shell is available.

This design means that rendered Swarm websites can request their own relative assets through ordinary browser fetch/navigation behavior, while the active Rust runtime resolves and retrieves the underlying Swarm data.

The Service Worker is security-sensitive. Browsers only enable it for secure origins, and a trusted certificate is required for normal deployment. Because a Service Worker can intercept requests for its scope, production deployments should treat Service Worker replacement, injected pages, and malicious Swarm-hosted websites as important security boundaries. The current architecture reduces some risk by rendering Swarm websites in iframes and by keeping sensitive state behind the secure vault layer, but security hardening remains an active development area.

### Main dependencies

The weeb-3 project uses the following main Rust crates and browser bindings:

- `libp2p` and `libp2p-stream` for peer identity, transport, multiplexing, stream protocols, identify, ping, autonat / dcutr support, and browser WebSocket transport.
- `async-std` and `async-lock` for async runtime primitives that work in the browser Wasm target.
- `wasm-bindgen`, `wasm-bindgen-futures`, `js-sys`, and `web-sys` for JavaScript, DOM, Service Worker, browser API, and Promise integration.
- `web3`, `alloy`, `alloy-signer-local`, and `ethers` for wallet, signing, ABI, and on-chain contract interaction.
- `indexed_db_futures` for browser IndexedDB persistence.
- `tar` and `mime_guess` for collection upload handling and MIME inference.
- `getrandom` with the `wasm_js` backend for browser-compatible randomness.
- `base64`, `hex`, `byteorder`, and numeric / cryptographic helper crates for protocol encoding and Swarm data structures.
- `tokio` and `tower-http` for the local development server used outside the Wasm target.

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
