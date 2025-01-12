const putInCache = async (request, response) => {
  const cache = await caches.open('--');
  await cache.put(request, response);
};

let data;
self.addEventListener('message', function(event) {
  console.log(event);
  data = event.data;
  console.log(data);
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