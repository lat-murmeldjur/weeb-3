export async function loadHls() {
  const module = await import("hls.js");
  return module.default ?? module.Hls;
}
