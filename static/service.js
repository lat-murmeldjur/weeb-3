const putInCache = async (request, response) => {
  const cache = await caches.open('default0');
  var cacheput0 = await cache.put(request, response);
  console.log(cacheput0);
};

self.addEventListener('message', async function(event) {
  let asset0 = new Blob([event.data.data0], {type: event.data.mime0});

  const reqHeaders = new Headers();
  reqHeaders.set("Cache-Control", "public, max-age=60000000000000");
  const options = {
    headers: reqHeaders,
  };

  const request0 = new Request(event.data.path0, options);
  const response0 = new Response(asset0, { headers: { 'Content-Type': event.data.mime0, 'Content-Length': event.data.data0.length } });

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
  console.log("loading from cache failed");
  return fetch(request);
};

self.addEventListener("fetch", (event) => {
  event.respondWith(cacheFirst(event.request));
});