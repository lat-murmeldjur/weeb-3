import init, { Weeb3WorkerRuntime } from "./weeb_3.js";

const SHARED_WORKER_PROTOCOL = 5;
const RUNTIME_ID = self.crypto?.randomUUID?.() ||
  `${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`;

let runtimePromise;
let startFlight;
let configuredNetworkId = null;
let vaultBroker = null;
let activeVaultClient = null;
let vaultQueue = Promise.resolve();
const vaultDisconnects = new Map();
let activeHlsLease = null;
let playbackLockRelease = null;
let serviceWorkerPort = null;
let activeTransferOperations = 0;
const clients = new Map();

const TRANSFER_NODE_OPERATIONS = new Set([
  "acquire",
  "retrieveBytes",
  "retrieveChunk",
  "acquireFeed",
  "upload",
  "pushChunk",
  "resolveBzz",
  "acquireRange"
]);

function errorResponse(error, status = 500) {
  return { ok: false, status, error: error?.message || String(error || "shared worker failure") };
}

function requestedNetworkId(message) {
  const networkId = Number(message?.networkId);
  return Number.isSafeInteger(networkId) ? networkId : null;
}

function getRuntime() {
  if (!runtimePromise) {
    runtimePromise = init().then(() => new Weeb3WorkerRuntime()).catch((error) => {
      runtimePromise = undefined;
      throw error;
    });
  }
  return runtimePromise;
}

function status() {
  return configuredNetworkId === null
    ? errorResponse("weeb-3 shared worker is not configured", 503)
    : {
        ok: true,
        type: "WEEB3_CLIENT_PONG",
        sharedWorkerProtocol: SHARED_WORKER_PROTOCOL,
        runtimeId: RUNTIME_ID,
        networkId: configuredNetworkId
      };
}

function closePort(port) {
  try { port?.close(); } catch (_) {}
}

function closeServiceWorkerPort(notify = false) {
  const current = serviceWorkerPort;
  serviceWorkerPort = null;
  if (!current) return;
  if (notify) {
    try {
      current.port.postMessage({
        type: "WEEB3_RUNTIME_PORT_INVALID",
        protocol: SHARED_WORKER_PROTOCOL,
        networkId: current.networkId
      });
    } catch (_) {}
  }
  closePort(current.port);
}

function bindServiceWorkerPort(message, port) {
  const networkId = requestedNetworkId(message);
  if (
    !port ||
    message?.serviceWorkerRelay !== SHARED_WORKER_PROTOCOL ||
    networkId === null ||
    networkId !== configuredNetworkId
  ) {
    try {
      port?.postMessage(errorResponse("service worker port targets another Swarm network", 409));
    } catch (_) {}
    closePort(port);
    return;
  }

  closeServiceWorkerPort(true);
  const binding = { networkId, port };
  serviceWorkerPort = binding;
  const invalidate = () => {
    if (serviceWorkerPort === binding) closeServiceWorkerPort();
  };
  port.addEventListener("message", (event) => {
    const request = event.data;
    if (request?.type !== "WEEB3_FETCH_REQUEST") {
      const reply = event.ports?.[0];
      try {
        reply?.postMessage(errorResponse("direct service worker port only accepts fetches", 400));
      } catch (_) {}
      closePort(reply);
      return;
    }
    void dispatchAndReply(request, event.ports?.[0], null);
  });
  port.addEventListener("messageerror", invalidate);
  port.addEventListener("close", invalidate);
  port.start();
  port.postMessage({
    ok: true,
    type: "WEEB3_RUNTIME_PORT_READY",
    protocol: SHARED_WORKER_PROTOCOL,
    networkId
  });
}

async function performStart(message, client) {
  if (message?.protocol !== SHARED_WORKER_PROTOCOL) {
    return errorResponse("incompatible weeb-3 shared worker protocol", 409);
  }
  const networkId = requestedNetworkId(message);
  if (networkId === null) return errorResponse("invalid weeb-3 shared worker network", 400);
  if (configuredNetworkId !== null && configuredNetworkId !== networkId) {
    if (activeTransferOperations > 0) {
      return errorResponse(
        "cannot switch Swarm network while dispatched transfers are settling",
        409
      );
    }
    if (activeHlsLease && activeHlsLease.clientId !== client?.id) {
      return errorResponse("another tab owns active HLS playback", 409);
    }
    closeServiceWorkerPort(true);
    const stale = activeHlsLease;
    activeHlsLease = null;
    releasePlaybackLock();
    if (stale) await cancelHlsLease(stale);
  }
  const previousNetworkId = configuredNetworkId;
  const changed = previousNetworkId !== networkId;
  const previousBroker = vaultBroker;
  if (client?.port) vaultBroker = client;
  // Rust can ask the window-backed vault whether cheques are active while
  // start() is still yielding. Publish the serialized transition first so
  // that startup request is validated against the network being started.
  configuredNetworkId = networkId;
  try {
    const response = await (await getRuntime()).start(message);
    if (response?.ok !== true || Number(response.networkId) !== networkId) {
      throw new Error(response?.error || "weeb-3 runtime failed to start");
    }
  } catch (error) {
    configuredNetworkId = previousNetworkId;
    if (vaultBroker === client) vaultBroker = previousBroker;
    throw error;
  }
  if (changed) {
    for (const connected of clients.values()) {
      try {
        connected.port.postMessage({ type: "WEEB3_NETWORK_CHANGED", networkId });
      } catch (_) {
        await removeClient(connected);
      }
    }
    console.info("[weeb-3] main process is running in SharedWorker", {
      networkId,
      runtimeId: RUNTIME_ID
    });
  }
  return status();
}

