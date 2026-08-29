const SCOPE = new URL(self.registration.scope);
const SCOPE_PATH = SCOPE.pathname.endsWith("/") ? SCOPE.pathname : `${SCOPE.pathname}/`;
const NETWORK_ROUTE_PREFIXES = ["", "mainnet/", "testnet/"];
const RAW_ROUTE_KINDS = [
  ["hls/bytes", "hls-bytes"],
  ["bytes", "bytes"],
  ["chunks", "chunk"]
];
const BZZ_ROUTE_MARKERS = NETWORK_ROUTE_PREFIXES.map(
  (prefix) => `${SCOPE_PATH}${prefix}bzz/`
);
const RAW_ROUTE_MARKERS = NETWORK_ROUTE_PREFIXES.flatMap((prefix) =>
  RAW_ROUTE_KINDS.map(([kind, rawType]) => [`${SCOPE_PATH}${prefix}${kind}/`, rawType])
);
const FEED_ROUTE_MARKERS = NETWORK_ROUTE_PREFIXES.map(
  (prefix) => `${SCOPE_PATH}${prefix}feeds/`
);
const FETCH_TIMEOUT_MS = 240000;
const SERVICE_WORKER_MARKER = "forwarder-default29";
const SERVICE_WORKER_PROTOCOL = 10;
const MIB_BYTES = 1024 * 1024;
const STREAM_STORAGE_WINDOW_BYTES = MIB_BYTES / 2;
const STREAM_LOOKAHEAD_CHUNKS = 8;
const HLS_STREAM_WINDOW_BYTES = MIB_BYTES / 2;
const HLS_STREAM_INITIAL_LOOKAHEAD_CHUNKS = 1;
const HLS_STREAM_LOOKAHEAD_CHUNKS = 4;
const HLS_LIVE_STREAM_WINDOW_BYTES = MIB_BYTES / 2;
const HLS_LIVE_STREAM_LOOKAHEAD_CHUNKS = 4;
const RANGE_REQUEST_FLIGHTS = new Map();
const SHARED_WORKER_PROTOCOL = 5;
const WINDOW_RELAY_TIMEOUT_MS = 1_500;
const RUNTIME_PORT_BIND_TIMEOUT_MS = 1_500;

const cachedWindowNetworks = new Map();
const windowNetworkDiscoveries = new Map();
let runtimePort = null;
let runtimePortBinding = null;
let nextRuntimePortId = 0;

console.log(`weeb-3 service worker start ${SERVICE_WORKER_MARKER}`);

function logServiceWorkerVersion(reason) {
  console.log(`weeb-3 service worker ${reason} ${SERVICE_WORKER_MARKER}`);
}

function isSwarmReference(reference) {
  return /^(?:[a-fA-F0-9]{64}|[a-fA-F0-9]{128})$/.test(reference);
}

function canonicalRouteNetworkId(pathname) {
  if (!pathname.startsWith(SCOPE_PATH)) {
    return null;
  }

  const relative = pathname.substring(SCOPE_PATH.length);
  return relative === "testnet" || relative.startsWith("testnet/") ? 10 : 1;
}

function isNetworkShellPath(pathname) {
  return ["mainnet", "testnet"].some((mode) =>
    pathname === `${SCOPE_PATH}${mode}` || pathname === `${SCOPE_PATH}${mode}/`
  );
}

function isCanonicalStreamTopic(value) {
  try {
    const topic = decodeURIComponent(value || "");
    return Boolean(topic) &&
      topic !== "." &&
      topic !== ".." &&
      new TextEncoder().encode(topic).byteLength <= 256 &&
      !/[\u0000-\u001F\u007F-\u009F]/.test(topic);
  } catch (_) {
    return false;
  }
}

function isDirectShareShellPath(pathname) {
  if (!pathname.startsWith(SCOPE_PATH)) {
    return false;
  }

  const parts = pathname.substring(SCOPE_PATH.length).split("/");
  const streamOffset = parts[0] === "live" ? 1 : 0;
  return parts.length === streamOffset + 3 &&
    parts[streamOffset] === "stream" &&
    /^[a-fA-F0-9]{40}$/.test(parts[streamOffset + 1]) &&
    isCanonicalStreamTopic(parts[streamOffset + 2]);
}

