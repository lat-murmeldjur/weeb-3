const CACHE_NAME = "default3";
const BZZ_MARKER = "/weeb-3/bzz/";
const BYTES_MARKER = "/weeb-3/bytes/";
const CHUNKS_MARKER = "/weeb-3/chunks/";
const CHUNK_MARKER = "/weeb-3/chunk/";
const MIB_BYTES = 1024 * 1024;
const STREAM_STORAGE_WINDOW_BYTES = MIB_BYTES / 2;
const STREAM_RESPONSE_BUFFER_BYTES = 8 * MIB_BYTES;
const STREAM_ACTIVE_RESPONSE_BUFFER_BYTES = 2 * MIB_BYTES;
const STREAM_PREFETCH_AHEAD_LIMIT_BYTES = 64 * MIB_BYTES;
const RANGE_CACHE_MEMORY_RATIO = 0.75;
const RANGE_CACHE_DEVICE_MEMORY_RATIO = 0.45;
const RANGE_CACHE_FALLBACK_MAX_BYTES = 3 * 1024 * MIB_BYTES;
const RANGE_CACHE_MIN_MAX_BYTES = 512 * MIB_BYTES;
const METADATA_CACHE_MAX_ENTRIES = 1024;
const MEDIA_STREAM_STATE_MAX_ENTRIES = 64;
const CLIENT_MESSAGE_TIMEOUT_MS = 120000;
const RANGE_MESSAGE_TIMEOUT_MS = 240000;
const RANGE_CACHE_STALE_MS = RANGE_MESSAGE_TIMEOUT_MS + 30000;
const RANGE_RETRY_COUNT = 4;
const RANGE_RETRY_DELAY_MS = 700;
const DEBUG_SERVICE_WORKER = false;
const STREAM_PREFETCH_STAGE_BYTES = [
  4 * MIB_BYTES,
  4 * MIB_BYTES,
  4 * MIB_BYTES,
  4 * MIB_BYTES,
  8 * MIB_BYTES,
  8 * MIB_BYTES,
  8 * MIB_BYTES,
  8 * MIB_BYTES,
  16 * MIB_BYTES
];
const STREAM_SEEK_KEEP_AHEAD_BYTES = 16 * MIB_BYTES;
const STREAM_SEEK_RESET_GAP_BYTES = STREAM_SEEK_KEEP_AHEAD_BYTES;
const STREAM_SEEK_REQUEST_GAP_BYTES = 6 * MIB_BYTES;
const STREAM_PREFETCH_MAX_WINDOWS = 8;
const STREAM_INITIAL_LOOKAHEAD_CHUNKS = 8;
const STREAM_LOOKAHEAD_CHUNKS = 8;
const metadataCache = new Map();
const rangeCache = new Map();
const mediaStreamStates = new Map();

function debugLog(...args) {
  if (DEBUG_SERVICE_WORKER) {
    console.log(...args);
  }
}

function isCanonicalBzzRequest(request) {
  try {
    const url = new URL(request.url);
    return canonicalBzzResource(url) !== null || canonicalRawResource(url) !== null;
  } catch (_) {
    return false;
  }
}

const putInCache = async (request, response) => {
  if (isCanonicalBzzRequest(request)) {
    debugLog("Skipped bzz Cache API write:", request.url);
    return;
  }

  const cache = await caches.open(CACHE_NAME);
  await cache.put(request, response);
  debugLog("Cached:", request.url);
};

async function purgeBzzCacheEntries() {
  const cache = await caches.open(CACHE_NAME);
  const requests = await cache.keys();
  let purged = 0;

  await Promise.all(requests.map(async (request) => {
    if (isCanonicalBzzRequest(request)) {
      await cache.delete(request);
      purged += 1;
    }
  }));

  if (purged > 0) {
    debugLog("Purged bzz Cache API entries:", purged);
  }
}

function clearBzzMemoryCaches(reason) {
  const metadataCount = metadataCache.size;
  const rangeCount = rangeCache.size;
  const mediaStateCount = mediaStreamStates.size;

  metadataCache.clear();
  rangeCache.clear();
  mediaStreamStates.clear();

  if (metadataCount > 0 || rangeCount > 0 || mediaStateCount > 0) {
    debugLog(
      "Cleared bzz memory caches:",
      reason,
      `metadata=${metadataCount}`,
      `ranges=${rangeCount}`,
      `media_states=${mediaStateCount}`
    );
  }
}

self.addEventListener("message", async function(event) {
  const port = event.ports && event.ports[0];

  if (!event.data || !event.data.data0) {
    return;
  }

  const asset0 = new Blob([event.data.data0[0]], { type: event.data.mime0 });
  const reqHeaders = new Headers();
  reqHeaders.set("Cache-Control", "public, max-age=60000000000000");

  const encodedPath = encodeURI(event.data.path0);
  const request0 = new Request(encodedPath, { headers: reqHeaders });
  const response0 = new Response(asset0, {
    headers: {
      "Content-Type": event.data.mime0,
      "Content-Length": event.data.data0[0].length
    }
  });

  await putInCache(request0, response0);

  if (port) {
    port.postMessage({
      type: "CACHE_RESPONSE",
      cached: event.data.path0
    });
  }

  debugLog("Message event processed:", event);
});

async function cacheAppShellResponse(cache, request, response) {
  if (!response.ok || isCanonicalBzzRequest(request)) {
    return;
  }

  await cache.put(request, response.clone());

  const url = new URL(request.url);
  if (url.pathname === "/weeb-3/" || url.pathname === "/weeb-3/index.html") {
    await cache.put(new URL("/weeb-3/", self.registration.scope).toString(), response.clone());
    await cache.put(
      new URL("/weeb-3/index.html", self.registration.scope).toString(),
      response.clone()
    );
  }
}