async function startRuntime(message, client) {
  const previous = startFlight;
  const flight = (previous ? previous.catch(() => undefined) : Promise.resolve())
    .then(() => performStart(message, client));
  startFlight = flight;
  try {
    return await flight;
  } catch (error) {
    return errorResponse(error);
  } finally {
    if (startFlight === flight) startFlight = undefined;
  }
}

function vaultRequest(request) {
  let fallback = null;
  for (const connected of clients.values()) fallback = connected;
  const target = activeVaultClient || vaultBroker || fallback;
  if (!target?.port) return Promise.reject(new Error("no window is available for weeb-3-secure"));
  if (requestedNetworkId(request) !== configuredNetworkId) {
    return Promise.reject(new Error("weeb-3-secure request targets another Swarm network"));
  }
  return new Promise((resolve, reject) => {
    const channel = new MessageChannel();
    let settled = false;
    const finish = (callback, value) => {
      if (settled) return;
      settled = true;
      clearTimeout(timeout);
      channel.port1.onmessage = null;
      channel.port1.close();
      const disconnects = vaultDisconnects.get(target.id);
      disconnects?.delete(disconnected);
      if (disconnects?.size === 0) vaultDisconnects.delete(target.id);
      callback(value);
    };
    const disconnected = () => finish(reject, new Error("weeb-3-secure window closed"));
    const timeout = setTimeout(
      () => finish(reject, new Error("weeb-3-secure request timed out")),
      250_000
    );
    const disconnects = vaultDisconnects.get(target.id) || new Set();
    disconnects.add(disconnected);
    vaultDisconnects.set(target.id, disconnects);
    channel.port1.onmessage = (event) => finish(resolve, event.data);
    channel.port1.start();
    try {
      target.port.postMessage({ type: "WEEB3_VAULT_REQUEST", request }, [channel.port2]);
    } catch (error) {
      finish(reject, error);
    }
  });
}

self.weeb3VaultCall = vaultRequest;

function requiresVault(message) {
  return message?.type === "UPLOAD_REQUEST" ||
    (message?.type === "WEEB3_NODE_REQUEST" &&
      (message.op === "upload" || message.op === "resetStamp"));
}

function carriesTransferAccounting(message) {
  return message?.type === "WEEB3_FETCH_REQUEST" ||
    message?.type === "UPLOAD_REQUEST" ||
    (message?.type === "WEEB3_NODE_REQUEST" && TRANSFER_NODE_OPERATIONS.has(message.op));
}

async function dispatchRuntimeMessage(message) {
  const runtime = await getRuntime();
  if (!carriesTransferAccounting(message)) return runtime.handleMessage(message);

  activeTransferOperations += 1;
  try {
    return await runtime.handleMessage(message);
  } finally {
    activeTransferOperations -= 1;
  }
}

function dispatchForClient(message, client) {
  if (!requiresVault(message)) return dispatch(message, client);
  const run = async () => {
    activeVaultClient = client || vaultBroker;
    try {
      return await dispatch(message, client);
    } finally {
      activeVaultClient = null;
    }
  };
  const flight = vaultQueue.then(run, run);
  vaultQueue = flight.then(() => undefined, () => undefined);
  return flight;
}

function acquirePlaybackLock() {
  if (playbackLockRelease || !self.navigator?.locks?.request) return;
  let release;
  const held = new Promise((resolve) => { release = resolve; });
  playbackLockRelease = release;
  void self.navigator.locks.request("weeb3-active-playback", {
    mode: "exclusive",
    ifAvailable: true
  }, async (lock) => {
    if (!lock) {
      if (playbackLockRelease === release) playbackLockRelease = null;
      return;
    }
    await held;
  }).catch(() => {
    if (playbackLockRelease === release) playbackLockRelease = null;
  });
}

function releasePlaybackLock() {
  const release = playbackLockRelease;
  playbackLockRelease = null;
  release?.();
}

async function cancelHlsLease(lease) {
  const runtime = await getRuntime();
  return lease.session !== null
    ? runtime.handleMessage({ type: "WEEB3_HLS_RELEASE", networkId: configuredNetworkId, session: lease.session })
    : runtime.handleMessage({ type: "WEEB3_HLS_CANCEL_PREPARE", networkId: configuredNetworkId, prepareKey: lease.prepareKey });
}

