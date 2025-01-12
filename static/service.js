const putInCache = async (request, response) => {
  const cache = await caches.open('--');
  await cache.put(request, response);
};

self.addEventListener('message', async function(event) {
  let asset0 = new Blob([event.data.data0], {type: event.data.mime0});

  const reqHeaders = new Headers();
  reqHeaders.set("Cache-Control", "private");
  const options = {
    headers: reqHeaders,
  };

  const request0 = new Request(event.data.path0, options);
  const response0 = new Response(asset0, { headers: { contentType: event.data.mime0 } });

  await putInCache(request0, response0);
  console.log(event);
})

const cacheFirst = async (request) => {
  const responseFromCache = await caches.match(request);
  if (responseFromCache) {
    return responseFromCache;
  }
  return fetch(request);
};

self.addEventListener("fetch", (event) => {
  event.respondWith(cacheFirst(event.request));
});