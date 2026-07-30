const SCOPE = new URL(self.registration.scope);
const SCOPE_PATH = SCOPE.pathname.endsWith("/") ? SCOPE.pathname : `${SCOPE.pathname}/`;
const APP_ROOT = SCOPE_PATH;
const APP_INDEX = `${SCOPE_PATH}index.html`;
const NETWORK_ROUTE_PREFIXES = ["", "mainnet/", "testnet/"];
const RAW_ROUTE_KINDS = [
  ["hls/bytes", "hls-bytes"],
  ["bytes", "bytes"],
  ["chunks", "chunk"]
];
const FETCH_TIMEOUT_MS = 240000;
const SERVICE_WORKER_MARKER = "forwarder-default20";
const SERVICE_WORKER_PROTOCOL = 5;
const MIB_BYTES = 1024 * 1024;
const STREAM_STORAGE_WINDOW_BYTES = MIB_BYTES / 2;
const STREAM_LOOKAHEAD_CHUNKS = 8;
const HLS_REQUEST_FLIGHTS = new Map();
const CLIENT_RUNTIME_PROBES = new Map();
const CLIENT_RUNTIME_PROBE_TIMEOUT_MS = 1_500;

console.log(`weeb-3 service worker start ${SERVICE_WORKER_MARKER}`);

function logServiceWorkerVersion(reason) {
  console.log(`weeb-3 service worker ${reason} ${SERVICE_WORKER_MARKER}`);
}

function isSwarmReference(reference) {
  return /^(?:[a-fA-F0-9]{64}|[a-fA-F0-9]{128})$/.test(reference);
}

function bzzMarkers() {
  return NETWORK_ROUTE_PREFIXES.map((prefix) => `${SCOPE_PATH}${prefix}bzz/`);
}

function rawRouteMarkers() {
  return NETWORK_ROUTE_PREFIXES.flatMap((prefix) =>
    RAW_ROUTE_KINDS.map(([kind, rawType]) => [`${SCOPE_PATH}${prefix}${kind}/`, rawType])
  );
}

function feedMarkers() {
  return NETWORK_ROUTE_PREFIXES.map((prefix) => `${SCOPE_PATH}${prefix}feeds/`);
}

function canonicalRouteNetworkId(pathname) {
  if (!pathname.startsWith(SCOPE_PATH)) {
    return null;
  }

  const relative = pathname.substring(SCOPE_PATH.length);
  const first = relative.split("/", 1)[0];
  return first === "testnet" ? 10 : 1;
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
  return parts.length === 3 &&
    parts[0] === "stream" &&
    /^[a-fA-F0-9]{40}$/.test(parts[1]) &&
    isCanonicalStreamTopic(parts[2]);
}

function isBzzUploadPath(pathname) {
  return NETWORK_ROUTE_PREFIXES.some((prefix) => pathname === `${SCOPE_PATH}${prefix}bzz`);
}

