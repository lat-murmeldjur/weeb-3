const putInCache = async (request, response) => {
  const cache = await caches.open('default0');
  var cacheput0 = await cache.put(request, response);
  console.log(cacheput0);
};

self.addEventListener('message', async function(event) {
  let asset0 = new Blob([event.data.data0[0]], {type: event.data.mime0});

  const reqHeaders = new Headers();
  reqHeaders.set("Cache-Control", "public, max-age=60000000000000");
  const options = {
    headers: reqHeaders,
  };

  const request0 = new Request(event.data.path0, options);
  const response0 = new Response(asset0, { headers: { 'Content-Type': event.data.mime0, 'Content-Length': event.data.data0[0].length } });

  await putInCache(request0, response0);
  console.log(event);
})

const cacheFirst = async (request) => {
  const cache = await caches.open('default0');
  console.log("req0: ", request);

  const responseFromCache = await cache.match(request);
  console.log("respc: ", responseFromCache);
  if (responseFromCache) {
    return responseFromCache;
  } 

  try {
    const actual_resource = await fetch(request);
    if (actual_resource.ok) {
      return actual_resource;
    }
  } catch (e) {
  }
  
  let cachedIndex = await cache.match('/weeb-3/index.html');
  if (cachedIndex) {
    return cachedIndex;
  }
  
  let fetched = await fetch('/weeb-3/index.html');
  cache.put('/weeb-3/index.html', fetched.clone());
  return fetched;
};

self.addEventListener('install', (event) => {
  console.log("install");
  event.waitUntil(
    (async () => {
      const cache = await caches.open('default0');
      await cache.addAll(['/weeb-3/index.html']);
      self.skipWaiting(); 
    })()
  );
});

self.addEventListener("activate", event => {
  console.log("serice activated, claim client");
  event.waitUntil(self.clients.claim());
});

// self.addEventListener("fetch", (event) => {
//   event.respondWith(cacheFirst(event.request));
// });

// Try experimental endpoint

self.addEventListener("fetch", (event) => {
  console.log("fetch mode:", event.request.mode, "url:", event.request.url);

  const req = event.request;

  event.respondWith((async () => {
    if (req.mode === "navigate") {
      console.log("navigate attempt for", req.url);
      return cacheFirst(req);
    }

    // try cache first anyway
    const cache = await caches.open('default0');
    const responseFromCache = await cache.match(req);
    if (responseFromCache) {
      return responseFromCache;
    }

    try {
      const actual_resource = await fetch(req);
  
      if (actual_resource.ok) {
        return actual_resource;
      }
    } catch (e) {
    }

    // if no cache hit find tab where fetch originated from
    let client = null;
    if (event.clientId) {
      client = await self.clients.get(event.clientId);
    }

    console.log("acquire attempt for", req.url);
    return await fetchFromLibRs(req, client);
    
  })());
});

const fetchFromLibRs = async (request, client) => {
  const allClients = await self.clients.matchAll({ includeUncontrolled: true, type: "window" });
  console.log("all clients", allClients.map(c => c.id));
  console.log("actual client", client ? client.id : "none");

  if (!client) {
    return new Response("Request ghosted by client", { status: 502 });
  }

  const url = new URL(request.url);
  let resource = url.pathname;
  const marker = "/weeb-3/bzz/";
  const idx = resource.indexOf(marker);
  if (idx >= 0) {
    resource = resource.substring(idx + marker.length);
  }

  return new Promise((resolve) => {
    const channel = new MessageChannel();
    channel.port1.onmessage = async (event) => {
      const { ok, body, mime, path } = event.data;
      console.log("Message from interface:", {
        ok,
        bodyType: body ? body.constructor.name : body, // show if it's Uint8Array, undefined, etc.
        bodyLen: body && body.length ? body.length : 0,
        mime,
        path
      });
      if (ok && body) {
        const response = new Response(new Blob([body], { type: mime }), {
          headers: { "Content-Type": mime }
        });

        const cache = await caches.open("default0");
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






