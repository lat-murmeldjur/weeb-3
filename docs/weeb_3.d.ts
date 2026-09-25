/* tslint:disable */
/* eslint-disable */

/** Bee-compatible erasure-coding level used for uploads. */
export type UploadRedundancyLevel = 0 | 1 | 2 | 3 | 4;
export type HlsStart = "beginning" | "live";



export class RequestArguments {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
    readonly method: string;
    readonly params: Array<any>;
}

export class Weeb3No103 {
    free(): void;
    [Symbol.dispose](): void;
    /**
     * The complete read, including startup, has a 30-second budget; expiry returns status="timeout".
     */
    acquireFeedBytes(owner: string, topic: string): Promise<object>;
    attachStream(media: HTMLMediaElement, owner: string, topic: string, start: HlsStart): Promise<void>;
    batchState(depth: number, validity_days: number): Promise<object>;
    buyBatch(depth: number, validity_days: number): Promise<object>;
    deployChequebook(): Promise<object>;
    depositChequebook(amount: string): Promise<object>;
    logs(): Promise<Array<any>>;
    networkState(): Promise<object>;
    constructor(shared_worker_url?: string | null);
    openSecureVault(): object;
    /**
     * Omit walletOwner to use the existing stored vault identity. An explicit EOA
     * address uses that wallet as the portable owner and prompts for each update.
     * Example: await node.postFeedBytes(topic, bytes, "text/plain", "captions", false, account).
     * Contract-wallet signatures and rejected/mismatched signatures fail; no key is derived or imported.
     */
    postFeedBytes(topic: string, bytes: Uint8Array, mime: string, filename: string, encryption: boolean, wallet_owner?: string | null): Promise<object>;
    postPushChunk(data: Uint8Array, soc: boolean, chunk_address: Uint8Array, stamp: Uint8Array): Promise<string>;
    postUploadBytes(bytes: Uint8Array, mime: string, filename: string, encryption: boolean, add_to_feed: boolean, feed_topic: string): Promise<object>;
    postUploadBytesWithRedundancy(bytes: Uint8Array, mime: string, filename: string, encryption: boolean, redundancy_level: UploadRedundancyLevel, add_to_feed: boolean, feed_topic: string): Promise<object>;
    progressSnapshot(seen_revision: number): Promise<object>;
    ready(min_connections: number, timeout_ms: number): Promise<boolean>;
    renderInterface(container: Element): object;
    resetStamp(): Promise<object>;
    retrieve(address: string): Promise<Array<any>>;
    retrieveBytes(address: string): Promise<Uint8Array>;
    retrieveChunk(address: string): Promise<Uint8Array>;
    start(options?: any | null): void;
    switchNetwork(mode: string): Promise<object>;
    upload(file: File, encryption: boolean, index_string: string, add_to_feed: boolean, feed_topic: string): Promise<object>;
    uploadWithRedundancy(file: File, encryption: boolean, redundancy_level: UploadRedundancyLevel, index_string: string, add_to_feed: boolean, feed_topic: string, wallet_owner?: string | null): Promise<object>;
}

export class Weeb3WorkerRuntime {
    free(): void;
    [Symbol.dispose](): void;
    handleMessage(message: any): Promise<object>;
    constructor();
    start(options: any): Promise<object>;
}

export function interweeb(_st: string): Promise<void>;

export type InitInput = RequestInfo | URL | Response | BufferSource | WebAssembly.Module;

