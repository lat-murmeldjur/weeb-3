const putInCache = async (request, response) => {
  const cache = await caches.open('--');
  await cache.put(request, response);
};

self.addEventListener("fetch", (event) => {
  event.respondWith(caches.match(event.request));
});