function isBzzUploadPath(pathname) {
  return NETWORK_ROUTE_PREFIXES.some((prefix) => pathname === `${SCOPE_PATH}${prefix}bzz`);
}

function canonicalBzzResource(url) {
  for (const marker of BZZ_ROUTE_MARKERS) {
    if (!url.pathname.startsWith(marker)) {
      continue;
    }

    const resource = url.pathname.substring(marker.length);
    if (!resource) {
      return null;
    }

    const reference = resource.split("/", 1)[0];
    if (!isSwarmReference(reference)) {
      return null;
    }

    try {
      return decodeURIComponent(resource);
    } catch (_) {
      return resource;
    }
  }

  return null;
}

function canonicalRawResource(url) {
  for (const [marker, rawType] of RAW_ROUTE_MARKERS) {
    if (!url.pathname.startsWith(marker)) {
      continue;
    }

    const encodedResource = url.pathname.substring(marker.length);
    if (!encodedResource) {
      return null;
    }

    let resource;
    try {
      resource = decodeURIComponent(encodedResource);
    } catch (_) {
      resource = encodedResource;
    }
    if (rawType === "hls-bytes" && !isSwarmReference(resource)) {
      return null;
    }
    return resource;
  }

  return null;
}

function canonicalFeedResource(url) {
  for (const marker of FEED_ROUTE_MARKERS) {
    if (!url.pathname.startsWith(marker)) {
      continue;
    }

    const resource = url.pathname.substring(marker.length);
    const parts = resource.split("/");
    if (
      parts.length !== 2 ||
      !/^[a-fA-F0-9]{40}$/.test(parts[0]) ||
      !/^[a-fA-F0-9]{64}$/.test(parts[1])
    ) {
      return null;
    }

    return `${parts[0]}/${parts[1]}`;
  }

  return null;
}

function isHlsResource(url) {
  return canonicalFeedResource(url) !== null || RAW_ROUTE_MARKERS.some(([marker, rawType]) =>
    rawType === "hls-bytes" &&
    url.pathname.startsWith(marker) &&
    canonicalRawResource(url) !== null
  );
}

function isAppShellNavigation(request) {
  const headerDestination = request.headers.get("Sec-Fetch-Dest") || "";
  return request.method === "GET" &&
    request.mode === "navigate" &&
    request.destination !== "iframe" &&
    request.destination !== "frame" &&
    headerDestination !== "iframe" &&
    headerDestination !== "frame";
}

async function fetchOrError(request) {
  try {
    return await fetch(request);
  } catch (_) {
    return new Response("network fetch failed", { status: 502 });
  }
}

function appShellRequest(sourceRequest) {
  return new Request(new URL(SCOPE_PATH, self.registration.scope).toString(), {
    cache: sourceRequest.cache,
    credentials: "same-origin"
  });
}

self.addEventListener("install", (event) => {
  logServiceWorkerVersion("install");
  event.waitUntil(self.skipWaiting());
});

self.addEventListener("activate", (event) => {
  logServiceWorkerVersion("activate");
  event.waitUntil(self.clients.claim());
});

self.addEventListener("message", (event) => {
  if (
    (event.data?.type === "WEEB3_CLAIM" || event.data?.type === "WEEB3_PING") &&
    event.data?.protocol !== SERVICE_WORKER_PROTOCOL
  ) {
    return;
  }
  if (event.data?.type === "WEEB3_CLAIM") {
    const port = event.ports?.[0];
    event.waitUntil((async () => {
      await self.clients.claim();
      port?.postMessage({
        type: "WEEB3_CLAIMED",
        protocol: SERVICE_WORKER_PROTOCOL,
        scope: SCOPE_PATH,
        marker: SERVICE_WORKER_MARKER
      });
      closeMessagePort(port);
    })());
    return;
  }

  if (event.data?.type !== "WEEB3_PING") {
    return;
  }
  const port = event.ports?.[0];
  if (!port) {
    return;
  }
  port.postMessage({
    type: "WEEB3_PONG",
    protocol: SERVICE_WORKER_PROTOCOL,
    scope: SCOPE_PATH,
    marker: SERVICE_WORKER_MARKER
  });
  closeMessagePort(port);
});

