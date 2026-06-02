const CACHE_NAME = "default9";
const SCOPE = new URL(self.registration.scope);
const SCOPE_PATH = SCOPE.pathname.endsWith("/") ? SCOPE.pathname : `${SCOPE.pathname}/`;
const APP_ROOT = SCOPE_PATH;
const APP_INDEX = `${SCOPE_PATH}index.html`;
const BZZ_MARKER = `${SCOPE_PATH}bzz/`;
const BYTES_MARKER = `${SCOPE_PATH}bytes/`;
const CHUNKS_MARKER = `${SCOPE_PATH}chunks/`;
const CHUNK_MARKER = `${SCOPE_PATH}chunk/`;
const FETCH_TIMEOUT_MS = 240000;
const SERVICE_WORKER_MARKER = "forwarder-default9";
const DEBUG_SERVICE_WORKER = false;
const MIB_BYTES = 1024 * 1024;
const STREAM_STORAGE_WINDOW_BYTES = MIB_BYTES / 2;
const STREAM_LOOKAHEAD_CHUNKS = 8;

console.log(`weeb-3 service worker start ${SERVICE_WORKER_MARKER}`);

function debugLog(...args) {
  if (DEBUG_SERVICE_WORKER) {
    console.log(...args);
  }
}

function bytesToHex(bytes) {
  return Array.from(bytes)
    .map((byte) => byte.toString(16).padStart(2, "0"))
    .join("");
}

async function serviceWorkerVersion() {
  try {
    const response = await fetch(self.location.href, { cache: "no-store" });
    const body = await response.arrayBuffer();
    const digest = await crypto.subtle.digest("SHA-256", body);
    return `${SERVICE_WORKER_MARKER}-${bytesToHex(new Uint8Array(digest)).slice(0, 16)}`;
  } catch (_) {
    return SERVICE_WORKER_MARKER;
  }
}

async function logServiceWorkerVersion(reason) {
  console.log(`weeb-3 service worker ${reason} ${await serviceWorkerVersion()}`);
}

function isSwarmReference(reference) {
  return /^(?:[a-fA-F0-9]{64}|[a-fA-F0-9]{128})$/.test(reference);
}