const networkFirst = async (request) => {
  const cache = await caches.open(CACHE_NAME);

  try {
    const fetched = await fetch(request);
    await cacheAppShellResponse(cache, request, fetched);
    return fetched;
  } catch (e) {
    const responseFromCache = await cache.match(request);
    if (responseFromCache) {
      return responseFromCache;
    }
    return new Response("network fetch failed", { status: 502 });
  }
};

function isAppShellNavigation(request) {
  const headerDestination = request.headers.get("Sec-Fetch-Dest") || "";
  return request.mode === "navigate" &&
    request.destination !== "iframe" &&
    request.destination !== "frame" &&
    headerDestination !== "iframe" &&
    headerDestination !== "frame";
}

self.addEventListener("install", (event) => {
  debugLog("install");
  event.waitUntil(
    (async () => {
      const cache = await caches.open(CACHE_NAME);
      await cache.addAll(["/weeb-3/", "/weeb-3/index.html"]);
      await self.skipWaiting();
    })()
  );
});

self.addEventListener("activate", event => {
  debugLog("service activated, claim client");
  event.waitUntil((async () => {
    const cacheNames = await caches.keys();
    await Promise.all(cacheNames.map((name) => {
      if (name !== CACHE_NAME) {
        return caches.delete(name);
      }
      return Promise.resolve(false);
    }));
    clearBzzMemoryCaches("activate");
    await purgeBzzCacheEntries();
    await self.clients.claim();
  })());
});

self.addEventListener("fetch", (event) => {
  debugLog("fetch mode:", event.request.mode, "url:", event.request.url);

  const req = event.request;
  const url = new URL(req.url);
  const isReloadNavigation =
    req.mode === "navigate" &&
    (req.cache === "reload" ||
      (req.headers.get("Cache-Control") || "").includes("no-cache"));

  debugLog("SW FETCH:", req.method, url.pathname, "scope:", self.registration.scope);

  if (isReloadNavigation) {
    clearBzzMemoryCaches("navigate reload");
  }

  if (req.method === "POST" && url.pathname.endsWith("/weeb-3/bzz")) {
    debugLog("Intercepting POST upload:", url.toString());
    event.respondWith(postToLibRs(req, event));
    return;
  }

  if (url.origin !== new URL(self.registration.scope).origin) {
    event.respondWith(fetch(req));
    return;
  }

  const bzzResource = canonicalBzzResource(url);
  if (
    bzzResource &&
    !isAppShellNavigation(req) &&
    (req.method === "GET" || req.method === "HEAD")
  ) {
    event.respondWith(handleCanonicalBzz(req, event, bzzResource));
    return;
  }

  const rawResource = canonicalRawResource(url);
  if (
    rawResource &&
    !isAppShellNavigation(req) &&
    (req.method === "GET" || req.method === "HEAD")
  ) {
    event.respondWith(handleCanonicalRaw(req, event, rawResource));
    return;
  }

  event.respondWith((async () => {
    if (req.mode === "navigate") {
      debugLog("navigate attempt for", req.url);
      return networkFirst(req);
    }

    const cache = await caches.open(CACHE_NAME);
    const responseFromCache = await cache.match(req);
    if (responseFromCache) {
      return responseFromCache;
    }

    try {
      const actualResource = await fetch(req);

      if (actualResource.ok) {
        return actualResource;
      }
    } catch (e) {
    }

    const client = await requestClient(event);

    debugLog("acquire attempt for", req.url);
    return await fetchFromLibRs(req, client);
  })());
});

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
  const markers = [
    { marker: BYTES_MARKER, type: "bytes" },
    { marker: CHUNKS_MARKER, type: "chunk" },
    { marker: CHUNK_MARKER, type: "chunk" }
  ];

  for (const { marker, type } of markers) {
    const idx = url.pathname.indexOf(marker);
    if (idx < 0) {
      continue;
    }

    const resource = url.pathname.substring(idx + marker.length);
    if (!resource) {
      return null;
    }

    try {
      return { type, reference: decodeURIComponent(resource) };
    } catch (_) {
      return { type, reference: resource };
    }
  }

  return null;
}