self.addEventListener("fetch", (event) => {
  const request = event.request;
  const url = new URL(request.url);

  if (request.method === "POST" && url.origin === SCOPE.origin && isBzzUploadPath(url.pathname)) {
    event.respondWith(forwardUploadToRust(request, event.clientId, event.resultingClientId));
    return;
  }

  if (url.origin !== SCOPE.origin) {
    return;
  }

  if (
    isAppShellNavigation(request) &&
    (isNetworkShellPath(url.pathname) || isDirectShareShellPath(url.pathname))
  ) {
    event.respondWith(fetchOrError(appShellRequest(request)));
    return;
  }

  const bzzResource = canonicalBzzResource(url);
  if (bzzResource && (request.method === "GET" || request.method === "HEAD")) {
    if (isAppShellNavigation(request)) {
      event.respondWith(fetchOrError(appShellRequest(request)));
    } else {
      event.respondWith(forwardRequestToRust(request, event.clientId, event.resultingClientId));
    }
    return;
  }

  const rawResource = canonicalRawResource(url);
  if (rawResource && (request.method === "GET" || request.method === "HEAD")) {
    event.respondWith(forwardRequestToRust(request, event.clientId, event.resultingClientId));
    return;
  }

  const feedResource = canonicalFeedResource(url);
  if (feedResource && (request.method === "GET" || request.method === "HEAD")) {
    event.respondWith(forwardRequestToRust(request, event.clientId, event.resultingClientId));
    return;
  }

});

function isStableWindowClient(client) {
  if (!client || typeof client.postMessage !== "function") {
    return false;
  }
  try {
    const url = new URL(client.url);
    return client.type === "window" &&
      url.origin === SCOPE.origin &&
      url.pathname.startsWith(SCOPE_PATH);
  } catch (_) {
    return false;
  }
}

function invalidateWindowClient(client) {
  if (client?.id) {
    cachedWindowNetworks.delete(client.id);
  }
}

function invalidateRuntimePort(candidate = runtimePort) {
  if (!candidate || runtimePort !== candidate) {
    return;
  }
  runtimePort = null;
  closeMessagePort(candidate.port);
}

async function windowNetworkId(client) {
  if (cachedWindowNetworks.has(client.id)) {
    return cachedWindowNetworks.get(client.id);
  }

  let discovery = windowNetworkDiscoveries.get(client.id);
  if (!discovery) {
    discovery = messageClient(
      client,
      { type: "WEEB3_CLIENT_PING" },
      WINDOW_RELAY_TIMEOUT_MS
    ).then((response) => {
      const networkId = Number(response?.networkId);
      if (
        response?.ok !== true ||
        response?.type !== "WEEB3_CLIENT_PONG" ||
        Number(response?.sharedWorkerProtocol) !== SHARED_WORKER_PROTOCOL ||
        !Number.isSafeInteger(networkId)
      ) {
        return null;
      }
      cachedWindowNetworks.set(client.id, networkId);
      return networkId;
    }).finally(() => {
      if (windowNetworkDiscoveries.get(client.id) === discovery) {
        windowNetworkDiscoveries.delete(client.id);
      }
    });
    windowNetworkDiscoveries.set(client.id, discovery);
  }
  return discovery;
}

async function windowMatchesNetwork(client, requiredNetworkId) {
  const hadCachedNetwork = cachedWindowNetworks.has(client.id);
  if (await windowNetworkId(client) === requiredNetworkId) {
    return true;
  }
  if (!hadCachedNetwork) {
    return false;
  }
  // A SharedWorker network switch keeps the WindowClient identity stable.
  // Re-probe once instead of letting its old cached network strand the tab.
  invalidateWindowClient(client);
  return await windowNetworkId(client) === requiredNetworkId;
}

