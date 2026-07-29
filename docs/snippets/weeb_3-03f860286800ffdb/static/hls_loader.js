// Dynamic `import()` has no CSP-safe Web API callable directly from Wasm.
// Keep this irreducible bridge tiny; player policy and lifecycle live in Rust.
export async function loadHls() {
  const module = await import("hls.js");
  return module.default ?? module.Hls;
}