async function removeClient(client) {
  const id = client?.id;
  if (!id) return;
  clients.delete(id);
  const disconnects = vaultDisconnects.get(id);
  vaultDisconnects.delete(id);
  for (const disconnected of disconnects || []) disconnected();
  if (vaultBroker?.id === id) vaultBroker = null;
  if (activeHlsLease?.clientId === id) {
    const lease = activeHlsLease;
    activeHlsLease = null;
    releasePlaybackLock();
    try { await cancelHlsLease(lease); } catch (_) {}
  }
  if (clients.size === 0) closeServiceWorkerPort(true);
}

async function dispatch(message, client = null) {
  if (message?.type === "WEEB3_WORKER_START") return startRuntime(message, client);
  if (startFlight) await startFlight;
  if (message?.type === "WEEB3_CLIENT_PING") return status();
  if (message?.type === "WEEB3_VAULT_BROKER_CLAIM") {
    if (!client?.port) return errorResponse("vault broker must be a window", 400);
    vaultBroker = client;
    return { ok: true };
  }
  if (message?.type === "WEEB3_CLIENT_CLOSE") {
    await removeClient(client);
    return { ok: true };
  }
  if (configuredNetworkId === null) return errorResponse("weeb-3 shared worker is not configured", 503);
  const networkId = requestedNetworkId(message);
  if (networkId !== null && networkId !== configuredNetworkId) {
    return errorResponse("request targets another Swarm network", 409);
  }

  if (message?.type === "WEEB3_HLS_PREPARE") {
    const attempt = Number(message.attempt);
    if (!client?.id || !Number.isSafeInteger(attempt) || attempt <= 0) {
      return errorResponse("HLS preparation requires a window attempt", 400);
    }
    if (activeHlsLease && activeHlsLease.clientId !== client.id) {
      return errorResponse("another tab owns active HLS playback", 409);
    }
    const stale = activeHlsLease;
    const lease = { clientId: client.id, attempt, session: null, prepareKey: `${client.id}:${attempt}` };
    activeHlsLease = lease;
    try {
      if (stale) await cancelHlsLease(stale);
      // A Web Lock improves background tolerance when available. The singleton
      // HLS lease remains authoritative, so browsers without a lock must still play.
      void acquirePlaybackLock();
      const runtime = await getRuntime();
      const response = await runtime.handleMessage({ ...message, prepareKey: lease.prepareKey });
      const session = Number(response?.session);
      if (activeHlsLease !== lease) {
        if (response?.ok === true && Number.isSafeInteger(session)) {
          await runtime.handleMessage({ type: "WEEB3_HLS_RELEASE", networkId, session });
        }
        return errorResponse("HLS preparation was abandoned", 409);
      }
      if (response?.ok === true && Number.isSafeInteger(session)) {
        lease.session = session;
      } else {
        activeHlsLease = null;
        releasePlaybackLock();
      }
      return response;
    } catch (error) {
      if (activeHlsLease === lease) {
        activeHlsLease = null;
        releasePlaybackLock();
      }
      throw error;
    }
  }
  if (message?.type === "WEEB3_HLS_ABANDON") {
    const attempt = Number(message.attempt);
    if (activeHlsLease?.clientId === client?.id && activeHlsLease.attempt === attempt) {
      const lease = activeHlsLease;
      activeHlsLease = null;
      releasePlaybackLock();
      return cancelHlsLease(lease);
    }
    return { ok: true };
  }
  if (message?.type === "WEEB3_HLS_RELEASE") {
    const session = Number(message.session);
    if (activeHlsLease?.clientId !== client?.id || activeHlsLease.session !== session) {
      return errorResponse("HLS release does not own the active session", 409);
    }
    activeHlsLease = null;
    releasePlaybackLock();
  }
  if (message?.type === "WEEB3_HLS_CLEAR_CACHE" && activeHlsLease) {
    return errorResponse("cannot clear HLS cache during active playback", 409);
  }
  return dispatchRuntimeMessage(message);
}

async function dispatchAndReply(message, replyPort, client) {
  let response;
  try {
    response = await dispatchForClient(message, client);
  } catch (error) {
    response = errorResponse(error);
  }
  if (!replyPort) return;
  try {
    const body = response?.body;
    replyPort.postMessage(response, body instanceof Uint8Array ? [body.buffer] : []);
  } catch (_) {} finally {
    closePort(replyPort);
  }
}

self.addEventListener("connect", (event) => {
  const port = event.ports?.[0];
  if (!port) return;
  const client = { id: `${RUNTIME_ID}-${Math.random().toString(36).slice(2)}`, port };
  clients.set(client.id, client);
  port.addEventListener("message", (messageEvent) => {
    // A page restored from bfcache reuses its port after pagehide cleanup.
    if (!clients.has(client.id)) clients.set(client.id, client);
    if (messageEvent.data?.type === "WEEB3_RUNTIME_PORT_BIND") {
      bindServiceWorkerPort(messageEvent.data, messageEvent.ports?.[0]);
      return;
    }
    void dispatchAndReply(messageEvent.data, messageEvent.ports?.[0], client);
  });
  port.addEventListener("close", () => {
    void dispatch({ type: "WEEB3_CLIENT_CLOSE" }, client).catch(() => {});
  });
  port.start();
});