async function originatingWindow(clientId, resultingClientId) {
  for (const id of [clientId, resultingClientId]) {
    if (!id) {
      continue;
    }
    const client = await self.clients.get(id);
    if (isStableWindowClient(client)) {
      return client;
    }
  }
  return null;
}

async function requestClient(requiredNetworkId, clientId, resultingClientId, allowFallback = true) {
  const originating = await originatingWindow(clientId, resultingClientId);
  if (originating && await windowMatchesNetwork(originating, requiredNetworkId)) {
    return originating;
  }
  if (!allowFallback) {
    return null;
  }

  const clients = await self.clients.matchAll({
    type: "window",
    includeUncontrolled: false
  });
  const candidates = clients.filter(
    (client) => isStableWindowClient(client) && client.id !== originating?.id
  );
  const cached = candidates.find(
    (client) => cachedWindowNetworks.get(client.id) === requiredNetworkId
  );
  if (cached) {
    return cached;
  }
  const matches = await Promise.all(
    candidates.map((client) => windowMatchesNetwork(client, requiredNetworkId))
  );
  const index = matches.findIndex(Boolean);
  return index < 0 ? null : candidates[index];
}

function bindRuntimePort(client, networkId) {
  if (!client || typeof client.postMessage !== "function") {
    return Promise.resolve(null);
  }
  if (runtimePort?.networkId === networkId) {
    return Promise.resolve(runtimePort);
  }
  if (runtimePort) {
    invalidateRuntimePort(runtimePort);
  }
  if (runtimePortBinding) {
    return runtimePortBinding.then(() => bindRuntimePort(client, networkId));
  }

  const channel = new MessageChannel();
  const candidate = {
    id: `shared-worker:${networkId}:${++nextRuntimePortId}`,
    networkId,
    fallback: client,
    port: channel.port1
  };
  runtimePortBinding = new Promise((resolve) => {
    let settled = false;
    const finish = (value) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      if (!value) closeMessagePort(candidate.port);
      resolve(value);
    };
    const invalidate = () => {
      invalidateRuntimePort(candidate);
      finish(null);
    };
    const timer = setTimeout(invalidate, RUNTIME_PORT_BIND_TIMEOUT_MS);
    candidate.port.onmessage = (event) => {
      const message = event.data;
      if (
        message?.type !== "WEEB3_RUNTIME_PORT_READY" ||
        message?.ok !== true ||
        Number(message?.protocol) !== SHARED_WORKER_PROTOCOL ||
        Number(message?.networkId) !== networkId
      ) {
        invalidate();
        return;
      }
      runtimePort = candidate;
      candidate.port.onmessage = (controlEvent) => {
        if (controlEvent.data?.type === "WEEB3_RUNTIME_PORT_INVALID") {
          invalidateRuntimePort(candidate);
        }
      };
      finish(candidate);
    };
    candidate.port.onmessageerror = invalidate;
    candidate.port.addEventListener("close", invalidate);
    candidate.port.start();
    try {
      client.postMessage({
        type: "WEEB3_RUNTIME_PORT_BIND",
        networkId,
        serviceWorkerRelay: SHARED_WORKER_PROTOCOL
      }, [channel.port2]);
    } catch (_) {
      closeMessagePort(channel.port2);
      invalidate();
    }
  }).finally(() => {
    runtimePortBinding = null;
  });
  return runtimePortBinding;
}

async function requestRuntime(networkId, clientId, resultingClientId) {
  if (runtimePort?.networkId === networkId) {
    return runtimePort;
  }
  const client = await requestClient(networkId, clientId, resultingClientId);
  return await bindRuntimePort(client, networkId) || client;
}

function closeMessagePort(port) {
  try {
    port.onmessage = null;
    port.onmessageerror = null;
    port.close();
  } catch (_) {
  }
}

function errorResult(status, error) {
  return { ok: false, status, error };
}