function canonicalBzzResource(url) {
  const idx = url.pathname.indexOf(BZZ_MARKER);
  if (idx < 0) {
    return null;
  }

  const resource = url.pathname.substring(idx + BZZ_MARKER.length);
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

function canonicalRawResource(url) {
  for (const marker of [BYTES_MARKER, CHUNKS_MARKER, CHUNK_MARKER]) {
    const idx = url.pathname.indexOf(marker);
    if (idx < 0) {
      continue;
    }

    const resource = url.pathname.substring(idx + marker.length);
    if (!resource) {
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

function hasCanonicalBzzPath(resource) {
  const slash = resource.indexOf("/");
  return slash >= 0 && resource.substring(slash + 1).trim().length > 0;
}

function isCanonicalRequest(request) {
  try {
    const url = new URL(request.url);
    return canonicalBzzResource(url) !== null || canonicalRawResource(url) !== null;
  } catch (_) {
    return false;
  }
}

function isAppShellNavigation(request) {
  const headerDestination = request.headers.get("Sec-Fetch-Dest") || "";
  return request.mode === "navigate" &&
    request.destination !== "iframe" &&
    request.destination !== "frame" &&
    headerDestination !== "iframe" &&
    headerDestination !== "frame";
}

async function cacheAppShellResponse(cache, request, response) {
  if (!response.ok || isCanonicalRequest(request)) {
    return;
  }

  await cache.put(request, response.clone());

  const url = new URL(request.url);
  if (url.pathname === APP_ROOT || url.pathname === APP_INDEX) {
    await cache.put(new URL(APP_ROOT, self.registration.scope).toString(), response.clone());
    await cache.put(new URL(APP_INDEX, self.registration.scope).toString(), response.clone());
  }
}

async function networkFirst(request) {
  const cache = await caches.open(CACHE_NAME);

  try {
    const fetched = await fetch(request);
    await cacheAppShellResponse(cache, request, fetched);
    return fetched;
  } catch (_) {
    const cached = await cache.match(request);
    if (cached) {
      return cached;
    }
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
  event.waitUntil((async () => {
    await logServiceWorkerVersion("install");
    const cache = await caches.open(CACHE_NAME);
    await cache.addAll([APP_ROOT, APP_INDEX]);
    await self.skipWaiting();
  })());
});

self.addEventListener("activate", (event) => {
  event.waitUntil((async () => {
    await logServiceWorkerVersion("activate");
    const cacheNames = await caches.keys();
    await Promise.all(cacheNames.map((name) => {
      if (name !== CACHE_NAME) {
        return caches.delete(name);
      }
      return Promise.resolve(false);
    }));
    await self.clients.claim();
  })());
});

self.addEventListener("fetch", (event) => {
  const request = event.request;
  const url = new URL(request.url);

  if (request.method === "POST" && url.origin === SCOPE.origin && url.pathname.endsWith(`${SCOPE_PATH}bzz`)) {
    event.respondWith(forwardUploadToRust(request, event));
    return;
  }

  if (url.origin !== SCOPE.origin) {
    event.respondWith(fetch(request));
    return;
  }

  const bzzResource = canonicalBzzResource(url);
  if (bzzResource && (request.method === "GET" || request.method === "HEAD")) {
    if (isAppShellNavigation(request) && !hasCanonicalBzzPath(bzzResource)) {
      event.respondWith(networkFirst(appShellRequest(request)));
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

  event.respondWith(networkFirst(request));
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

async function requestClients(event, requestUrl) {
  const allClients = await self.clients.matchAll({
    includeUncontrolled: true,
    type: "window"
  });
  const clients = [];
  const seen = new Set();

  const eventClientId = event.clientId || event.resultingClientId || "";
  const eventClient = eventClientId ? await self.clients.get(eventClientId) : null;
  const requestReference = requestUrl ? bzzReferenceFromUrl(new URL(requestUrl)) : "";

  if (eventClient && isTopLevelClient(eventClient) && clientInScope(eventClient)) {
    pushUniqueClient(clients, seen, eventClient);
  }

  if (requestReference) {
    for (const client of allClients) {
      if (
        isTopLevelClient(client) &&
        clientInScope(client) &&
        bzzReferenceFromUrl(new URL(client.url)) === requestReference
      ) {
        pushUniqueClient(clients, seen, client);
      }
    }
  }

  for (const client of allClients) {
    if (isTopLevelClient(client) && isAppShellClient(client)) {
      pushUniqueClient(clients, seen, client);
    }
  }

  for (const client of allClients) {
    if (isTopLevelClient(client) && clientInScope(client)) {
      pushUniqueClient(clients, seen, client);
    }
  }

  if (eventClient && eventClient.frameType !== "nested") {
    pushUniqueClient(clients, seen, eventClient);
  }

  for (const client of allClients) {
    if (isTopLevelClient(client) && clientInScope(client)) {
      pushUniqueClient(clients, seen, client);
    }
  }

  for (const client of allClients) {
    if (isTopLevelClient(client)) {
      pushUniqueClient(clients, seen, client);
    }
  }

  return clients;
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

function messageClients(clients, message, timeoutMs = FETCH_TIMEOUT_MS) {
  if (!clients.length) {
    return Promise.resolve({ ok: false, status: 502, error: "weeb-3 client is not available" });
  }

  return new Promise((resolve) => {
    let settled = false;
    let finished = 0;
    let firstResponse = null;
    let bestError = null;

    const finish = (response) => {
      if (settled) {
        return;
      }
      settled = true;
      resolve(response);
    };

    for (const client of clients) {
      messageClient(client, message, timeoutMs).then((response) => {
        finished += 1;
        if (!firstResponse) {
          firstResponse = response;
        }
        if (response && response.ok) {
          finish(response);
          return;
        }
        if (response && response.status !== 502 && response.status !== 504 && !bestError) {
          bestError = response;
        }
        if (finished === clients.length) {
          finish(bestError || firstResponse || response || {
            ok: false,
            status: 502,
            error: "weeb-3 request failed"
          });
        }
      });
    }
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

function responseHeaders(headerRows) {
  const headers = new Headers();
  for (const row of headerRows || []) {
    if (row && row.length >= 2) {
      headers.set(String(row[0]), String(row[1]));
    }
  }
  return headers;
}

function requestRustRange(clients, url, start, end) {
  return messageClients(clients, {
    type: "WEEB3_FETCH_REQUEST",
    url,
    method: "GET",
    range: `bytes=${start}-${end}`
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

function createRustRangeStream(clients, url, size) {
  let position = 0;
  let schedulePosition = 0;
  const scheduled = new Map();

  const scheduleMore = () => {
    while (schedulePosition < size && scheduled.size < STREAM_LOOKAHEAD_CHUNKS) {
      const start = schedulePosition;
      const end = Math.min(start + STREAM_STORAGE_WINDOW_BYTES - 1, size - 1);
      scheduled.set(start, requestRustRange(clients, url, start, end));
      schedulePosition = end + 1;
    }
  };

  return new ReadableStream({
    async pull(controller) {
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
    },
    cancel() {
      scheduled.clear();
    }
  });
}

async function forwardRequestToRust(request, event) {
  const clients = await requestClients(event, request.url);
  const response = await messageClients(clients, {
    type: "WEEB3_FETCH_REQUEST",
    url: request.url,
    method: request.method,
    range: request.headers.get("Range") || ""
  });

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
    return new Response(createRustRangeStream(clients, request.url, size), {
      status,
      headers
    });
  }

  return new Response(request.method === "HEAD" ? null : toUint8Array(response.body), {
    status,
    headers
  });
}

async function forwardUploadToRust(request, event) {
  const url = new URL(request.url);
  const formData = await request.formData();
  const file = formData.get("file");

  if (!(file instanceof File)) {
    return new Response("No file in form data", { status: 400 });
  }

  const clients = await requestClients(event, request.url);
  const response = await messageClients(clients, {
    type: "UPLOAD_REQUEST",
    file,
    encryption: request.headers.get("swarm-encrypt") === "true",
    indexString: request.headers.get("swarm-index-document") || "",
    addToFeed: request.headers.get("swarm-collection") === "true",
    feedTopic: url.searchParams.get("feedTopic") || ""
  });

  if (!response.ok) {
    return new Response("Upload failed", { status: Number(response.status || 500) });
  }

  return new Response(JSON.stringify({ reference: response.reference || "" }), {
    status: 201,
    headers: { "Content-Type": "application/json" }
  });
}