export interface InitOutput {
    readonly memory: WebAssembly.Memory;
    readonly __wbg_weeb3no103_free: (a: number, b: number) => void;
    readonly __wbg_weeb3workerruntime_free: (a: number, b: number) => void;
    readonly interweeb: (a: number, b: number) => any;
    readonly weeb3no103_acquireFeedBytes: (a: number, b: number, c: number, d: number, e: number) => any;
    readonly weeb3no103_attachStream: (a: number, b: any, c: number, d: number, e: number, f: number, g: number, h: number) => any;
    readonly weeb3no103_batchState: (a: number, b: number, c: number) => any;
    readonly weeb3no103_buyBatch: (a: number, b: number, c: number) => any;
    readonly weeb3no103_deployChequebook: (a: number) => any;
    readonly weeb3no103_depositChequebook: (a: number, b: number, c: number) => any;
    readonly weeb3no103_logs: (a: number) => any;
    readonly weeb3no103_networkState: (a: number) => any;
    readonly weeb3no103_new: (a: number, b: number) => number;
    readonly weeb3no103_openSecureVault: (a: number) => any;
    readonly weeb3no103_postFeedBytes: (a: number, b: number, c: number, d: any, e: number, f: number, g: number, h: number, i: number, j: number, k: number) => any;
    readonly weeb3no103_postPushChunk: (a: number, b: any, c: number, d: any, e: any) => any;
    readonly weeb3no103_postUploadBytes: (a: number, b: any, c: number, d: number, e: number, f: number, g: number, h: number, i: number, j: number) => any;
    readonly weeb3no103_postUploadBytesWithRedundancy: (a: number, b: any, c: number, d: number, e: number, f: number, g: number, h: number, i: number, j: number, k: number) => any;
    readonly weeb3no103_progressSnapshot: (a: number, b: number) => any;
    readonly weeb3no103_ready: (a: number, b: number, c: number) => any;
    readonly weeb3no103_renderInterface: (a: number, b: any) => any;
    readonly weeb3no103_resetStamp: (a: number) => any;
    readonly weeb3no103_retrieve: (a: number, b: number, c: number) => any;
    readonly weeb3no103_retrieveBytes: (a: number, b: number, c: number) => any;
    readonly weeb3no103_retrieveChunk: (a: number, b: number, c: number) => any;
    readonly weeb3no103_start: (a: number, b: number) => void;
    readonly weeb3no103_switchNetwork: (a: number, b: number, c: number) => any;
    readonly weeb3no103_upload: (a: number, b: any, c: number, d: number, e: number, f: number, g: number, h: number) => any;
    readonly weeb3no103_uploadWithRedundancy: (a: number, b: any, c: number, d: number, e: number, f: number, g: number, h: number, i: number, j: number, k: number) => any;
    readonly weeb3workerruntime_handleMessage: (a: number, b: any) => any;
    readonly weeb3workerruntime_new: () => number;
    readonly weeb3workerruntime_start: (a: number, b: any) => any;
    readonly __wbg_requestarguments_free: (a: number, b: number) => void;
    readonly requestarguments_method: (a: number) => [number, number];
    readonly requestarguments_params: (a: number) => any;
    readonly wasm_bindgen_c5c348924b01089b___closure__destroy___dyn_core_ed718c3d60ebd546___ops__function__FnMut_____Output_______: (a: number, b: number) => void;
    readonly wasm_bindgen_c5c348924b01089b___closure__destroy___dyn_core_ed718c3d60ebd546___ops__function__Fn__wasm_bindgen_c5c348924b01089b___JsValue__wasm_bindgen_c5c348924b01089b___JsValue___Output_______: (a: number, b: number) => void;
    readonly wasm_bindgen_c5c348924b01089b___closure__destroy___dyn_core_ed718c3d60ebd546___ops__function__FnMut__wasm_bindgen_c5c348924b01089b___JsValue____Output_______: (a: number, b: number) => void;
    readonly wasm_bindgen_c5c348924b01089b___convert__closures_____invoke___wasm_bindgen_c5c348924b01089b___JsValue__alloc_508e33bd5656020b___string__String__core_ed718c3d60ebd546___result__Result_____wasm_bindgen_c5c348924b01089b___JsValue__: (a: number, b: number, c: any, d: number, e: number) => [number, number];
    readonly wasm_bindgen_c5c348924b01089b___convert__closures_____invoke___wasm_bindgen_c5c348924b01089b___JsValue__wasm_bindgen_c5c348924b01089b___JsValue_____: (a: number, b: number, c: any, d: any) => void;
    readonly wasm_bindgen_c5c348924b01089b___convert__closures_____invoke___web_sys_cfecf4937efbdc06___features__gen_IdbVersionChangeEvent__IdbVersionChangeEvent__core_ed718c3d60ebd546___result__Result_____wasm_bindgen_c5c348924b01089b___JsValue__: (a: number, b: number, c: any) => [number, number];
    readonly wasm_bindgen_c5c348924b01089b___convert__closures_____invoke___wasm_bindgen_c5c348924b01089b___JsValue__wasm_bindgen_c5c348924b01089b___JsValue______1_: (a: number, b: number, c: any, d: any) => void;
    readonly wasm_bindgen_c5c348924b01089b___convert__closures_____invoke___wasm_bindgen_c5c348924b01089b___JsValue_____: (a: number, b: number, c: any) => void;
    readonly wasm_bindgen_c5c348924b01089b___convert__closures_____invoke______: (a: number, b: number) => void;
    readonly __wbindgen_malloc: (a: number, b: number) => number;
    readonly __wbindgen_realloc: (a: number, b: number, c: number, d: number) => number;
    readonly __wbindgen_exn_store: (a: number) => void;
    readonly __externref_table_alloc: () => number;
    readonly __wbindgen_externrefs: WebAssembly.Table;
    readonly __wbindgen_free: (a: number, b: number, c: number) => void;
    readonly __externref_table_dealloc: (a: number) => void;
    readonly __wbindgen_start: () => void;
}

export type SyncInitInput = BufferSource | WebAssembly.Module;

/**
 * Instantiates the given `module`, which can either be bytes or
 * a precompiled `WebAssembly.Module`.
 *
 * @param {{ module: SyncInitInput }} module - Passing `SyncInitInput` directly is deprecated.
 *
 * @returns {InitOutput}
 */
export function initSync(module: { module: SyncInitInput } | SyncInitInput): InitOutput;

/**
 * If `module_or_path` is {RequestInfo} or {URL}, makes a request and
 * for everything else, calls `WebAssembly.instantiate` directly.
 *
 * @param {{ module_or_path: InitInput | Promise<InitInput> }} module_or_path - Passing `InitInput` directly is deprecated.
 *
 * @returns {Promise<InitOutput>}
 */
export default function __wbg_init (module_or_path?: { module_or_path: InitInput | Promise<InitInput> } | InitInput | Promise<InitInput>): Promise<InitOutput>;