function messageChannelRequest(timeoutMs, invalidate, send, sendFailure, receive = value => value) {
  return new Promise((resolve) => {
    const channel = new MessageChannel();
    let settled = false;
    const settle = (value) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      closeMessagePort(channel.port1);
      resolve(value || errorResult(500, "empty weeb-3 response"));
    };
    const timer = setTimeout(() => {
      invalidate();
      settle(errorResult(504, "Timed out waiting for weeb-3"));
    }, timeoutMs);
    channel.port1.onmessage = (event) => settle(receive(event.data));
    try {
      send(channel.port2);
    } catch (error) {
      closeMessagePort(channel.port2);
      invalidate();
      void Promise.resolve(sendFailure(error)).then(settle);
    }
  });
}

function messageClient(client, message, timeoutMs = FETCH_TIMEOUT_MS) {
  if (!client || typeof client.postMessage !== "function") {
    invalidateWindowClient(client);
    return Promise.resolve(errorResult(502, "weeb-3 window relay is not available"));
  }
  const invalidate = () => invalidateWindowClient(client);
  return messageChannelRequest(
    timeoutMs,
    invalidate,
    (port) => {
      message.serviceWorkerRelay = SHARED_WORKER_PROTOCOL;
      client.postMessage(message, [port]);
    },
    (error) => errorResult(502, error?.message || "failed to message weeb-3"),
    (value) => {
      if (!value) invalidate();
      return value;
    }
  );
}

function messageRuntimePort(runtime, message, timeoutMs) {
  // A timed-out dispatched request is detached, never replayed. A synchronous
  // send failure happened before dispatch, so the validated window remains safe.
  return messageChannelRequest(
    timeoutMs,
    () => invalidateRuntimePort(runtime),
    (port) => runtime.port.postMessage(message, [port]),
    () => {
      // A synchronous post failure happened before dispatch, so the validated
      // broker window is still a safe fallback for this one request.
      return messageClient(runtime.fallback, message, timeoutMs);
    }
  );
}

function messageRuntime(runtime, message, timeoutMs = FETCH_TIMEOUT_MS) {
  if (!runtime) {
    const networkId = Number(message?.networkId);
    return Promise.resolve(errorResult(
      503,
      Number.isSafeInteger(networkId)
        ? `weeb-3 runtime for Swarm network ${networkId} is not available`
        : "weeb-3 runtime is not available"
    ));
  }

  // Timeouts detach the port; dispatched accounting work is never replayed.
  const response = runtime.port
    ? messageRuntimePort(runtime, message, timeoutMs)
    : messageClient(runtime, message, timeoutMs);
  return response.then((response) => {
    if (Number(response?.status) === 409) {
      // A network transition can make the cached discovery stale. Never replay
      // this request; make the next request discover the worker again.
      if (runtime.port) invalidateRuntimePort(runtime);
      else invalidateWindowClient(runtime);
    }
    return response;
  });
}

function requestRustFetch(
  client,
  requestUrl,
  method,
  range,
  networkId,
  ifNoneMatch = "",
  ifRange = ""
) {
  return messageRuntime(client, {
    type: "WEEB3_FETCH_REQUEST",
    url: requestUrl,
    method,
    range,
    networkId,
    ifNoneMatch,
    ifRange
  });
}

function toUint8Array(body) {
  if (body instanceof Uint8Array) {
    return body;
  }
  if (body instanceof ArrayBuffer) {
    return new Uint8Array(body);
  }
  if (ArrayBuffer.isView(body)) {
    return new Uint8Array(body.buffer, body.byteOffset, body.byteLength);
  }
  return new Uint8Array();
}

function responseBodyStream(body) {
  const bytes = toUint8Array(body);
  return new ReadableStream({
    start(controller) {
      controller.enqueue(bytes);
      controller.close();
    }
  });
}

function responseHeaders(headerRows) {
  const headers = new Headers();
  for (const row of headerRows || []) {
    if (row && row.length >= 2) {
      headers.set(String(row[0]), String(row[1]));
    }
  }
  return headers;
}

