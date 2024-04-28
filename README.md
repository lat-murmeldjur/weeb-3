# Rust-libp2p Browser-Server WebRTC Example

This example demonstrates how to use the `libp2p-webrtc-websys` transport library in a browser to ping the WebRTC Server.
It uses [wasm-pack](https://rustwasm.github.io/docs/wasm-pack/) to build the project for use in the browser.

## Running the example

Ensure you have `wasm-pack` [installed](https://rustwasm.github.io/wasm-pack/).

1. Build the client library:
```shell
wasm-pack build --target web --out-dir static
```

2. Start the server:
```shell
cargo run
```

3. Open the URL printed in the terminal

## [Notes]

### More Installed dependencies:

	protoc
	clang 

### Eqs

	e0 - header
	e1 - handshake
	e2 - hive
	e3 - pingpong
	e4 - pricing
	e5 - pseudosettle
	e6 - retrieval