function canonicalBzzResource(url) {
  for (const marker of bzzMarkers()) {
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
  for (const [marker, rawType] of rawRouteMarkers()) {
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
  for (const marker of feedMarkers()) {
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
  return new Request(new URL(APP_ROOT, self.registration.scope).toString(), {
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
  if (event.data?.type === "WEEB3_CLAIM") {
    const port = event.ports?.[0];
    event.waitUntil((async () => {
      await self.clients.claim();
      port?.postMessage({
        type: "WEEB3_CLAIMED",
        protocol: SERVICE_WORKER_PROTOCOL,
        scope: SCOPE_PATH
      });
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
    scope: SCOPE_PATH
  });
});

self.addEventListener("fetch", (event) => {
  const request = event.request;
  const url = new URL(request.url);

  if (request.method === "POST" && url.origin === SCOPE.origin && isBzzUploadPath(url.pathname)) {
    event.respondWith(forwardUploadToRust(request, event));
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
      event.respondWith(forwardRequestToRust(request, event));
    }
    return;
  }

  const rawResource = canonicalRawResource(url);
  if (rawResource && (request.method === "GET" || request.method === "HEAD")) {
    event.respondWith(forwardRequestToRust(request, event));
    return;
  }

  const feedResource = canonicalFeedResource(url);
  if (feedResource && (request.method === "GET" || request.method === "HEAD")) {
    event.respondWith(forwardRequestToRust(request, event));
    return;
  }

});

function clientInScope(client) {
  try {
    const url = new URL(client.url);
    return url.origin === SCOPE.origin && url.pathname.startsWith(SCOPE_PATH);
  } catch (_) {
    return false;
  }
}

function isTopLevelClient(client) {
  return client.frameType === "top-level" || client.frameType === "auxiliary";
}

function bzzReferenceFromResource(resource) {
  return resource ? resource.split("/", 1)[0] : "";
}

function bzzReferenceFromUrl(url) {
  return bzzReferenceFromResource(canonicalBzzResource(url));
}

function isAppShellClient(client) {
  if (!clientInScope(client)) {
    return false;
  }

  try {
    const url = new URL(client.url);
    if (url.pathname === APP_ROOT || url.pathname === APP_INDEX) {
      return true;
    }

    if (isNetworkShellPath(url.pathname) || isDirectShareShellPath(url.pathname)) {
      return true;
    }

    const bzzResource = canonicalBzzResource(url);
    return Boolean(bzzResource && !bzzResource.includes("/"));
  } catch (_) {
    return false;
  }
}

function pushUniqueClient(list, seen, client) {
  if (!client || seen.has(client.id)) {
    return;
  }
  seen.add(client.id);
  list.push(client);
}

async function clientWeeb3NetworkId(client) {
  const existing = CLIENT_RUNTIME_PROBES.get(client.id);
  if (existing) {
    return existing;
  }

  let tracked;
  tracked = messageClient(
    client,
    { type: "WEEB3_CLIENT_PING" },
    CLIENT_RUNTIME_PROBE_TIMEOUT_MS
  )
    .then((response) => {
      const ready = response?.ok === true && response?.type === "WEEB3_CLIENT_PONG";
      const networkId = Number(response?.networkId);
      return ready && Number.isSafeInteger(networkId) ? networkId : null;
    })
    .finally(() => {
      if (CLIENT_RUNTIME_PROBES.get(client.id) === tracked) {
        CLIENT_RUNTIME_PROBES.delete(client.id);
      }
    });
  CLIENT_RUNTIME_PROBES.set(client.id, tracked);
  return tracked;
}

async function firstReadyClient(candidates, requiredNetworkId) {
  if (!candidates.length) {
    return [];
  }

  // Probe candidates concurrently without redispatching work.
  const probes = candidates.map((candidate) => clientWeeb3NetworkId(candidate));
  if (await probes[0] === requiredNetworkId) {
    return [candidates[0]];
  }

  const remainingNetworkIds = await Promise.all(probes.slice(1));
  const match = remainingNetworkIds.findIndex(
    (networkId) => networkId === requiredNetworkId
  );
  if (match >= 0) {
    return [candidates[match + 1]];
  }

  return [];
}

async function requestClients(event, requestUrl, requiredNetworkId) {
  const eventClientId = event.clientId || "";
  const eventClient = eventClientId ? await self.clients.get(eventClientId) : null;
  const requestUrlObject = requestUrl ? new URL(requestUrl) : null;
  const directHlsRequest = requestUrlObject !== null && (
    canonicalFeedResource(requestUrlObject) !== null ||
    rawRouteMarkers().some(([marker, rawType]) =>
      rawType === "hls-bytes" && requestUrlObject.pathname.startsWith(marker)
    )
  );

  // Direct top-level HLS requests skip the redundant liveness probe.
  if (
    directHlsRequest &&
    eventClient &&
    isTopLevelClient(eventClient) &&
    clientInScope(eventClient)
  ) {
    return [eventClient];
  }

  const allClients = await self.clients.matchAll({
    includeUncontrolled: true,
    type: "window"
  });
  const candidates = [];
  const seen = new Set();
  const requestReference = requestUrlObject ? bzzReferenceFromUrl(requestUrlObject) : "";

  if (eventClient && isTopLevelClient(eventClient) && clientInScope(eventClient)) {
    pushUniqueClient(candidates, seen, eventClient);
  }

  if (requestReference) {
    for (const client of allClients) {
      if (
        isTopLevelClient(client) &&
        clientInScope(client) &&
        bzzReferenceFromUrl(new URL(client.url)) === requestReference
      ) {
        pushUniqueClient(candidates, seen, client);
      }
    }
  }

  for (const client of allClients) {
    if (isTopLevelClient(client) && isAppShellClient(client)) {
      pushUniqueClient(candidates, seen, client);
    }
  }

  for (const client of allClients) {
    if (isTopLevelClient(client) && clientInScope(client)) {
      pushUniqueClient(candidates, seen, client);
    }
  }

  return firstReadyClient(candidates, requiredNetworkId);
}

function closeMessagePort(port) {
  try {
    port.close();
  } catch (_) {
  }
}

function messageClient(client, message, timeoutMs = FETCH_TIMEOUT_MS) {
  return new Promise((resolve) => {
    if (!client || typeof client.postMessage !== "function") {
      resolve({ ok: false, status: 502, error: "weeb-3 client is not available" });
      return;
    }

    const channel = new MessageChannel();
    let settled = false;
    let timer = null;

    const settle = (value) => {
      if (settled) {
        return;
      }
      settled = true;
      if (timer !== null) {
        clearTimeout(timer);
      }
      closeMessagePort(channel.port1);
      resolve(value || { ok: false, status: 500, error: "empty weeb-3 response" });
    };

    timer = setTimeout(() => {
      settle({ ok: false, status: 504, error: "Timed out waiting for weeb-3" });
    }, timeoutMs);

    channel.port1.onmessage = (event) => {
      settle(event.data);
    };

    try {
      client.postMessage(message, [channel.port2]);
    } catch (error) {
      settle({
        ok: false,
        status: 502,
        error: error && error.message ? error.message : "failed to message weeb-3"
      });
    }
  });
}

function messageFirstClient(clients, message, timeoutMs = FETCH_TIMEOUT_MS) {
  if (!clients.length) {
    const networkId = Number(message?.networkId);
    return Promise.resolve({
      ok: false,
      status: 503,
      error: Number.isSafeInteger(networkId)
        ? `weeb-3 runtime for Swarm network ${networkId} is not available`
        : "weeb-3 runtime is not available"
    });
  }

  // Timeouts detach the port; dispatched accounting work is never replayed.
  return messageClient(clients[0], message, timeoutMs);
}

function hlsRequestFlightKey(
  clients,
  requestUrl,
  method,
  range,
  ifNoneMatch,
  ifRange
) {
  if (!clients.length || !clients[0] || !clients[0].id) {
    return null;
  }

  const url = new URL(requestUrl);
  const hlsRoute = rawRouteMarkers().some(([marker, rawType]) =>
    rawType === "hls-bytes" && url.pathname.startsWith(marker)
  );
  const hlsAsset = hlsRoute && canonicalRawResource(url) !== null;
  const feedManifest = canonicalFeedResource(url) !== null;
  if (!hlsAsset && !feedManifest) {
    return null;
  }

  return [
    clients[0].id,
    method.toUpperCase(),
    url.pathname.toLowerCase(),
    url.search,
    range.trim().toLowerCase(),
    ifNoneMatch.trim(),
    ifRange.trim()
  ].join("|");
}

function requestRustFetch(
  clients,
  requestUrl,
  method,
  range,
  networkId,
  ifNoneMatch = "",
  ifRange = ""
) {
  const key = hlsRequestFlightKey(
    clients,
    requestUrl,
    method,
    range,
    ifNoneMatch,
    ifRange
  );
  if (!key) {
    return messageFirstClient(clients, {
      type: "WEEB3_FETCH_REQUEST",
      url: requestUrl,
      method,
      range,
      networkId,
      ifNoneMatch,
      ifRange
    });
  }

  const existing = HLS_REQUEST_FLIGHTS.get(key);
  if (existing) {
    return existing;
  }

  const pending = messageFirstClient(clients, {
    type: "WEEB3_FETCH_REQUEST",
    url: requestUrl,
    method,
    range,
    networkId,
    ifNoneMatch,
    ifRange
  });
  let tracked;
  tracked = pending.finally(() => {
    if (HLS_REQUEST_FLIGHTS.get(key) === tracked) {
      HLS_REQUEST_FLIGHTS.delete(key);
    }
  });
  HLS_REQUEST_FLIGHTS.set(key, tracked);
  return tracked;
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

function responseHeaders(headerRows) {
  const headers = new Headers();
  for (const row of headerRows || []) {
    if (row && row.length >= 2) {
      headers.set(String(row[0]), String(row[1]));
    }
  }
  return headers;
}

function requestRustRange(clients, url, start, end, networkId) {
  return messageFirstClient(clients, {
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
}

function createRustRangeStream(clients, url, size, networkId) {
  let position = 0;
  let schedulePosition = 0;
  const scheduled = new Map();

  const scheduleMore = () => {
    while (schedulePosition < size && scheduled.size < STREAM_LOOKAHEAD_CHUNKS) {
      const start = schedulePosition;
      const end = Math.min(start + STREAM_STORAGE_WINDOW_BYTES - 1, size - 1);
      scheduled.set(start, requestRustRange(clients, url, start, end, networkId));
      schedulePosition = end + 1;
    }
  };

  return new ReadableStream({
    async pull(controller) {
      try {
        if (position >= size) {
          controller.close();
          return;
        }

        scheduleMore();
        const start = Math.floor(position / STREAM_STORAGE_WINDOW_BYTES) * STREAM_STORAGE_WINDOW_BYTES;
        const request = scheduled.get(start);
        if (!request) {
          controller.error(new Error("weeb-3 stream window was not scheduled"));
          return;
        }

        const response = await request;
        scheduled.delete(start);
        if (!response || !response.ok) {
          controller.error(new Error(response && response.error ? response.error : "weeb-3 range request failed"));
          return;
        }

        const body = toUint8Array(response.body);
        position = start + body.byteLength;
        controller.enqueue(body);
        scheduleMore();
      } catch (error) {
        controller.error(error);
      }
    },
    cancel() {
      scheduled.clear();
    }
  });
}

async function forwardRequestToRust(request, event) {
  try {
    const url = new URL(request.url);
    const networkId = canonicalRouteNetworkId(url.pathname);
    if (networkId === null) {
      return new Response("weeb-3 route is outside the configured scope", { status: 400 });
    }
    const clients = await requestClients(event, request.url, networkId);
    const response = await requestRustFetch(
      clients,
      request.url,
      request.method,
      request.headers.get("Range") || "",
      networkId,
      request.headers.get("If-None-Match") || "",
      request.headers.get("If-Range") || ""
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
      return new Response(createRustRangeStream(clients, request.url, size, networkId), {
        status,
        headers
      });
    }

    return new Response(
      request.method === "HEAD" || status === 304
        ? null
        : toUint8Array(response.body),
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

async function forwardUploadToRust(request, event) {
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

    const clients = await requestClients(event, request.url, networkId);
    const response = await messageFirstClient(clients, {
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