function requestRustRange(client, url, start, end, networkId) {
  const key = `${client?.id || ""}|${networkId}|${url}|${start}|${end}`;
  const existing = RANGE_REQUEST_FLIGHTS.get(key);
  if (existing) {
    return existing;
  }

  const request = messageRuntime(client, {
    type: "WEEB3_FETCH_REQUEST",
    url,
    method: "GET",
    range: `bytes=${start}-${end}`,
    networkId
  }).then((response) => {
    if (!response || !response.ok) {
      return response || { ok: false, status: 503, error: "empty weeb-3 range response" };
    }

    const body = toUint8Array(response.body);
    const expected = end - start + 1;
    if (body.byteLength !== expected) {
      return {
        ok: false,
        status: 502,
        error: `weeb-3 returned ${body.byteLength} bytes for ${expected} byte range`
      };
    }

    return { ok: true, body };
  });
  RANGE_REQUEST_FLIGHTS.set(key, request);
  request.finally(() => {
    if (RANGE_REQUEST_FLIGHTS.get(key) === request) {
      RANGE_REQUEST_FLIGHTS.delete(key);
    }
  }).catch(() => {});
  return request;
}

function createRustRangeStream(
  client,
  url,
  size,
  networkId,
  windowBytes,
  initialLookahead,
  lookahead
) {
  let position = 0;
  let schedulePosition = 0;
  let admissionOpen = true;
  const scheduled = new Map();

  const nextRangeBounds = () => {
    if (!admissionOpen || schedulePosition >= size) {
      return null;
    }

    const start = schedulePosition;
    const end = Math.min(start + windowBytes - 1, size - 1);
    schedulePosition = end + 1;
    return { start, end };
  };

  const admitRange = () => {
    const range = nextRangeBounds();
    if (!range) {
      return null;
    }
    const { start, end } = range;
    const request = requestRustRange(client, url, start, end, networkId);
    scheduled.set(start, request);
    return { start, request };
  };

  const scheduleMore = () => {
    const limit = position < initialLookahead * windowBytes ? initialLookahead : lookahead;
    while (admissionOpen && schedulePosition < size && scheduled.size < limit) {
      if (!admitRange()) {
        break;
      }
    }
  };

  const drainScheduledRanges = () => {
    return Promise.allSettled(Array.from(scheduled.values())).then(() => {
      scheduled.clear();
    });
  };

  const closeAdmission = () => {
    admissionOpen = false;
  };

  const failStream = async (controller, error) => {
    if (!admissionOpen) {
      return;
    }
    closeAdmission();
    controller.error(error);
    await drainScheduledRanges();
  };

  return new ReadableStream({
    async pull(controller) {
      try {
        if (position >= size) {
          closeAdmission();
          controller.close();
          await drainScheduledRanges();
          return;
        }

        scheduleMore();
        const start = Math.floor(position / windowBytes) * windowBytes;
        let pending = scheduled.get(start);
        if (!pending) {
          const foreground = admitRange();
          if (!foreground || foreground.start !== start) {
            await failStream(
              controller,
              new Error("weeb-3 foreground stream window was not admitted")
            );
            return;
          }
          pending = foreground.request;
        }
        const response = await pending;
        scheduled.delete(start);

        if (!admissionOpen) {
          return;
        }
        if (!response || !response.ok) {
          await failStream(
            controller,
            new Error(response && response.error ? response.error : "weeb-3 range request failed")
          );
          return;
        }

        const body = toUint8Array(response.body);
        position = start + body.byteLength;
        controller.enqueue(body);
        scheduleMore();
      } catch (error) {
        if (admissionOpen) {
          await failStream(controller, error);
        }
      }
    },
    cancel() {
      closeAdmission();
      return drainScheduledRanges();
    }
  });
}