function isSwarmReference(reference) {
  return /^(?:[a-fA-F0-9]{64}|[a-fA-F0-9]{128})$/.test(reference);
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

async function requestClient(event) {
  if (event.clientId) {
    const client = await self.clients.get(event.clientId);
    if (client) {
      return client;
    }
  }

  const allClients = await self.clients.matchAll({
    includeUncontrolled: false,
    type: "window"
  });

  return allClients[0] || null;
}

function closeMessagePort(port) {
  try {
    port.close();
  } catch (_) {
  }
}

function messageClient(client, message, timeoutMs = CLIENT_MESSAGE_TIMEOUT_MS) {
  return new Promise((resolve) => {
    if (!client || typeof client.postMessage !== "function") {
      resolve({ ok: false, error: "weeb-3 client is not available" });
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
      resolve(value);
    };

    timer = setTimeout(() => {
      settle({ ok: false, error: "Timed out waiting for weeb-3" });
    }, timeoutMs);

    channel.port1.onmessage = (event) => {
      settle(event.data || { ok: false });
    };

    try {
      client.postMessage(message, [channel.port2]);
    } catch (error) {
      settle({
        ok: false,
        error: error && error.message ? error.message : "Failed to message weeb-3"
      });
    }
  });
}

async function resolveBzz(resource, client) {
  const cached = metadataCache.get(resource);
  if (cached) {
    metadataCache.delete(resource);
    metadataCache.set(resource, cached);
    return cached;
  }

  const response = await messageClient(client, {
    type: "RESOLVE_BZZ_REQUEST",
    url: resource
  });

  if (!response.ok) {
    return null;
  }

  const metadata = {
    data_reference: response.data_reference || "",
    mime: response.mime || "application/octet-stream",
    size: Number(response.size || 0),
    etag: response.etag || `"${response.data_reference || resource}"`,
    path: response.path || ""
  };

  metadataCache.set(resource, metadata);
  trimMetadataCache();
  return metadata;
}

function trimMetadataCache() {
  while (metadataCache.size > METADATA_CACHE_MAX_ENTRIES) {
    const first = metadataCache.keys().next().value;
    if (first === undefined) {
      break;
    }
    metadataCache.delete(first);
  }
}

async function retrieveRange(resource, start, end, client, metadata, generation = 0) {
  const message = {
    type: "RETRIEVE_RANGE_REQUEST",
    url: resource,
    start,
    end
  };

  if (metadata) {
    message.data_reference = metadata.data_reference || "";
    message.mime = metadata.mime || "application/octet-stream";
    message.size = metadata.size || 0;
    message.etag = metadata.etag || "";
    message.path = metadata.path || "";
  }

  if (metadata && generation > 0) {
    message.stream_key = mediaStateKey(resource, metadata);
    message.stream_generation = generation;
  }

  return messageClient(client, message, RANGE_MESSAGE_TIMEOUT_MS);
}

function metadataIdentity(resource, metadata) {
  return metadata.etag || metadata.data_reference || resource;
}

function mediaStateKey(resource, metadata) {
  return `${metadataIdentity(resource, metadata)}|${resource}`;
}

function rangeCacheKey(resource, start, end, metadata) {
  return `${metadataIdentity(resource, metadata)}|${metadata.size}|${start}-${end}`;
}

function deleteCachedRange(resource, start, end, metadata, generation) {
  rangeCache.delete(rangeCacheKey(resource, start, end, metadata, generation));
}

function invalidRangeResponse(message, body) {
  return {
    ok: false,
    error: message,
    body: body || new Uint8Array()
  };
}

function checkedRangeResponse(rangeResponse, start, end, metadata) {
  if (!rangeResponse || !rangeResponse.ok) {
    return invalidRangeResponse(
      rangeResponse && rangeResponse.error
        ? rangeResponse.error
        : "weeb-3 did not retrieve range"
    );
  }

  const body = toUint8Array(rangeResponse.body);
  const expectedLength = end - start + 1;
  if (body.byteLength !== expectedLength) {
    return {
      ...invalidRangeResponse(
        `weeb-3 returned ${body.byteLength} bytes for ${expectedLength} byte range`,
        body
      ),
      short: true
    };
  }

  const responseSize = Number(rangeResponse.size || metadata.size || 0);
  if (metadata.size && responseSize && responseSize !== metadata.size) {
    return invalidRangeResponse(
      `weeb-3 range metadata size mismatch ${responseSize} != ${metadata.size}`,
      body
    );
  }

  if (metadata.etag && rangeResponse.etag && rangeResponse.etag !== metadata.etag) {
    return invalidRangeResponse("weeb-3 range metadata etag mismatch", body);
  }

  return {
    ...rangeResponse,
    body
  };
}

function rangeCacheUsedBytes() {
  let bytes = 0;
  for (const entry of rangeCache.values()) {
    bytes += entry.bytes || 0;
  }
  return bytes;
}

function rangeCacheMaxBytes() {
  const memory = performance && performance.memory;
  if (memory && Number.isFinite(memory.jsHeapSizeLimit) && memory.jsHeapSizeLimit > 0) {
    return Math.max(
      RANGE_CACHE_MIN_MAX_BYTES,
      Math.floor(memory.jsHeapSizeLimit * RANGE_CACHE_MEMORY_RATIO)
    );
  }

  const deviceMemory = navigator && navigator.deviceMemory;
  if (Number.isFinite(deviceMemory) && deviceMemory > 0) {
    return Math.max(
      RANGE_CACHE_MIN_MAX_BYTES,
      Math.floor(deviceMemory * 1024 * MIB_BYTES * RANGE_CACHE_DEVICE_MEMORY_RATIO)
    );
  }

  return RANGE_CACHE_FALLBACK_MAX_BYTES;
}

function trimRangeCache() {
  const maxBytes = rangeCacheMaxBytes();
  let usedBytes = rangeCacheUsedBytes();
  if (usedBytes <= maxBytes) {
    return;
  }

  const before = rangeCache.size;
  const beforeBytes = usedBytes;
  let trimmed = 0;

  for (const [key, entry] of rangeCache) {
    if (usedBytes <= maxBytes) {
      break;
    }

    if (!entry.settledAt) {
      continue;
    }

    rangeCache.delete(key);
    usedBytes -= entry.bytes || 0;
    trimmed += 1;
  }

  if (trimmed > 0) {
    debugLog(
      "bzz media trimmed range cache",
      `removed=${trimmed}`,
      `before=${before}`,
      `after=${rangeCache.size}`,
      `bytes=${beforeBytes}->${usedBytes}`,
      `limit=${maxBytes}`
    );
  }

  if (usedBytes > maxBytes) {
    debugLog(
      "bzz media kept range cache above memory target to preserve in-flight ranges",
      `bytes=${usedBytes}`,
      `limit=${maxBytes}`
    );
  }
}

function rangeStorageWindowForStart(start, size) {
  const storageStart =
    Math.floor(start / STREAM_STORAGE_WINDOW_BYTES) * STREAM_STORAGE_WINDOW_BYTES;
  return {
    start: storageStart,
    end: Math.min(storageStart + STREAM_STORAGE_WINDOW_BYTES - 1, size - 1)
  };
}

function rangeStorageWindowsForSpan(start, end, size) {
  const windows = [];
  let position = start;

  while (position <= end) {
    const window = rangeStorageWindowForStart(position, size);
    windows.push(window);
    position = window.end + 1;
  }

  return windows;
}

function getMediaStreamState(resource, metadata) {
  const key = mediaStateKey(resource, metadata);
  let state = mediaStreamStates.get(key);
  if (!state) {
    state = {
      key,
      generation: 1,
      anchorStart: null,
      highWaterEnd: -1,
      scheduledHighWaterEnd: -1,
      completedRanges: new Map(),
      consecutiveFailures: 0,
      lastRequestStart: 0,
      lastRangeWasSeek: false,
      lastRangeWasStartup: false,
      prefetchRunning: false,
      prefetchGeneration: 0,
      lastTouch: Date.now()
    };
    mediaStreamStates.set(key, state);
    trimMediaStreamStates(key);
  }
  return state;
}

function trimMediaStreamStates(activeKey) {
  if (mediaStreamStates.size <= MEDIA_STREAM_STATE_MAX_ENTRIES) {
    return;
  }

  for (const [key, state] of mediaStreamStates) {
    if (mediaStreamStates.size <= MEDIA_STREAM_STATE_MAX_ENTRIES) {
      break;
    }
    if (key !== activeKey && !state.prefetchRunning) {
      mediaStreamStates.delete(key);
    }
  }
}

function effectiveMediaHighWaterEnd(state) {
  return Math.max(state.highWaterEnd, state.scheduledHighWaterEnd);
}

function markMediaRangeScheduled(state, end) {
  if (!state) {
    return;
  }

  state.scheduledHighWaterEnd = Math.max(state.scheduledHighWaterEnd, end);
  state.lastTouch = Date.now();
}

function markMediaRangeComplete(state, start, end) {
  if (!state) {
    return;
  }

  if (start <= state.highWaterEnd + 1) {
    state.highWaterEnd = Math.max(state.highWaterEnd, end);
  } else {
    state.completedRanges.set(start, Math.max(end, state.completedRanges.get(start) || -1));
  }

  let advanced = true;
  while (advanced) {
    advanced = false;
    for (const [rangeStart, rangeEnd] of state.completedRanges) {
      if (rangeStart <= state.highWaterEnd + 1) {
        state.highWaterEnd = Math.max(state.highWaterEnd, rangeEnd);
        state.completedRanges.delete(rangeStart);
        advanced = true;
      }
    }
  }

  state.scheduledHighWaterEnd = Math.max(state.scheduledHighWaterEnd, state.highWaterEnd);
  state.consecutiveFailures = 0;
  state.lastTouch = Date.now();
}

function noteMediaRangeFailure(state, start, reason) {
  if (!state) {
    return;
  }

  state.consecutiveFailures += 1;
  state.scheduledHighWaterEnd = Math.max(state.highWaterEnd, start - 1);
  state.lastTouch = Date.now();

  debugLog(
    "bzz media range failure",
    `start=${start}`,
    `generation=${state.generation}`,
    `failures=${state.consecutiveFailures}`,
    reason || "unknown"
  );
}

function rangeEntryMatchesResource(entry, resource, metadata) {
  return entry.identity === metadataIdentity(resource, metadata);
}

function discardMediaRangesOutside(resource, metadata, generation, keepStart, keepEnd) {
  let discarded = 0;
  for (const [key, entry] of rangeCache) {
    if (!rangeEntryMatchesResource(entry, resource, metadata)) {
      continue;
    }

    if (entry.settledAt && entry.ok) {
      continue;
    }

    if (
      entry.generation < generation ||
      entry.end < keepStart ||
      entry.start > keepEnd
    ) {
      rangeCache.delete(key);
      discarded += 1;
    }
  }

  if (discarded > 0) {
    debugLog(
      "bzz media canceled stale in-flight ranges",
      discarded,
      `keep=${keepStart}-${keepEnd}`,
      `generation=${generation}`
    );
  }
}

function beginMediaRange(resource, start, metadata) {
  const state = getMediaStreamState(resource, metadata);
  const previousAnchor = state.anchorStart;
  const previousHighWater = effectiveMediaHighWaterEnd(state);
  const previousRequestStart = state.lastRequestStart;
  const isStartup = previousAnchor === null;
  const isRequestJump =
    previousAnchor !== null &&
    (start + STREAM_SEEK_REQUEST_GAP_BYTES < previousRequestStart ||
      start > previousRequestStart + STREAM_SEEK_REQUEST_GAP_BYTES);
  const isSeek =
    previousAnchor !== null &&
    (isRequestJump ||
      start + STREAM_SEEK_RESET_GAP_BYTES < previousAnchor ||
      start > previousHighWater + STREAM_SEEK_RESET_GAP_BYTES);
  const isPrefetchRunaway =
    previousAnchor !== null &&
    previousHighWater >
      start + STREAM_RESPONSE_BUFFER_BYTES + STREAM_PREFETCH_AHEAD_LIMIT_BYTES;

  if (isSeek || isPrefetchRunaway) {
    state.generation += 1;
    state.anchorStart = start;
    state.highWaterEnd = start - 1;
    state.scheduledHighWaterEnd = start - 1;
    state.completedRanges.clear();
    state.consecutiveFailures = 0;
    discardMediaRangesOutside(
      resource,
      metadata,
      state.generation,
      Math.max(0, start - STREAM_RESPONSE_BUFFER_BYTES),
      Math.min(metadata.size - 1, start + STREAM_SEEK_KEEP_AHEAD_BYTES - 1)
    );
    debugLog(
      isSeek ? "bzz media seek reset" : "bzz media prefetch lead reset",
      `${start}/${metadata.size}`,
      `previous_high_water=${previousHighWater}`,
      `generation=${state.generation}`
    );
  } else if (isStartup) {
    state.anchorStart = start;
  }

  state.lastRequestStart = start;
  state.lastRangeWasSeek = isSeek || isPrefetchRunaway;
  state.lastRangeWasStartup = isStartup;
  state.lastTouch = Date.now();
  return state;
}

function cachedRangePromise(resource, start, end, client, metadata, generation = 0) {
  const key = rangeCacheKey(resource, start, end, metadata, generation);
  let entry = rangeCache.get(key);
  if (entry) {
    if (!entry.settledAt && Date.now() - entry.createdAt > RANGE_CACHE_STALE_MS) {
      rangeCache.delete(key);
      debugLog(
        "bzz media expired stale range",
        `${start}-${end}/${metadata.size}`,
        `generation=${generation}`
      );
    } else if (!entry.settledAt && entry.generation !== generation) {
      rangeCache.delete(key);
      debugLog(
        "bzz media replaced stale in-flight range",
        `${start}-${end}/${metadata.size}`,
        `old_generation=${entry.generation}`,
        `generation=${generation}`
      );
    } else if (entry.settledAt && !entry.ok) {
      rangeCache.delete(key);
    } else {
      entry.generation = generation;
      rangeCache.delete(key);
      rangeCache.set(key, entry);
      return entry.promise;
    }
  }

  const createdAt = Date.now();
  const promise = retrieveRange(resource, start, end, client, metadata, generation).then(
    (rangeResponse) => {
      const checked = checkedRangeResponse(rangeResponse, start, end, metadata);
      if (entry) {
        entry.settledAt = Date.now();
        entry.ok = checked.ok && !checked.short;
        entry.bytes = entry.ok ? checked.body.byteLength : 0;
      }
      if (!checked.ok) {
        rangeCache.delete(key);
      } else {
        trimRangeCache();
      }
      return checked;
    },
    (error) => {
      if (entry) {
        entry.settledAt = Date.now();
        entry.ok = false;
        entry.bytes = 0;
      }
      rangeCache.delete(key);
      return invalidRangeResponse(
        error && error.message ? error.message : "weeb-3 range request failed"
      );
    }
  );

  entry = {
    promise,
    resource,
    identity: metadataIdentity(resource, metadata),
    start,
    end,
    generation,
    createdAt,
    settledAt: null,
    ok: false,
    bytes: 0
  };
  rangeCache.set(key, entry);
  trimRangeCache();
  return promise;
}

async function readCachedRange(resource, start, end, client, metadata, generation = 0) {
  const windows = rangeStorageWindowsForSpan(start, end, metadata.size);
  const responses = await Promise.all(
    windows.map((window) =>
      cachedRangePromise(
        resource,
        window.start,
        window.end,
        client,
        metadata,
        generation
      )
    )
  );

  const body = new Uint8Array(end - start + 1);
  let offset = 0;

  for (let i = 0; i < windows.length; i += 1) {
    const window = windows[i];
    const rangeResponse = responses[i];
    if (!rangeResponse.ok) {
      deleteCachedRange(resource, window.start, window.end, metadata, generation);
      return {
        ok: false,
        short: !!rangeResponse.short,
        error: rangeResponse.error || "weeb-3 did not retrieve range"
      };
    }

    const storageBody = toUint8Array(rangeResponse.body);
    if (storageBody.byteLength !== window.end - window.start + 1) {
      deleteCachedRange(resource, window.start, window.end, metadata, generation);
      return { ok: true, short: true, body: storageBody };
    }

    const overlapStart = Math.max(start, window.start);
    const overlapEnd = Math.min(end, window.end);
    const localStart = overlapStart - window.start;
    const localEnd = overlapEnd - window.start;
    const slice = storageBody.slice(localStart, localEnd + 1);
    body.set(slice, offset);
    offset += slice.byteLength;
  }

  return { ok: true, body };
}

async function readCachedRangeWithRetry(resource, start, end, client, metadata, generation = 0) {
  let lastResponse = { ok: false, error: "range retry did not run" };

  for (let attempt = 0; attempt <= RANGE_RETRY_COUNT; attempt += 1) {
    lastResponse = await readCachedRange(resource, start, end, client, metadata, generation);
    if (lastResponse.ok && !lastResponse.short) {
      return lastResponse;
    }

    debugLog(
      "bzz media range retry",
      `${start}-${end}/${metadata.size}`,
      "attempt",
      attempt + 1,
      lastResponse.error || (lastResponse.short ? "short range" : "unknown")
    );

    if (attempt < RANGE_RETRY_COUNT) {
      await sleep(RANGE_RETRY_DELAY_MS * (attempt + 1));
    }
  }

  return lastResponse;
}

function retrieveRangeWindow(resource, start, end, client, metadata, generation = 0) {
  const storageWindow = rangeStorageWindowForStart(start, metadata.size);
  const windowEnd = Math.min(storageWindow.end, end, metadata.size - 1);
  const promise = cachedRangePromise(
    resource,
    storageWindow.start,
    windowEnd,
    client,
    metadata,
    generation
  );
  return { start: storageWindow.start, end: windowEnd, promise };
}

function prefetchMediaWindows(
  resource,
  responseRange,
  requestedEnd,
  client,
  metadata,
  state,
  options = {}
) {
  const generation = state ? state.generation : 0;
  const maxActive = options.maxActive || STREAM_PREFETCH_MAX_WINDOWS;
  const label = options.label || "prefetch";
  const targetEnd = Math.min(
    requestedEnd,
    metadata.size - 1,
    options.targetEnd ?? requestedEnd
  );
  let position = Math.max(responseRange.end + 1, (state ? state.highWaterEnd : -1) + 1);
  let scheduled = 0;

  if (state) {
    markMediaRangeComplete(state, responseRange.start, responseRange.end);
    markMediaRangeScheduled(state, responseRange.end);
  }

  return new Promise((resolve) => {
    let active = 0;
    let logged = false;
    let failed = false;

    const isCurrentGeneration = () => !state || state.generation === generation;

    const finishIfDone = () => {
      if (active === 0 && (!isCurrentGeneration() || failed || position > targetEnd)) {
        if (scheduled > 0) {
          debugLog(
            `bzz media ${label} done`,
            `${responseRange.end + 1}-${state ? state.highWaterEnd : targetEnd}/${metadata.size}`,
            `windows=${scheduled}`,
            `generation=${generation}`
          );
        }
        resolve();
      }
    };

    const launchMore = () => {
      if (!isCurrentGeneration()) {
        debugLog(
          `bzz media ${label} stopped stale`,
          `${responseRange.end + 1}-${targetEnd}/${metadata.size}`,
          `generation=${generation}`,
          `current=${state ? state.generation : generation}`
        );
        finishIfDone();
        return;
      }

      if (failed) {
        finishIfDone();
        return;
      }

      if (state) {
        position = Math.max(position, state.highWaterEnd + 1);
      }

      while (isCurrentGeneration() && position <= targetEnd && active < maxActive) {
        const window = retrieveRangeWindow(
          resource,
          position,
          targetEnd,
          client,
          metadata,
          generation
        );
        if (state) {
          markMediaRangeScheduled(state, window.end);
        }
        position = window.end + 1;
        scheduled += 1;
        active += 1;

        if (!logged) {
          logged = true;
          debugLog(
            `bzz media ${label}`,
            `${responseRange.end + 1}-${targetEnd}/${metadata.size}`,
            `max_active=${maxActive}`,
            `generation=${generation}`
          );
        }

        Promise.resolve(window.promise)
          .then(
            (rangeResponse) => {
              if (!isCurrentGeneration()) {
                return;
              }

              if (rangeResponse && rangeResponse.ok && !rangeResponse.short) {
                markMediaRangeComplete(state, window.start, window.end);
              } else {
                failed = true;
                noteMediaRangeFailure(
                  state,
                  window.start,
                  rangeResponse && rangeResponse.error
                    ? rangeResponse.error
                    : "prefetch range failed"
                );
              }
            },
            (error) => {
              if (!isCurrentGeneration()) {
                return;
              }

              failed = true;
              noteMediaRangeFailure(
                state,
                window.start,
                error && error.message ? error.message : "prefetch range rejected"
              );
            }
          )
          .finally(() => {
            active -= 1;
            launchMore();
            finishIfDone();
          });
      }

      finishIfDone();
    };

    if (position > targetEnd) {
      resolve();
      return;
    }

    launchMore();
  });
}

async function prefetchMediaStages(resource, responseRange, requestedEnd, client, metadata, state) {
  const generation = state ? state.generation : 0;
  const prefetchLimitEnd = Math.min(
    requestedEnd,
    responseRange.end + STREAM_PREFETCH_AHEAD_LIMIT_BYTES,
    metadata.size - 1
  );
  if (state) {
    if (state.prefetchRunning && state.prefetchGeneration === generation) {
      debugLog(
        "bzz media prefetch already running",
        `${responseRange.start}-${responseRange.end}/${metadata.size}`,
        `generation=${generation}`
      );
      return;
    }
    state.prefetchRunning = true;
    state.prefetchGeneration = generation;
  }

  try {
    for (let i = 0; i < STREAM_PREFETCH_STAGE_BYTES.length; i += 1) {
      if (state && state.generation !== generation) {
        return;
      }

      const stageBytes = STREAM_PREFETCH_STAGE_BYTES[i];
      const currentEnd = Math.max(
        responseRange.end,
        state ? state.highWaterEnd : responseRange.end
      );
      if (currentEnd >= prefetchLimitEnd || currentEnd >= metadata.size - 1) {
        return;
      }

      const targetEnd = Math.min(currentEnd + stageBytes, prefetchLimitEnd, metadata.size - 1);
      await prefetchMediaWindows(
        resource,
        responseRange,
        prefetchLimitEnd,
        client,
        metadata,
        state,
        {
          targetEnd,
          maxActive: STREAM_PREFETCH_MAX_WINDOWS,
          label: `prefetch stage ${i + 1} ${Math.round(stageBytes / MIB_BYTES)}MiB`
        }
      );
    }
  } finally {
    if (state && state.prefetchGeneration === generation) {
      state.prefetchRunning = false;
    }
  }
}

function keepPrefetchAlive(event, prefetchWork) {
  if (!prefetchWork || !event || typeof event.waitUntil !== "function") {
    return;
  }

  try {
    event.waitUntil(Promise.resolve(prefetchWork).then(() => undefined));
  } catch (_) {
  }
}

function retrieveRangeStream(resource, start, end, client, metadata) {
  let position = start;
  let schedulePosition = start;
  let deliveredFirstWindow = false;
  const scheduled = new Map();

  function scheduleLookahead() {
    const lookaheadLimit = deliveredFirstWindow
      ? STREAM_LOOKAHEAD_CHUNKS
      : STREAM_INITIAL_LOOKAHEAD_CHUNKS;

    while (
      schedulePosition <= end &&
      scheduled.size < lookaheadLimit
    ) {
      const window = retrieveRangeWindow(
        resource,
        schedulePosition,
        end,
        client,
        metadata
      );
      scheduled.set(window.start, window);
      schedulePosition = window.end + 1;
    }
  }

  return new ReadableStream({
    async pull(controller) {
      if (position > end) {
        controller.close();
        return;
      }

      scheduleLookahead();
      const window = scheduled.get(position) || retrieveRangeWindow(
        resource,
        position,
        end,
        client,
        metadata
      );
      const rangeResponse = await readCachedRangeWithRetry(
        resource,
        window.start,
        window.end,
        client,
        metadata
      );
      if (!rangeResponse.ok) {
        controller.error(new Error("weeb-3 did not retrieve range"));
        return;
      }

      const body = rangeResponse.body;
      if (rangeResponse.short) {
        controller.error(new Error("weeb-3 returned a short range"));
        return;
      }

      controller.enqueue(body);
      scheduled.delete(position);
      position = window.end + 1;
      deliveredFirstWindow = true;
      scheduleLookahead();
    }
  });
}

function metadataHeaders(metadata, length) {
  const headers = new Headers();
  headers.set("Accept-Ranges", "bytes");
  headers.set("Content-Length", String(length));
  headers.set("Content-Type", metadata.mime || "application/octet-stream");
  headers.set("ETag", metadata.etag);
  return headers;
}

function parseSingleRange(rangeHeader, size) {
  if (!rangeHeader) {
    return null;
  }

  const match = /^bytes=([^,]+)$/.exec(rangeHeader.trim());
  if (!match) {
    return { invalid: true };
  }

  const [rawStart, rawEnd] = match[1].split("-");
  let start;
  let end;
  if (rawStart === "") {
    const suffixLength = Number(rawEnd);
    if (!Number.isInteger(suffixLength) || suffixLength <= 0) {
      return { invalid: true };
    }
    start = Math.max(size - suffixLength, 0);
    end = size - 1;
  } else {
    start = Number(rawStart);
    if (rawEnd === "") {
      end = size - 1;
    } else {
      end = Number(rawEnd);
    }
  }

  if (
    !Number.isInteger(start) ||
    !Number.isInteger(end) ||
    start < 0 ||
    end < start ||
    start >= size
  ) {
    return { invalid: true };
  }

  return {
    start,
    end: Math.min(end, size - 1)
  };
}

function responseRangeForRequest(parsedRange, size, streamable, state) {
  let responseBytes = STREAM_STORAGE_WINDOW_BYTES;
  if (streamable) {
    const startupLike =
      state && (state.lastRangeWasStartup || state.lastRangeWasSeek);
    responseBytes = startupLike
      ? STREAM_RESPONSE_BUFFER_BYTES
      : STREAM_ACTIVE_RESPONSE_BUFFER_BYTES;
  }
  const responseEnd = Math.min(parsedRange.start + responseBytes - 1, parsedRange.end, size - 1);

  return {
    start: parsedRange.start,
    end: responseEnd
  };
}

function startupRangeForMetadata(metadata) {
  return {
    start: 0,
    end: Math.min(STREAM_RESPONSE_BUFFER_BYTES - 1, metadata.size - 1)
  };
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

async function handleCanonicalRaw(request, event, rawResource) {
  const parts = rawResource.reference.split("/");
  const reference = parts[0] || rawResource.reference;
  if (parts.slice(1).some((part) => part.length > 0)) {
    return new Response("raw route accepts one swarm reference", { status: 400 });
  }
  if (!isSwarmReference(reference)) {
    return new Response("invalid swarm reference", { status: 400 });
  }

  const headers = new Headers();
  headers.set("Content-Type", "application/octet-stream");
  headers.set("Content-Disposition", `attachment; filename="${reference}"`);
  headers.set("Cache-Control", "no-store");

  if (request.method === "HEAD") {
    return new Response(null, {
      status: 200,
      headers
    });
  }

  const client = await requestClient(event);
  if (!client) {
    return new Response("Request ghosted by client", { status: 502 });
  }

  const response = await messageClient(
    client,
    {
      type: rawResource.type === "chunk" ? "RETRIEVE_CHUNK_REQUEST" : "RETRIEVE_BYTES_REQUEST",
      url: reference
    },
    RANGE_MESSAGE_TIMEOUT_MS
  );

  if (!response || !response.ok) {
    return new Response("weeb-3 did not retrieve resource", { status: 404 });
  }

  const body = toUint8Array(response.body);
  headers.set("Content-Length", String(body.byteLength));

  return new Response(body, {
    status: 200,
    headers
  });
}

async function handleCanonicalBzz(request, event, resource) {
  const client = await requestClient(event);
  if (!client) {
    return new Response("Request ghosted by client", { status: 502 });
  }

  const metadata = await resolveBzz(resource, client);
  if (!metadata) {
    return new Response("weeb-3 did not resolve resource", { status: 404 });
  }

  if (request.method === "HEAD") {
    return new Response(null, {
      status: 200,
      headers: metadataHeaders(metadata, metadata.size)
    });
  }

  const streamable = isStreamableMime(metadata.mime) && metadata.size > 0;
  const parsedRange = parseSingleRange(request.headers.get("Range"), metadata.size);
  if (parsedRange && parsedRange.invalid) {
    return new Response(null, {
      status: 416,
      headers: {
        "Content-Range": `bytes */${metadata.size}`
      }
    });
  }

  if (parsedRange) {
    let mediaState = null;
    if (streamable) {
      mediaState = beginMediaRange(resource, parsedRange.start, metadata);
    }

    const responseRange = responseRangeForRequest(
      parsedRange,
      metadata.size,
      streamable,
      mediaState
    );
    const responseStorageWindows = rangeStorageWindowsForSpan(
      responseRange.start,
      responseRange.end,
      metadata.size
    );
    debugLog(
      "bzz media range",
      request.headers.get("Range"),
      "=>",
      `${responseRange.start}-${responseRange.end}/${metadata.size}`
    );
    for (const responseStorage of responseStorageWindows) {
      if (mediaState) {
        markMediaRangeScheduled(mediaState, responseStorage.end);
      }
      cachedRangePromise(
        resource,
        responseStorage.start,
        responseStorage.end,
        client,
        metadata,
        mediaState ? mediaState.generation : 0
      );
    }
    const rangeResponse = await readCachedRangeWithRetry(
      resource,
      responseRange.start,
      responseRange.end,
      client,
      metadata,
      mediaState ? mediaState.generation : 0
    );

    if (!rangeResponse.ok) {
      noteMediaRangeFailure(
        mediaState,
        responseRange.start,
        rangeResponse.error || "weeb-3 did not retrieve range"
      );
      return new Response(rangeResponse.error || "weeb-3 did not retrieve range", { status: 503 });
    }

    const body = rangeResponse.body;
    if (rangeResponse.short) {
      noteMediaRangeFailure(mediaState, responseRange.start, "weeb-3 returned a short range");
      return new Response("weeb-3 returned a short range", { status: 502 });
    }

    if (streamable) {
      markMediaRangeComplete(mediaState, responseRange.start, responseRange.end);
      keepPrefetchAlive(
        event,
        prefetchMediaStages(resource, responseRange, parsedRange.end, client, metadata, mediaState)
      );
    }

    const headers = metadataHeaders(metadata, body.byteLength);
    headers.set(
      "Content-Range",
      `bytes ${responseRange.start}-${responseRange.end}/${metadata.size}`
    );

    return new Response(body, {
      status: 206,
      headers
    });
  }

  if (streamable) {
    const mediaState = beginMediaRange(resource, 0, metadata);

    const startupRange = startupRangeForMetadata(metadata);
    const startupStorageWindows = rangeStorageWindowsForSpan(
      startupRange.start,
      startupRange.end,
      metadata.size
    );
    debugLog(
      "bzz media startup range",
      `${startupRange.start}-${startupRange.end}/${metadata.size}`
    );
    for (const startupStorage of startupStorageWindows) {
      markMediaRangeScheduled(mediaState, startupStorage.end);
      cachedRangePromise(
        resource,
        startupStorage.start,
        startupStorage.end,
        client,
        metadata,
        mediaState.generation
      );
    }
    const rangeResponse = await readCachedRangeWithRetry(
      resource,
      startupRange.start,
      startupRange.end,
      client,
      metadata,
      mediaState.generation
    );

    if (!rangeResponse.ok) {
      noteMediaRangeFailure(
        mediaState,
        startupRange.start,
        rangeResponse.error || "weeb-3 did not retrieve startup range"
      );
      return new Response(rangeResponse.error || "weeb-3 did not retrieve startup range", {
        status: 503
      });
    }

    const body = rangeResponse.body;
    if (rangeResponse.short) {
      noteMediaRangeFailure(mediaState, startupRange.start, "weeb-3 returned a short startup range");
      return new Response("weeb-3 returned a short startup range", { status: 502 });
    }

    markMediaRangeComplete(mediaState, startupRange.start, startupRange.end);
    keepPrefetchAlive(
      event,
      prefetchMediaStages(resource, startupRange, metadata.size - 1, client, metadata, mediaState)
    );

    const headers = metadataHeaders(metadata, body.byteLength);
    headers.set(
      "Content-Range",
      `bytes ${startupRange.start}-${startupRange.end}/${metadata.size}`
    );

    return new Response(body, {
      status: 206,
      headers
    });
  }

  const stream = retrieveRangeStream(resource, 0, metadata.size - 1, client, metadata);

  return new Response(stream, {
    status: 200,
    headers: metadataHeaders(metadata, metadata.size)
  });
}

function isStreamableMime(mime) {
  return typeof mime === "string" && (mime.startsWith("video/") || mime.startsWith("audio/"));
}

const fetchFromLibRs = async (request, client) => {
  const allClients = await self.clients.matchAll({ includeUncontrolled: true, type: "window" });
  debugLog("all clients", allClients.map(c => c.id));
  debugLog("actual client", client ? client.id : "none");

  if (!client) {
    return new Response("Request ghosted by client", { status: 502 });
  }

  const url = new URL(request.url);
  let resource = url.pathname;
  const idx = resource.indexOf(BZZ_MARKER);
  if (idx >= 0) {
    resource = resource.substring(idx + BZZ_MARKER.length);
  }

  return new Promise((resolve) => {
    const channel = new MessageChannel();
    channel.port1.onmessage = async (event) => {
      const { ok, body, mime, path } = event.data;
      debugLog("Message from interface:", {
        ok,
        bodyType: body ? body.constructor.name : body,
        bodyLen: body && body.length ? body.length : 0,
        mime,
        path
      });
      if (ok && body) {
        const response = new Response(new Blob([body], { type: mime }), {
          headers: { "Content-Type": mime }
        });

        if (!isCanonicalBzzRequest(request)) {
          const cache = await caches.open(CACHE_NAME);
          await cache.put(request, response.clone());
        }

        resolve(response);
      } else {
        resolve(new Response("weeb-3 did not retrieve resource", { status: 404 }));
      }
    };

    client.postMessage(
      { type: "RETRIEVE_REQUEST", url: resource },
      [channel.port2]
    );
  });
};

async function postToLibRs(request, event) {
  debugLog("attempting upload (multipart/form-data)");

  const url = new URL(request.url);

  const encryption = request.headers.get("swarm-encrypt") === "true";
  const indexString = request.headers.get("swarm-index-document") || "";
  const addToFeed = request.headers.get("swarm-collection") === "true";
  const feedTopic = url.searchParams.get("feedTopic") || "";

  const formData = await request.formData();
  const file = formData.get("file");
  debugLog("Got file:", file?.name, file?.type, file?.size);

  if (!(file instanceof File)) {
    return new Response("No file in form data", { status: 400 });
  }

  const client = await requestClient(event);
  if (!client) {
    return new Response("No client available for upload", { status: 502 });
  }

  return new Promise((resolve) => {
    const channel = new MessageChannel();
    channel.port1.onmessage = (event) => {
      const { ok, reference } = event.data;
      if (ok) {
        resolve(new Response(JSON.stringify({ reference }), {
          status: 201,
          headers: { "Content-Type": "application/json" }
        }));
      } else {
        resolve(new Response("Upload failed", { status: 500 }));
      }
    };

    client.postMessage(
      {
        type: "UPLOAD_REQUEST",
        file,
        encryption,
        indexString,
        addToFeed,
        feedTopic
      },
      [channel.port2]
    );
  });
}
