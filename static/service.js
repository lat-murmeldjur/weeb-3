const CACHE_NAME = "default0";
const BZZ_MARKER = "/weeb-3/bzz/";
const MIB_BYTES = 1024 * 1024;
const STREAM_STORAGE_WINDOW_BYTES = MIB_BYTES / 2;
const STREAM_RESPONSE_BUFFER_BYTES = 8 * MIB_BYTES;
const STREAM_ACTIVE_RESPONSE_BUFFER_BYTES = 2 * MIB_BYTES;
const RANGE_RETRY_COUNT = 2;
const RANGE_RETRY_DELAY_MS = 500;
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
const RANGE_CACHE_MAX_ENTRIES = 384;
const metadataCache = new Map();
const rangeCache = new Map();
const mediaStreamStates = new Map();

const putInCache = async (request, response) => {
  const cache = await caches.open(CACHE_NAME);
  await cache.put(request, response);
  console.log("Cached:", request.url);
};

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

  console.log("Message event processed:", event);
});

const cacheFirst = async (request) => {
  const cache = await caches.open(CACHE_NAME);
  console.log("req0: ", request);

  const responseFromCache = await cache.match(request);
  console.log("respc: ", responseFromCache);
  if (responseFromCache) {
    return responseFromCache;
  }

  try {
    const actualResource = await fetch(request);
    if (actualResource.ok) {
      return actualResource;
    }
  } catch (e) {
  }

  const fetched = await fetch(request);
  cache.put(request, fetched.clone());
  return fetched;
};

self.addEventListener("install", (event) => {
  console.log("install");
  event.waitUntil(
    (async () => {
      const cache = await caches.open(CACHE_NAME);
      await cache.addAll(["/weeb-3/index.html"]);
      await self.skipWaiting();
    })()
  );
});

self.addEventListener("activate", event => {
  console.log("service activated, claim client");
  event.waitUntil(self.clients.claim());
});