async function forwardRequestToRust(request, clientId, resultingClientId) {
  try {
    const url = new URL(request.url);
    const networkId = canonicalRouteNetworkId(url.pathname);
    if (networkId === null) {
      return new Response("weeb-3 route is outside the configured scope", { status: 400 });
    }
    const client = await requestRuntime(networkId, clientId, resultingClientId);
    const hlsResource = isHlsResource(url);
    const response = await requestRustFetch(
      client,
      request.url,
      request.method,
      hlsResource ? "" : request.headers.get("Range") || "",
      networkId,
      request.headers.get("If-None-Match") || "",
      hlsResource ? "" : request.headers.get("If-Range") || ""
    );

    const status = Number(response.status || (response.ok ? 200 : 404));
    const headers = responseHeaders(response.headers);

    if (!response.ok) {
      return new Response(response.error || "weeb-3 request failed", {
        status,
        headers
      });
    }

    if (response.stream && request.method !== "HEAD") {
      const size = Number(headers.get("Content-Length") || "0");
      if (!Number.isSafeInteger(size) || size <= 0) {
        return new Response("weeb-3 stream response has an invalid length", { status: 502 });
      }
      const liveHlsResource = hlsResource && url.searchParams.get("start") === "live";
      const windowBytes = liveHlsResource
        ? HLS_LIVE_STREAM_WINDOW_BYTES
        : hlsResource ? HLS_STREAM_WINDOW_BYTES : STREAM_STORAGE_WINDOW_BYTES;
      const lookahead = liveHlsResource
        ? HLS_LIVE_STREAM_LOOKAHEAD_CHUNKS
        : hlsResource ? HLS_STREAM_LOOKAHEAD_CHUNKS : STREAM_LOOKAHEAD_CHUNKS;
      const initialLookahead = hlsResource
        ? HLS_STREAM_INITIAL_LOOKAHEAD_CHUNKS
        : lookahead;
      return new Response(createRustRangeStream(
        client,
        request.url,
        size,
        networkId,
        windowBytes,
        initialLookahead,
        lookahead
      ), {
        status,
        headers
      });
    }

    return new Response(
      request.method === "HEAD" || status === 304
        ? null
        : responseBodyStream(response.body),
      {
        status,
        headers
      }
    );
  } catch (error) {
    return new Response(error && error.message ? error.message : "weeb-3 forwarder error", {
      status: 502
    });
  }
}

function parseUploadRedundancyHeader(value) {
  if (value === null || value === "") {
    return 1;
  }
  if (!/^[0-9]+$/.test(value)) {
    return null;
  }

  const level = Number(value);
  return Number.isSafeInteger(level) && level <= 4 ? level : null;
}

async function forwardUploadToRust(request, clientId, resultingClientId) {
  try {
    const url = new URL(request.url);
    const networkId = canonicalRouteNetworkId(url.pathname);
    if (networkId === null) {
      return new Response("weeb-3 route is outside the configured scope", { status: 400 });
    }
    const formData = await request.formData();
    const file = formData.get("file");

    if (!(file instanceof File)) {
      return new Response("No file in form data", { status: 400 });
    }

    const parsedRedundancy = parseUploadRedundancyHeader(
      request.headers.get("swarm-redundancy-level")
    );
    if (parsedRedundancy === null) {
      return new Response("Swarm-Redundancy-Level must be an integer from 0 to 4", { status: 400 });
    }

    const client = await requestClient(networkId, clientId, resultingClientId, false);
    const response = await messageRuntime(client, {
      type: "UPLOAD_REQUEST",
      file,
      networkId,
      encryption: request.headers.get("swarm-encrypt") === "true",
      redundancyLevel: parsedRedundancy,
      indexString: request.headers.get("swarm-index-document") || "",
      addToFeed: request.headers.get("swarm-collection") === "true",
      feedTopic: url.searchParams.get("feedTopic") || ""
    });

    if (!response.ok) {
      return new Response(response.error || "Upload failed", { status: Number(response.status || 500) });
    }

    return new Response(JSON.stringify({ reference: response.reference || "" }), {
      status: 201,
      headers: { "Content-Type": "application/json" }
    });
  } catch (error) {
    return new Response(error && error.message ? error.message : "weeb-3 upload forwarder error", {
      status: 502
    });
  }
}
