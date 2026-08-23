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
    acquireFeedBytes(owner: string, topic: string): Promise<object>;
    attachStream(media: HTMLMediaElement, owner: string, topic: string, start: HlsStart): Promise<void>;
    batchState(depth: number, validity_days: number): Promise<object>;
    buyBatch(depth: number, validity_days: number): Promise<object>;
    deployChequebook(): Promise<object>;
    depositChequebook(amount: string): Promise<object>;
    logs(): Promise<Array<any>>;
    networkState(): Promise<object>;
    constructor();
    openSecureVault(): object;
    postFeedBytes(topic: string, bytes: Uint8Array, mime: string, filename: string, encryption: boolean): Promise<object>;
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
    uploadWithRedundancy(file: File, encryption: boolean, redundancy_level: UploadRedundancyLevel, index_string: string, add_to_feed: boolean, feed_topic: string): Promise<object>;
}

export function init_wasm_runtime(): void;

export function interweeb(_st: string): Promise<void>;

export type InitInput = RequestInfo | URL | Response | BufferSource | WebAssembly.Module;

export interface InitOutput {
    readonly memory: WebAssembly.Memory;
    readonly __wbg_weeb3no103_free: (a: number, b: number) => void;
    readonly init_wasm_runtime: () => void;
    readonly interweeb: (a: number, b: number) => number;
    readonly weeb3no103_acquireFeedBytes: (a: number, b: number, c: number, d: number, e: number) => number;
    readonly weeb3no103_attachStream: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number) => number;
    readonly weeb3no103_batchState: (a: number, b: number, c: number) => number;
    readonly weeb3no103_buyBatch: (a: number, b: number, c: number) => number;
    readonly weeb3no103_deployChequebook: (a: number) => number;
    readonly weeb3no103_depositChequebook: (a: number, b: number, c: number) => number;
    readonly weeb3no103_logs: (a: number) => number;
    readonly weeb3no103_networkState: (a: number) => number;
    readonly weeb3no103_new: () => number;
    readonly weeb3no103_openSecureVault: (a: number) => number;
    readonly weeb3no103_postFeedBytes: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number, i: number, j: number) => number;
    readonly weeb3no103_postPushChunk: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number) => number;
    readonly weeb3no103_postUploadBytes: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number, i: number, j: number, k: number) => number;
    readonly weeb3no103_postUploadBytesWithRedundancy: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number, i: number, j: number, k: number, l: number) => number;
    readonly weeb3no103_progressSnapshot: (a: number, b: number) => number;
    readonly weeb3no103_ready: (a: number, b: number, c: number) => number;
    readonly weeb3no103_renderInterface: (a: number, b: number) => number;
    readonly weeb3no103_resetStamp: (a: number) => number;
    readonly weeb3no103_retrieve: (a: number, b: number, c: number) => number;
    readonly weeb3no103_retrieveBytes: (a: number, b: number, c: number) => number;
    readonly weeb3no103_retrieveChunk: (a: number, b: number, c: number) => number;
    readonly weeb3no103_start: (a: number, b: number) => void;
    readonly weeb3no103_switchNetwork: (a: number, b: number, c: number) => number;
    readonly weeb3no103_upload: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number) => number;
    readonly weeb3no103_uploadWithRedundancy: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number, i: number) => number;
    readonly __wbg_requestarguments_free: (a: number, b: number) => void;
    readonly requestarguments_method: (a: number, b: number) => void;
    readonly requestarguments_params: (a: number) => number;
    readonly __wasm_bindgen_func_elem_529: (a: number, b: number) => void;
    readonly __wasm_bindgen_func_elem_1395: (a: number, b: number, c: number, d: number) => void;
    readonly __wasm_bindgen_func_elem_1397: (a: number, b: number, c: number, d: number) => void;
    readonly __wasm_bindgen_func_elem_3407: (a: number, b: number, c: number) => void;
    readonly __wasm_bindgen_func_elem_3333: (a: number, b: number) => void;
    readonly __wbindgen_export: (a: number, b: number) => number;
    readonly __wbindgen_export2: (a: number, b: number, c: number, d: number) => number;
    readonly __wbindgen_export3: (a: number) => void;
    readonly __wbindgen_export4: (a: number, b: number, c: number) => void;
    readonly __wbindgen_add_to_stack_pointer: (a: number) => number;
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