self.addEventListener("fetch", (event) => {
  console.log("fetch mode:", event.request.mode, "url:", event.request.url);

  const req = event.request;
  const url = new URL(req.url);

  console.log("SW FETCH:", req.method, url.pathname, "scope:", self.registration.scope);

  if (req.method === "POST" && url.pathname.endsWith("/weeb-3/bzz")) {
    console.log("Intercepting POST upload:", url.toString());
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
    req.mode !== "navigate" &&
    (req.method === "GET" || req.method === "HEAD")
  ) {
    event.respondWith(handleCanonicalBzz(req, event, bzzResource));
    return;
  }

  event.respondWith((async () => {
    if (req.mode === "navigate") {
      console.log("navigate attempt for", req.url);
      return cacheFirst(req);
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

    console.log("acquire attempt for", req.url);
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

  try {
    return decodeURIComponent(resource);
  } catch (_) {
    return resource;
  }
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

function messageClient(client, message) {
  return new Promise((resolve) => {
    const channel = new MessageChannel();
    const timer = setTimeout(() => {
      resolve({ ok: false, error: "Timed out waiting for weeb-3" });
    }, 120000);

    channel.port1.onmessage = (event) => {
      clearTimeout(timer);
      resolve(event.data || { ok: false });
    };

    client.postMessage(message, [channel.port2]);
  });
}

async function resolveBzz(resource, client) {
  const cached = metadataCache.get(resource);
  if (cached) {
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
  return metadata;
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

  return messageClient(client, message);
}

function metadataIdentity(resource, metadata) {
  return metadata.etag || metadata.data_reference || resource;
}

function mediaStateKey(resource, metadata) {
  return `${metadataIdentity(resource, metadata)}|${resource}`;
}

function rangeCacheKey(resource, start, end, metadata, generation) {
  return `${mediaStateKey(resource, metadata)}|g${generation || 0}|${start}-${end}`;
}

function deleteCachedRange(resource, start, end, metadata, generation) {
  rangeCache.delete(rangeCacheKey(resource, start, end, metadata, generation));
}

function trimRangeCache() {
  while (rangeCache.size > RANGE_CACHE_MAX_ENTRIES) {
    const oldestKey = rangeCache.keys().next().value;
    rangeCache.delete(oldestKey);
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
      lastRequestStart: 0,
      lastRangeWasSeek: false,
      lastRangeWasStartup: false,
      prefetchRunning: false,
      prefetchGeneration: 0,
      lastTouch: Date.now()
    };
    mediaStreamStates.set(key, state);
  }
  return state;
}

function rangeEntryMatchesResource(entry, resource, metadata) {
  return entry.resource === resource && entry.identity === metadataIdentity(resource, metadata);
}

function discardMediaRangesOutside(resource, metadata, generation, keepStart, keepEnd) {
  let discarded = 0;
  for (const [key, entry] of rangeCache) {
    if (!rangeEntryMatchesResource(entry, resource, metadata)) {
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
    console.log(
      "bzz media canceled stale ranges",
      discarded,
      `keep=${keepStart}-${keepEnd}`,
      `generation=${generation}`
    );
  }
}

function beginMediaRange(resource, start, metadata) {
  const state = getMediaStreamState(resource, metadata);
  const previousAnchor = state.anchorStart;
  const previousHighWater = state.highWaterEnd;
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

  if (isSeek) {
    state.generation += 1;
    state.anchorStart = start;
    state.highWaterEnd = start - 1;
    discardMediaRangesOutside(
      resource,
      metadata,
      state.generation,
      Math.max(0, start - STREAM_RESPONSE_BUFFER_BYTES),
      Math.min(metadata.size - 1, start + STREAM_SEEK_KEEP_AHEAD_BYTES - 1)
    );
    console.log(
      "bzz media seek reset",
      `${start}/${metadata.size}`,
      `generation=${state.generation}`
    );
  } else if (isStartup) {
    state.anchorStart = start;
  }

  state.lastRequestStart = start;
  state.lastRangeWasSeek = isSeek;
  state.lastRangeWasStartup = isStartup;
  state.lastTouch = Date.now();
  return state;
}

function cachedRangePromise(resource, start, end, client, metadata, generation = 0) {
  const key = rangeCacheKey(resource, start, end, metadata, generation);
  let entry = rangeCache.get(key);
  if (entry) {
    return entry.promise;
  }

  const promise = retrieveRange(resource, start, end, client, metadata, generation).then(
    (rangeResponse) => {
      if (!rangeResponse.ok) {
        rangeCache.delete(key);
      }
      return rangeResponse;
    }
  );

  entry = {
    promise,
    resource,
    identity: metadataIdentity(resource, metadata),
    start,
    end,
    generation
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
      return { ok: false, error: rangeResponse.error || "weeb-3 did not retrieve range" };
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

    console.log(
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
    state.highWaterEnd = Math.max(state.highWaterEnd, responseRange.end);
  }

  return new Promise((resolve) => {
    let active = 0;
    let logged = false;

    const isCurrentGeneration = () => !state || state.generation === generation;

    const finishIfDone = () => {
      if (active === 0 && (!isCurrentGeneration() || position > targetEnd)) {
        if (scheduled > 0) {
          console.log(
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
        console.log(
          `bzz media ${label} stopped stale`,
          `${responseRange.end + 1}-${targetEnd}/${metadata.size}`,
          `generation=${generation}`,
          `current=${state ? state.generation : generation}`
        );
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
          state.highWaterEnd = Math.max(state.highWaterEnd, window.end);
        }
        position = window.end + 1;
        scheduled += 1;
        active += 1;

        if (!logged) {
          logged = true;
          console.log(
            `bzz media ${label}`,
            `${responseRange.end + 1}-${targetEnd}/${metadata.size}`,
            `max_active=${maxActive}`,
            `generation=${generation}`
          );
        }

        Promise.resolve(window.promise)
          .catch(() => undefined)
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
  if (state) {
    if (state.prefetchRunning && state.prefetchGeneration === generation) {
      console.log(
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
      if (currentEnd >= requestedEnd || currentEnd >= metadata.size - 1) {
        return;
      }

      const targetEnd = Math.min(currentEnd + stageBytes, requestedEnd, metadata.size - 1);
      await prefetchMediaWindows(resource, responseRange, requestedEnd, client, metadata, state, {
        targetEnd,
        maxActive: STREAM_PREFETCH_MAX_WINDOWS,
        label: `prefetch stage ${i + 1} ${Math.round(stageBytes / MIB_BYTES)}MiB`
      });
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
      const rangeResponse = await readCachedRange(
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
    console.log(
      "bzz media range",
      request.headers.get("Range"),
      "=>",
      `${responseRange.start}-${responseRange.end}/${metadata.size}`
    );
    for (const responseStorage of responseStorageWindows) {
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
      return new Response(rangeResponse.error || "weeb-3 did not retrieve range", { status: 503 });
    }

    const body = rangeResponse.body;
    if (rangeResponse.short) {
      return new Response("weeb-3 returned a short range", { status: 502 });
    }

    if (streamable) {
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
    console.log(
      "bzz media startup range",
      `${startupRange.start}-${startupRange.end}/${metadata.size}`
    );
    for (const startupStorage of startupStorageWindows) {
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
      return new Response(rangeResponse.error || "weeb-3 did not retrieve startup range", {
        status: 503
      });
    }

    const body = rangeResponse.body;
    if (rangeResponse.short) {
      return new Response("weeb-3 returned a short startup range", { status: 502 });
    }

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
  console.log("all clients", allClients.map(c => c.id));
  console.log("actual client", client ? client.id : "none");

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
      console.log("Message from interface:", {
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

        const cache = await caches.open(CACHE_NAME);
        await cache.put(request, response.clone());

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
  console.log("attempting upload (multipart/form-data)");

  const url = new URL(request.url);

  const encryption = request.headers.get("swarm-encrypt") === "true";
  const indexString = request.headers.get("swarm-index-document") || "";
  const addToFeed = request.headers.get("swarm-collection") === "true";
  const feedTopic = url.searchParams.get("feedTopic") || "";

  const formData = await request.formData();
  const file = formData.get("file");
  console.log("Got file:", file?.name, file?.type, file?.size);

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
