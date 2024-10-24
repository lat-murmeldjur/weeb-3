console.log('Initializing worker')
import init, { run } from "./weeb_3.js";

await init();
run("yo");
console.log('Initialized worker');

// run("/ip4/127.0.0.1/udp/31336/webrtc-direct/certhash/uEiD9-vMnnZYO1MEIenfFCCNqfGA7rPnjyVfdC0VjJuoXWg/p2p/QmVne42GS4QKBg48bHrmotcC8TjqmMyg2ehkCbstUT5tSN");
