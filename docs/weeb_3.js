import { get_provider_js } from './snippets/web3-0742d85b024bb6f5/inline0.js';

let wasm;

let cachedUint8ArrayMemory0 = null;

function getUint8ArrayMemory0() {
    if (cachedUint8ArrayMemory0 === null || cachedUint8ArrayMemory0.byteLength === 0) {
        cachedUint8ArrayMemory0 = new Uint8Array(wasm.memory.buffer);
    }
    return cachedUint8ArrayMemory0;
}

let cachedTextDecoder = new TextDecoder('utf-8', { ignoreBOM: true, fatal: true });

cachedTextDecoder.decode();

const MAX_SAFARI_DECODE_BYTES = 2146435072;
let numBytesDecoded = 0;
function decodeText(ptr, len) {
    numBytesDecoded += len;
    if (numBytesDecoded >= MAX_SAFARI_DECODE_BYTES) {
        cachedTextDecoder = new TextDecoder('utf-8', { ignoreBOM: true, fatal: true });
        cachedTextDecoder.decode();
        numBytesDecoded = len;
    }
    return cachedTextDecoder.decode(getUint8ArrayMemory0().subarray(ptr, ptr + len));
}

function getStringFromWasm0(ptr, len) {
    ptr = ptr >>> 0;
    return decodeText(ptr, len);
}

let WASM_VECTOR_LEN = 0;

const cachedTextEncoder = new TextEncoder();

if (!('encodeInto' in cachedTextEncoder)) {
    cachedTextEncoder.encodeInto = function (arg, view) {
        const buf = cachedTextEncoder.encode(arg);
        view.set(buf);
        return {
            read: arg.length,
            written: buf.length
        };
    }
}

function passStringToWasm0(arg, malloc, realloc) {

    if (realloc === undefined) {
        const buf = cachedTextEncoder.encode(arg);
        const ptr = malloc(buf.length, 1) >>> 0;
        getUint8ArrayMemory0().subarray(ptr, ptr + buf.length).set(buf);
        WASM_VECTOR_LEN = buf.length;
        return ptr;
    }

    let len = arg.length;
    let ptr = malloc(len, 1) >>> 0;

    const mem = getUint8ArrayMemory0();

    let offset = 0;

    for (; offset < len; offset++) {
        const code = arg.charCodeAt(offset);
        if (code > 0x7F) break;
        mem[ptr + offset] = code;
    }

    if (offset !== len) {
        if (offset !== 0) {
            arg = arg.slice(offset);
        }
        ptr = realloc(ptr, len, len = offset + arg.length * 3, 1) >>> 0;
        const view = getUint8ArrayMemory0().subarray(ptr + offset, ptr + len);
        const ret = cachedTextEncoder.encodeInto(arg, view);

        offset += ret.written;
        ptr = realloc(ptr, len, offset, 1) >>> 0;
    }

    WASM_VECTOR_LEN = offset;
    return ptr;
}

let cachedDataViewMemory0 = null;

function getDataViewMemory0() {
    if (cachedDataViewMemory0 === null || cachedDataViewMemory0.buffer.detached === true || (cachedDataViewMemory0.buffer.detached === undefined && cachedDataViewMemory0.buffer !== wasm.memory.buffer)) {
        cachedDataViewMemory0 = new DataView(wasm.memory.buffer);
    }
    return cachedDataViewMemory0;
}

function isLikeNone(x) {
    return x === undefined || x === null;
}

function debugString(val) {
    // primitive types
    const type = typeof val;
    if (type == 'number' || type == 'boolean' || val == null) {
        return  `${val}`;
    }
    if (type == 'string') {
        return `"${val}"`;
    }
    if (type == 'symbol') {
        const description = val.description;
        if (description == null) {
            return 'Symbol';
        } else {
            return `Symbol(${description})`;
        }
    }
    if (type == 'function') {
        const name = val.name;
        if (typeof name == 'string' && name.length > 0) {
            return `Function(${name})`;
        } else {
            return 'Function';
        }
    }
    // objects
    if (Array.isArray(val)) {
        const length = val.length;
        let debug = '[';
        if (length > 0) {
            debug += debugString(val[0]);
        }
        for(let i = 1; i < length; i++) {
            debug += ', ' + debugString(val[i]);
        }
        debug += ']';
        return debug;
    }
    // Test for built-in
    const builtInMatches = /\[object ([^\]]+)\]/.exec(toString.call(val));
    let className;
    if (builtInMatches && builtInMatches.length > 1) {
        className = builtInMatches[1];
    } else {
        // Failed to match the standard '[object ClassName]'
        return toString.call(val);
    }
    if (className == 'Object') {
        // we're a user defined class or Object
        // JSON.stringify avoids problems with cycles, and is generally much
        // easier than looping through ownProperties of `val`.
        try {
            return 'Object(' + JSON.stringify(val) + ')';
        } catch (_) {
            return 'Object';
        }
    }
    // errors
    if (val instanceof Error) {
        return `${val.name}: ${val.message}\n${val.stack}`;
    }
    // TODO we could test for more things here, like `Set`s and `Map`s.
    return className;
}

function addToExternrefTable0(obj) {
    const idx = wasm.__externref_table_alloc();
    wasm.__wbindgen_externrefs.set(idx, obj);
    return idx;
}

function handleError(f, args) {
    try {
        return f.apply(this, args);
    } catch (e) {
        const idx = addToExternrefTable0(e);
        wasm.__wbindgen_exn_store(idx);
    }
}

function getArrayU8FromWasm0(ptr, len) {
    ptr = ptr >>> 0;
    return getUint8ArrayMemory0().subarray(ptr / 1, ptr / 1 + len);
}

const CLOSURE_DTORS = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(state => state.dtor(state.a, state.b));

function makeMutClosure(arg0, arg1, dtor, f) {
    const state = { a: arg0, b: arg1, cnt: 1, dtor };
    const real = (...args) => {

        // First up with a closure we increment the internal reference
        // count. This ensures that the Rust closure environment won't
        // be deallocated while we're invoking it.
        state.cnt++;
        const a = state.a;
        state.a = 0;
        try {
            return f(a, state.b, ...args);
        } finally {
            state.a = a;
            real._wbg_cb_unref();
        }
    };
    real._wbg_cb_unref = () => {
        if (--state.cnt === 0) {
            state.dtor(state.a, state.b);
            state.a = 0;
            CLOSURE_DTORS.unregister(state);
        }
    };
    CLOSURE_DTORS.register(real, state, state);
    return real;
}

function getArrayJsValueFromWasm0(ptr, len) {
    ptr = ptr >>> 0;
    const mem = getDataViewMemory0();
    const result = [];
    for (let i = ptr; i < ptr + 4 * len; i += 4) {
        result.push(wasm.__wbindgen_externrefs.get(mem.getUint32(i, true)));
    }
    wasm.__externref_drop_slice(ptr, len);
    return result;
}
/**
 * @param {string} _st
 * @returns {Promise<void>}
 */
export function interweeb(_st) {
    const ptr0 = passStringToWasm0(_st, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len0 = WASM_VECTOR_LEN;
    const ret = wasm.interweeb(ptr0, len0);
    return ret;
}

function takeFromExternrefTable0(idx) {
    const value = wasm.__wbindgen_externrefs.get(idx);
    wasm.__externref_table_dealloc(idx);
    return value;
}
function wasm_bindgen__convert__closures_____invoke__h43c36ca2f90d11b7(arg0, arg1, arg2) {
    const ret = wasm.wasm_bindgen__convert__closures_____invoke__h43c36ca2f90d11b7(arg0, arg1, arg2);
    if (ret[1]) {
        throw takeFromExternrefTable0(ret[0]);
    }
}

function wasm_bindgen__convert__closures_____invoke__h02a1a5a325cc1eca(arg0, arg1, arg2) {
    wasm.wasm_bindgen__convert__closures_____invoke__h02a1a5a325cc1eca(arg0, arg1, arg2);
}

function wasm_bindgen__convert__closures_____invoke__he1f0b0ce9a4c8d18(arg0, arg1) {
    wasm.wasm_bindgen__convert__closures_____invoke__he1f0b0ce9a4c8d18(arg0, arg1);
}

function wasm_bindgen__convert__closures_____invoke__h87e2b5b49ec422e1(arg0, arg1) {
    wasm.wasm_bindgen__convert__closures_____invoke__h87e2b5b49ec422e1(arg0, arg1);
}

function wasm_bindgen__convert__closures_____invoke__h1b9dadbe13f4df00(arg0, arg1, arg2) {
    wasm.wasm_bindgen__convert__closures_____invoke__h1b9dadbe13f4df00(arg0, arg1, arg2);
}

function wasm_bindgen__convert__closures_____invoke__h9c0ae922cc046d1d(arg0, arg1, arg2) {
    wasm.wasm_bindgen__convert__closures_____invoke__h9c0ae922cc046d1d(arg0, arg1, arg2);
}

function wasm_bindgen__convert__closures_____invoke__hb4b123985bbb9e04(arg0, arg1) {
    wasm.wasm_bindgen__convert__closures_____invoke__hb4b123985bbb9e04(arg0, arg1);
}

function wasm_bindgen__convert__closures_____invoke__h2536b6f3fd53c579(arg0, arg1, arg2, arg3) {
    wasm.wasm_bindgen__convert__closures_____invoke__h2536b6f3fd53c579(arg0, arg1, arg2, arg3);
}

const __wbindgen_enum_IdbRequestReadyState = ["pending", "done"];

const __wbindgen_enum_IdbTransactionMode = ["readonly", "readwrite", "versionchange", "readwriteflush", "cleanup"];

const __wbindgen_enum_RequestCredentials = ["omit", "same-origin", "include"];

const __wbindgen_enum_RequestMode = ["same-origin", "no-cors", "cors", "navigate"];

const __wbindgen_enum_RtcDataChannelState = ["connecting", "open", "closing", "closed"];

const __wbindgen_enum_RtcDataChannelType = ["arraybuffer", "blob"];

const __wbindgen_enum_RtcSdpType = ["offer", "pranswer", "answer", "rollback"];

const RequestArgumentsFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_requestarguments_free(ptr >>> 0, 1));

export class RequestArguments {

    static __wrap(ptr) {
        ptr = ptr >>> 0;
        const obj = Object.create(RequestArguments.prototype);
        obj.__wbg_ptr = ptr;
        RequestArgumentsFinalization.register(obj, obj.__wbg_ptr, obj);
        return obj;
    }

    __destroy_into_raw() {
        const ptr = this.__wbg_ptr;
        this.__wbg_ptr = 0;
        RequestArgumentsFinalization.unregister(this);
        return ptr;
    }

    free() {
        const ptr = this.__destroy_into_raw();
        wasm.__wbg_requestarguments_free(ptr, 0);
    }
    /**
     * @returns {string}
     */
    get method() {
        let deferred1_0;
        let deferred1_1;
        try {
            const ret = wasm.requestarguments_method(this.__wbg_ptr);
            deferred1_0 = ret[0];
            deferred1_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
        }
    }
    /**
     * @returns {Array<any>}
     */
    get params() {
        const ret = wasm.requestarguments_params(this.__wbg_ptr);
        return ret;
    }
}
if (Symbol.dispose) RequestArguments.prototype[Symbol.dispose] = RequestArguments.prototype.free;

const SekireiFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_sekirei_free(ptr >>> 0, 1));

export class Sekirei {

    static __wrap(ptr) {
        ptr = ptr >>> 0;
        const obj = Object.create(Sekirei.prototype);
        obj.__wbg_ptr = ptr;
        SekireiFinalization.register(obj, obj.__wbg_ptr, obj);
        return obj;
    }

    __destroy_into_raw() {
        const ptr = this.__wbg_ptr;
        this.__wbg_ptr = 0;
        SekireiFinalization.unregister(this);
        return ptr;
    }

    free() {
        const ptr = this.__destroy_into_raw();
        wasm.__wbg_sekirei_free(ptr, 0);
    }
    /**
     * @param {File} file
     * @param {boolean} encryption
     * @param {string} index_string
     * @param {boolean} add_to_feed
     * @param {string} feed_topic
     * @returns {Promise<Uint8Array>}
     */
    post_upload(file, encryption, index_string, add_to_feed, feed_topic) {
        const ptr0 = passStringToWasm0(index_string, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passStringToWasm0(feed_topic, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len1 = WASM_VECTOR_LEN;
        const ret = wasm.sekirei_post_upload(this.__wbg_ptr, file, encryption, ptr0, len0, add_to_feed, ptr1, len1);
        return ret;
    }
    /**
     * @returns {Promise<Uint8Array>}
     */
    reset_stamp() {
        const ret = wasm.sekirei_reset_stamp(this.__wbg_ptr);
        return ret;
    }
    /**
     * @param {string} log0
     */
    interface_log(log0) {
        const ptr0 = passStringToWasm0(log0, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        wasm.sekirei_interface_log(this.__wbg_ptr, ptr0, len0);
    }
    /**
     * @returns {Promise<bigint>}
     */
    get_connections() {
        const ret = wasm.sekirei_get_connections(this.__wbg_ptr);
        return ret;
    }
    /**
     * @returns {Promise<string[]>}
     */
    get_current_logs() {
        const ret = wasm.sekirei_get_current_logs(this.__wbg_ptr);
        return ret;
    }
    /**
     * @param {string} address
     * @param {string} _id
     * @returns {Promise<Uint8Array>}
     */
    change_bootnode_address(address, _id) {
        const ptr0 = passStringToWasm0(address, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passStringToWasm0(_id, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len1 = WASM_VECTOR_LEN;
        const ret = wasm.sekirei_change_bootnode_address(this.__wbg_ptr, ptr0, len0, ptr1, len1);
        return ret;
    }
    /**
     * @returns {Promise<bigint>}
     */
    get_ongoing_connections() {
        const ret = wasm.sekirei_get_ongoing_connections(this.__wbg_ptr);
        return ret;
    }
    /**
     * @param {string} _st
     * @returns {Sekirei}
     */
    static new(_st) {
        const ptr0 = passStringToWasm0(_st, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.sekirei_new(ptr0, len0);
        return Sekirei.__wrap(ret);
    }
    /**
     * @param {string} _st
     * @returns {Promise<void>}
     */
    run(_st) {
        const ptr0 = passStringToWasm0(_st, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.sekirei_run(this.__wbg_ptr, ptr0, len0);
        return ret;
    }
    /**
     * @param {string} address
     * @returns {Promise<Uint8Array>}
     */
    acquire(address) {
        const ptr0 = passStringToWasm0(address, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.sekirei_acquire(this.__wbg_ptr, ptr0, len0);
        return ret;
    }
}
if (Symbol.dispose) Sekirei.prototype[Symbol.dispose] = Sekirei.prototype.free;

const SekireiNo103Finalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_sekireino103_free(ptr >>> 0, 1));

export class SekireiNo103 {

    __destroy_into_raw() {
        const ptr = this.__wbg_ptr;
        this.__wbg_ptr = 0;
        SekireiNo103Finalization.unregister(this);
        return ptr;
    }

    free() {
        const ptr = this.__destroy_into_raw();
        wasm.__wbg_sekireino103_free(ptr, 0);
    }
    /**
     * @returns {Promise<object>}
     */
    resetStamp() {
        const ret = wasm.sekireino103_resetStamp(this.__wbg_ptr);
        return ret;
    }
    constructor() {
        const ret = wasm.sekireino103_new();
        this.__wbg_ptr = ret >>> 0;
        SekireiNo103Finalization.register(this, this.__wbg_ptr, this);
        return this;
    }
    /**
     * @param {string} bootnode_multiaddr
     * @param {string} network_id
     */
    start(bootnode_multiaddr, network_id) {
        const ptr0 = passStringToWasm0(bootnode_multiaddr, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passStringToWasm0(network_id, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len1 = WASM_VECTOR_LEN;
        wasm.sekireino103_start(this.__wbg_ptr, ptr0, len0, ptr1, len1);
    }
    /**
     * @param {File} file
     * @param {boolean} encryption
     * @param {string} index_string
     * @param {boolean} add_to_feed
     * @param {string} feed_topic
     * @returns {Promise<object>}
     */
    upload(file, encryption, index_string, add_to_feed, feed_topic) {
        const ptr0 = passStringToWasm0(index_string, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passStringToWasm0(feed_topic, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len1 = WASM_VECTOR_LEN;
        const ret = wasm.sekireino103_upload(this.__wbg_ptr, file, encryption, ptr0, len0, add_to_feed, ptr1, len1);
        return ret;
    }
    /**
     * @param {string} address
     * @returns {Promise<Array<any>>}
     */
    retrieve(address) {
        const ptr0 = passStringToWasm0(address, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.sekireino103_retrieve(this.__wbg_ptr, ptr0, len0);
        return ret;
    }
}
if (Symbol.dispose) SekireiNo103.prototype[Symbol.dispose] = SekireiNo103.prototype.free;

const WingsFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_wings_free(ptr >>> 0, 1));

export class Wings {

    __destroy_into_raw() {
        const ptr = this.__wbg_ptr;
        this.__wbg_ptr = 0;
        WingsFinalization.unregister(this);
        return ptr;
    }

    free() {
        const ptr = this.__destroy_into_raw();
        wasm.__wbg_wings_free(ptr, 0);
    }
}
if (Symbol.dispose) Wings.prototype[Symbol.dispose] = Wings.prototype.free;

const EXPECTED_RESPONSE_TYPES = new Set(['basic', 'cors', 'default']);

async function __wbg_load(module, imports) {
    if (typeof Response === 'function' && module instanceof Response) {
        if (typeof WebAssembly.instantiateStreaming === 'function') {
            try {
                return await WebAssembly.instantiateStreaming(module, imports);

            } catch (e) {
                const validResponse = module.ok && EXPECTED_RESPONSE_TYPES.has(module.type);

                if (validResponse && module.headers.get('Content-Type') !== 'application/wasm') {
                    console.warn("`WebAssembly.instantiateStreaming` failed because your server does not serve Wasm with `application/wasm` MIME type. Falling back to `WebAssembly.instantiate` which is slower. Original error:\n", e);

                } else {
                    throw e;
                }
            }
        }

        const bytes = await module.arrayBuffer();
        return await WebAssembly.instantiate(bytes, imports);

    } else {
        const instance = await WebAssembly.instantiate(module, imports);

        if (instance instanceof WebAssembly.Instance) {
            return { instance, module };

        } else {
            return instance;
        }
    }
}

function __wbg_get_imports() {
    const imports = {};
    imports.wbg = {};
    imports.wbg.__wbg_Error_e83987f665cf5504 = function(arg0, arg1) {
        const ret = Error(getStringFromWasm0(arg0, arg1));
        return ret;
    };
    imports.wbg.__wbg_Number_bb48ca12f395cd08 = function(arg0) {
        const ret = Number(arg0);
        return ret;
    };
    imports.wbg.__wbg_String_8f0eb39a4a4c2f66 = function(arg0, arg1) {
        const ret = String(arg1);
        const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len1 = WASM_VECTOR_LEN;
        getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
        getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
    };
    imports.wbg.__wbg_Window_41559019033ede94 = function(arg0) {
        const ret = arg0.Window;
        return ret;
    };
    imports.wbg.__wbg_WorkerGlobalScope_d324bffbeaef9f3a = function(arg0) {
        const ret = arg0.WorkerGlobalScope;
        return ret;
    };
    imports.wbg.__wbg___wbindgen_bigint_get_as_i64_f3ebc5a755000afd = function(arg0, arg1) {
        const v = arg1;
        const ret = typeof(v) === 'bigint' ? v : undefined;
        getDataViewMemory0().setBigInt64(arg0 + 8 * 1, isLikeNone(ret) ? BigInt(0) : ret, true);
        getDataViewMemory0().setInt32(arg0 + 4 * 0, !isLikeNone(ret), true);
    };
    imports.wbg.__wbg___wbindgen_boolean_get_6d5a1ee65bab5f68 = function(arg0) {
        const v = arg0;
        const ret = typeof(v) === 'boolean' ? v : undefined;
        return isLikeNone(ret) ? 0xFFFFFF : ret ? 1 : 0;
    };
    imports.wbg.__wbg___wbindgen_debug_string_df47ffb5e35e6763 = function(arg0, arg1) {
        const ret = debugString(arg1);
        const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len1 = WASM_VECTOR_LEN;
        getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
        getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
    };
    imports.wbg.__wbg___wbindgen_in_bb933bd9e1b3bc0f = function(arg0, arg1) {
        const ret = arg0 in arg1;
        return ret;
    };
    imports.wbg.__wbg___wbindgen_is_bigint_cb320707dcd35f0b = function(arg0) {
        const ret = typeof(arg0) === 'bigint';
        return ret;
    };
    imports.wbg.__wbg___wbindgen_is_function_ee8a6c5833c90377 = function(arg0) {
        const ret = typeof(arg0) === 'function';
        return ret;
    };
    imports.wbg.__wbg___wbindgen_is_null_5e69f72e906cc57c = function(arg0) {
        const ret = arg0 === null;
        return ret;
    };
    imports.wbg.__wbg___wbindgen_is_object_c818261d21f283a4 = function(arg0) {
        const val = arg0;
        const ret = typeof(val) === 'object' && val !== null;
        return ret;
    };
    imports.wbg.__wbg___wbindgen_is_string_fbb76cb2940daafd = function(arg0) {
        const ret = typeof(arg0) === 'string';
        return ret;
    };
    imports.wbg.__wbg___wbindgen_is_undefined_2d472862bd29a478 = function(arg0) {
        const ret = arg0 === undefined;
        return ret;
    };
    imports.wbg.__wbg___wbindgen_jsval_eq_6b13ab83478b1c50 = function(arg0, arg1) {
        const ret = arg0 === arg1;
        return ret;
    };
    imports.wbg.__wbg___wbindgen_jsval_loose_eq_b664b38a2f582147 = function(arg0, arg1) {
        const ret = arg0 == arg1;
        return ret;
    };
    imports.wbg.__wbg___wbindgen_number_get_a20bf9b85341449d = function(arg0, arg1) {
        const obj = arg1;
        const ret = typeof(obj) === 'number' ? obj : undefined;
        getDataViewMemory0().setFloat64(arg0 + 8 * 1, isLikeNone(ret) ? 0 : ret, true);
        getDataViewMemory0().setInt32(arg0 + 4 * 0, !isLikeNone(ret), true);
    };
    imports.wbg.__wbg___wbindgen_string_get_e4f06c90489ad01b = function(arg0, arg1) {
        const obj = arg1;
        const ret = typeof(obj) === 'string' ? obj : undefined;
        var ptr1 = isLikeNone(ret) ? 0 : passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        var len1 = WASM_VECTOR_LEN;
        getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
        getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
    };
    imports.wbg.__wbg___wbindgen_throw_b855445ff6a94295 = function(arg0, arg1) {
        throw new Error(getStringFromWasm0(arg0, arg1));
    };
    imports.wbg.__wbg__wbg_cb_unref_2454a539ea5790d9 = function(arg0) {
        arg0._wbg_cb_unref();
    };
    imports.wbg.__wbg_abort_3b256cd5ad0ac232 = function() { return handleError(function (arg0) {
        arg0.abort();
    }, arguments) };
    imports.wbg.__wbg_abort_e7eb059f72f9ed0c = function(arg0) {
        arg0.abort();
    };
    imports.wbg.__wbg_active_7414865d0c117b43 = function(arg0) {
        const ret = arg0.active;
        return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
    };
    imports.wbg.__wbg_addEventListener_7a418931447b2eae = function() { return handleError(function (arg0, arg1, arg2, arg3) {
        arg0.addEventListener(getStringFromWasm0(arg1, arg2), arg3);
    }, arguments) };
    imports.wbg.__wbg_alert_d3cd45b8012d46c7 = function() { return handleError(function (arg0, arg1, arg2) {
        arg0.alert(getStringFromWasm0(arg1, arg2));
    }, arguments) };
    imports.wbg.__wbg_append_892c5e2d5bdd60ac = function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5) {
        arg0.append(getStringFromWasm0(arg1, arg2), arg3, getStringFromWasm0(arg4, arg5));
    }, arguments) };
    imports.wbg.__wbg_append_b577eb3a177bc0fa = function() { return handleError(function (arg0, arg1, arg2, arg3, arg4) {
        arg0.append(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4));
    }, arguments) };
    imports.wbg.__wbg_arrayBuffer_5930938a049abc90 = function(arg0) {
        const ret = arg0.arrayBuffer();
        return ret;
    };
    imports.wbg.__wbg_arrayBuffer_b375eccb84b4ddf3 = function() { return handleError(function (arg0) {
        const ret = arg0.arrayBuffer();
        return ret;
    }, arguments) };
    imports.wbg.__wbg_bound_7b891dbfb3be3593 = function() { return handleError(function (arg0, arg1, arg2, arg3) {
        const ret = IDBKeyRange.bound(arg0, arg1, arg2 !== 0, arg3 !== 0);
        return ret;
    }, arguments) };
    imports.wbg.__wbg_bufferedAmount_816db4eb90c958bc = function(arg0) {
        const ret = arg0.bufferedAmount;
        return ret;
    };
    imports.wbg.__wbg_call_525440f72fbfc0ea = function() { return handleError(function (arg0, arg1, arg2) {
        const ret = arg0.call(arg1, arg2);
        return ret;
    }, arguments) };
    imports.wbg.__wbg_call_e762c39fa8ea36bf = function() { return handleError(function (arg0, arg1) {
        const ret = arg0.call(arg1);
        return ret;
    }, arguments) };
    imports.wbg.__wbg_channel_592ae18783344afb = function(arg0) {
        const ret = arg0.channel;
        return ret;
    };
    imports.wbg.__wbg_checked_385e7aee6e569db9 = function(arg0) {
        const ret = arg0.checked;
        return ret;
    };
    imports.wbg.__wbg_clearTimeout_5a54f8841c30079a = function(arg0) {
        const ret = clearTimeout(arg0);
        return ret;
    };
    imports.wbg.__wbg_clearTimeout_96804de0ab838f26 = function(arg0) {
        const ret = clearTimeout(arg0);
        return ret;
    };
    imports.wbg.__wbg_close_74386af11ef5ae35 = function(arg0) {
        arg0.close();
    };
    imports.wbg.__wbg_close_edf4a14b3cb82bca = function(arg0) {
        arg0.close();
    };
    imports.wbg.__wbg_commit_a54edce65f3858f2 = function() { return handleError(function (arg0) {
        arg0.commit();
    }, arguments) };
    imports.wbg.__wbg_confirm_1724ed383c0182fc = function() { return handleError(function (arg0, arg1, arg2) {
        const ret = arg0.confirm(getStringFromWasm0(arg1, arg2));
        return ret;
    }, arguments) };
    imports.wbg.__wbg_createDataChannel_2c80fcae96c3f118 = function(arg0, arg1, arg2) {
        const ret = arg0.createDataChannel(getStringFromWasm0(arg1, arg2));
        return ret;
    };
    imports.wbg.__wbg_createDataChannel_a665fa411be65f99 = function(arg0, arg1, arg2, arg3) {
        const ret = arg0.createDataChannel(getStringFromWasm0(arg1, arg2), arg3);
        return ret;
    };
    imports.wbg.__wbg_createElement_964ab674a0176cd8 = function() { return handleError(function (arg0, arg1, arg2) {
        const ret = arg0.createElement(getStringFromWasm0(arg1, arg2));
        return ret;
    }, arguments) };
    imports.wbg.__wbg_createObjectStore_283a43a822bf49ca = function() { return handleError(function (arg0, arg1, arg2, arg3) {
        const ret = arg0.createObjectStore(getStringFromWasm0(arg1, arg2), arg3);
        return ret;
    }, arguments) };
    imports.wbg.__wbg_createObjectURL_6c6dec873acec30b = function() { return handleError(function (arg0, arg1) {
        const ret = URL.createObjectURL(arg1);
        const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len1 = WASM_VECTOR_LEN;
        getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
        getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
    }, arguments) };
    imports.wbg.__wbg_createOffer_f0003c393c14496b = function(arg0) {
        const ret = arg0.createOffer();
        return ret;
    };
    imports.wbg.__wbg_crypto_574e78ad8b13b65f = function(arg0) {
        const ret = arg0.crypto;
        return ret;
    };
    imports.wbg.__wbg_data_ee4306d069f24f2d = function(arg0) {
        const ret = arg0.data;
        return ret;
    };
    imports.wbg.__wbg_deleteDatabase_5717f87a73ae5ac5 = function() { return handleError(function (arg0, arg1, arg2) {
        const ret = arg0.deleteDatabase(getStringFromWasm0(arg1, arg2));
        return ret;
    }, arguments) };
    imports.wbg.__wbg_document_725ae06eb442a6db = function(arg0) {
        const ret = arg0.document;
        return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
    };
    imports.wbg.__wbg_done_2042aa2670fb1db1 = function(arg0) {
        const ret = arg0.done;
        return ret;
    };
    imports.wbg.__wbg_entries_e171b586f8f6bdbf = function(arg0) {
        const ret = Object.entries(arg0);
        return ret;
    };
    imports.wbg.__wbg_error_3e929987fcd3e155 = function() { return handleError(function (arg0) {
        const ret = arg0.error;
        return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
    }, arguments) };
    imports.wbg.__wbg_fetch_0c645bcbfc592368 = function(arg0, arg1) {
        const ret = arg0.fetch(arg1);
        return ret;
    };
    imports.wbg.__wbg_fetch_f1856afdb49415d1 = function(arg0) {
        const ret = fetch(arg0);
        return ret;
    };
    imports.wbg.__wbg_fetch_f8ba0e29a9d6de0d = function(arg0, arg1) {
        const ret = arg0.fetch(arg1);
        return ret;
    };
    imports.wbg.__wbg_files_fa5e206b28f24ddc = function(arg0) {
        const ret = arg0.files;
        return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
    };
    imports.wbg.__wbg_from_a4ad7cbddd0d7135 = function(arg0) {
        const ret = Array.from(arg0);
        return ret;
    };
    imports.wbg.__wbg_generateCertificate_64eb5c5628ce5f5a = function() { return handleError(function (arg0) {
        const ret = RTCPeerConnection.generateCertificate(arg0);
        return ret;
    }, arguments) };
    imports.wbg.__wbg_getElementById_c365dd703c4a88c3 = function(arg0, arg1, arg2) {
        const ret = arg0.getElementById(getStringFromWasm0(arg1, arg2));
        return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
    };
    imports.wbg.__wbg_getRandomValues_b8f5dbd5f3995a9e = function() { return handleError(function (arg0, arg1) {
        arg0.getRandomValues(arg1);
    }, arguments) };
    imports.wbg.__wbg_getRegistration_95b8acc0eb1b1d2b = function(arg0) {
        const ret = arg0.getRegistration();
        return ret;
    };
    imports.wbg.__wbg_get_7bed016f185add81 = function(arg0, arg1) {
        const ret = arg0[arg1 >>> 0];
        return ret;
    };
    imports.wbg.__wbg_get_efcb449f58ec27c2 = function() { return handleError(function (arg0, arg1) {
        const ret = Reflect.get(arg0, arg1);
        return ret;
    }, arguments) };
    imports.wbg.__wbg_get_fb1fa70beb44a754 = function() { return handleError(function (arg0, arg1) {
        const ret = arg0.get(arg1);
        return ret;
    }, arguments) };
    imports.wbg.__wbg_get_provider_js_3e4ff84c8bf90fbc = function() { return handleError(function () {
        const ret = get_provider_js();
        return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
    }, arguments) };
    imports.wbg.__wbg_get_with_ref_key_1dc361bd10053bfe = function(arg0, arg1) {
        const ret = arg0[arg1];
        return ret;
    };
    imports.wbg.__wbg_global_f5c2926e57ba457f = function(arg0) {
        const ret = arg0.global;
        return ret;
    };
    imports.wbg.__wbg_has_787fafc980c3ccdb = function() { return handleError(function (arg0, arg1) {
        const ret = Reflect.has(arg0, arg1);
        return ret;
    }, arguments) };
    imports.wbg.__wbg_headers_b87d7eaba61c3278 = function(arg0) {
        const ret = arg0.headers;
        return ret;
    };
    imports.wbg.__wbg_history_bf649e08118bb039 = function() { return handleError(function (arg0) {
        const ret = arg0.history;
        return ret;
    }, arguments) };
    imports.wbg.__wbg_hostname_b3afa4677fba29d1 = function() { return handleError(function (arg0, arg1) {
        const ret = arg1.hostname;
        const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len1 = WASM_VECTOR_LEN;
        getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
        getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
    }, arguments) };
    imports.wbg.__wbg_href_6d02c53ff820b6ae = function() { return handleError(function (arg0, arg1) {
        const ret = arg1.href;
        const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len1 = WASM_VECTOR_LEN;
        getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
        getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
    }, arguments) };
    imports.wbg.__wbg_indexedDB_54f01430b1e194e8 = function() { return handleError(function (arg0) {
        const ret = arg0.indexedDB;
        return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
    }, arguments) };
    imports.wbg.__wbg_indexedDB_62bfbbd55ec74b14 = function() { return handleError(function (arg0) {
        const ret = arg0.indexedDB;
        return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
    }, arguments) };
    imports.wbg.__wbg_indexedDB_8b464318fe56681e = function() { return handleError(function (arg0) {
        const ret = arg0.indexedDB;
        return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
    }, arguments) };
    imports.wbg.__wbg_instanceof_ArrayBuffer_70beb1189ca63b38 = function(arg0) {
        let result;
        try {
            result = arg0 instanceof ArrayBuffer;
        } catch (_) {
            result = false;
        }
        const ret = result;
        return ret;
    };
    imports.wbg.__wbg_instanceof_DomException_83b15e7b042a0b1a = function(arg0) {
        let result;
        try {
            result = arg0 instanceof DOMException;
        } catch (_) {
            result = false;
        }
        const ret = result;
        return ret;
    };
    imports.wbg.__wbg_instanceof_Error_a944ec10920129e2 = function(arg0) {
        let result;
        try {
            result = arg0 instanceof Error;
        } catch (_) {
            result = false;
        }
        const ret = result;
        return ret;
    };
    imports.wbg.__wbg_instanceof_File_b7e4dbeba5bb2cd0 = function(arg0) {
        let result;
        try {
            result = arg0 instanceof File;
        } catch (_) {
            result = false;
        }
        const ret = result;
        return ret;
    };
    imports.wbg.__wbg_instanceof_HtmlButtonElement_14067070682a44d6 = function(arg0) {
        let result;
        try {
            result = arg0 instanceof HTMLButtonElement;
        } catch (_) {
            result = false;
        }
        const ret = result;
        return ret;
    };
    imports.wbg.__wbg_instanceof_HtmlElement_e20a729df22f9e1c = function(arg0) {
        let result;
        try {
            result = arg0 instanceof HTMLElement;
        } catch (_) {
            result = false;
        }
        const ret = result;
        return ret;
    };
    imports.wbg.__wbg_instanceof_HtmlInputElement_b8672abb32fe4ab7 = function(arg0) {
        let result;
        try {
            result = arg0 instanceof HTMLInputElement;
        } catch (_) {
            result = false;
        }
        const ret = result;
        return ret;
    };
    imports.wbg.__wbg_instanceof_HtmlSelectElement_6b571cc4aa3fe771 = function(arg0) {
        let result;
        try {
            result = arg0 instanceof HTMLSelectElement;
        } catch (_) {
            result = false;
        }
        const ret = result;
        return ret;
    };
    imports.wbg.__wbg_instanceof_HtmlSpanElement_dae50b187d8640b4 = function(arg0) {
        let result;
        try {
            result = arg0 instanceof HTMLSpanElement;
        } catch (_) {
            result = false;
        }
        const ret = result;
        return ret;
    };
    imports.wbg.__wbg_instanceof_IdbDatabase_fcf75ffeeec3ec8c = function(arg0) {
        let result;
        try {
            result = arg0 instanceof IDBDatabase;
        } catch (_) {
            result = false;
        }
        const ret = result;
        return ret;
    };
    imports.wbg.__wbg_instanceof_IdbRequest_26754883a3cc8f81 = function(arg0) {
        let result;
        try {
            result = arg0 instanceof IDBRequest;
        } catch (_) {
            result = false;
        }
        const ret = result;
        return ret;
    };
    imports.wbg.__wbg_instanceof_Map_8579b5e2ab5437c7 = function(arg0) {
        let result;
        try {
            result = arg0 instanceof Map;
        } catch (_) {
            result = false;
        }
        const ret = result;
        return ret;
    };
    imports.wbg.__wbg_instanceof_MessagePort_1c505a864768efba = function(arg0) {
        let result;
        try {
            result = arg0 instanceof MessagePort;
        } catch (_) {
            result = false;
        }
        const ret = result;
        return ret;
    };
    imports.wbg.__wbg_instanceof_Object_10bb762262230c68 = function(arg0) {
        let result;
        try {
            result = arg0 instanceof Object;
        } catch (_) {
            result = false;
        }
        const ret = result;
        return ret;
    };
    imports.wbg.__wbg_instanceof_Promise_001fdd42afa1b7ef = function(arg0) {
        let result;
        try {
            result = arg0 instanceof Promise;
        } catch (_) {
            result = false;
        }
        const ret = result;
        return ret;
    };
    imports.wbg.__wbg_instanceof_Response_f4f3e87e07f3135c = function(arg0) {
        let result;
        try {
            result = arg0 instanceof Response;
        } catch (_) {
            result = false;
        }
        const ret = result;
        return ret;
    };
    imports.wbg.__wbg_instanceof_ServiceWorkerRegistration_554d7aad50613d08 = function(arg0) {
        let result;
        try {
            result = arg0 instanceof ServiceWorkerRegistration;
        } catch (_) {
            result = false;
        }
        const ret = result;
        return ret;
    };
    imports.wbg.__wbg_instanceof_Uint8Array_20c8e73002f7af98 = function(arg0) {
        let result;
        try {
            result = arg0 instanceof Uint8Array;
        } catch (_) {
            result = false;
        }
        const ret = result;
        return ret;
    };
    imports.wbg.__wbg_instanceof_Window_4846dbb3de56c84c = function(arg0) {
        let result;
        try {
            result = arg0 instanceof Window;
        } catch (_) {
            result = false;
        }
        const ret = result;
        return ret;
    };
    imports.wbg.__wbg_isArray_96e0af9891d0945d = function(arg0) {
        const ret = Array.isArray(arg0);
        return ret;
    };
    imports.wbg.__wbg_isSafeInteger_d216eda7911dde36 = function(arg0) {
        const ret = Number.isSafeInteger(arg0);
        return ret;
    };
    imports.wbg.__wbg_item_e0cfb7db1926e7dd = function(arg0, arg1) {
        const ret = arg0.item(arg1 >>> 0);
        return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
    };
    imports.wbg.__wbg_iterator_e5822695327a3c39 = function() {
        const ret = Symbol.iterator;
        return ret;
    };
    imports.wbg.__wbg_length_69bca3cb64fc8748 = function(arg0) {
        const ret = arg0.length;
        return ret;
    };
    imports.wbg.__wbg_length_cdd215e10d9dd507 = function(arg0) {
        const ret = arg0.length;
        return ret;
    };
    imports.wbg.__wbg_localDescription_7af0de60abe79067 = function(arg0) {
        const ret = arg0.localDescription;
        return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
    };
    imports.wbg.__wbg_location_c6230189c545b8cc = function(arg0) {
        const ret = arg0.location;
        return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
    };
    imports.wbg.__wbg_location_ef1665506d996dd9 = function(arg0) {
        const ret = arg0.location;
        return ret;
    };
    imports.wbg.__wbg_log_8cec76766b8c0e33 = function(arg0) {
        console.log(arg0);
    };
    imports.wbg.__wbg_lowerBound_738117f2d23f503a = function() { return handleError(function (arg0, arg1) {
        const ret = IDBKeyRange.lowerBound(arg0, arg1 !== 0);
        return ret;
    }, arguments) };
    imports.wbg.__wbg_message_1ee258909d7264fd = function(arg0) {
        const ret = arg0.message;
        return ret;
    };
    imports.wbg.__wbg_message_bd42dbe3f2f3ed8e = function(arg0, arg1) {
        const ret = arg1.message;
        const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len1 = WASM_VECTOR_LEN;
        getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
        getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
    };
    imports.wbg.__wbg_msCrypto_a61aeb35a24c1329 = function(arg0) {
        const ret = arg0.msCrypto;
        return ret;
    };
    imports.wbg.__wbg_name_012808ba1253a92d = function(arg0, arg1) {
        const ret = arg1.name;
        const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len1 = WASM_VECTOR_LEN;
        getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
        getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
    };
    imports.wbg.__wbg_name_2922909227d511f5 = function(arg0, arg1) {
        const ret = arg1.name;
        const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len1 = WASM_VECTOR_LEN;
        getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
        getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
    };
    imports.wbg.__wbg_name_3a33ad25b892b2dd = function(arg0, arg1) {
        const ret = arg1.name;
        const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len1 = WASM_VECTOR_LEN;
        getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
        getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
    };
    imports.wbg.__wbg_navigator_971384882e8ea23a = function(arg0) {
        const ret = arg0.navigator;
        return ret;
    };
    imports.wbg.__wbg_newVersion_bb7fcd3b7007a3a2 = function(arg0, arg1) {
        const ret = arg1.newVersion;
        getDataViewMemory0().setFloat64(arg0 + 8 * 1, isLikeNone(ret) ? 0 : ret, true);
        getDataViewMemory0().setInt32(arg0 + 4 * 0, !isLikeNone(ret), true);
    };
    imports.wbg.__wbg_new_1acc0b6eea89d040 = function() {
        const ret = new Object();
        return ret;
    };
    imports.wbg.__wbg_new_2531773dac38ebb3 = function() { return handleError(function () {
        const ret = new AbortController();
        return ret;
    }, arguments) };
    imports.wbg.__wbg_new_3c3d849046688a66 = function(arg0, arg1) {
        try {
            var state0 = {a: arg0, b: arg1};
            var cb0 = (arg0, arg1) => {
                const a = state0.a;
                state0.a = 0;
                try {
                    return wasm_bindgen__convert__closures_____invoke__h2536b6f3fd53c579(a, state0.b, arg0, arg1);
                } finally {
                    state0.a = a;
                }
            };
            const ret = new Promise(cb0);
            return ret;
        } finally {
            state0.a = state0.b = 0;
        }
    };
    imports.wbg.__wbg_new_5a79be3ab53b8aa5 = function(arg0) {
        const ret = new Uint8Array(arg0);
        return ret;
    };
    imports.wbg.__wbg_new_68651c719dcda04e = function() {
        const ret = new Map();
        return ret;
    };
    imports.wbg.__wbg_new_6f694bb0585846e0 = function() { return handleError(function () {
        const ret = new FormData();
        return ret;
    }, arguments) };
    imports.wbg.__wbg_new_9edf9838a2def39c = function() { return handleError(function () {
        const ret = new Headers();
        return ret;
    }, arguments) };
    imports.wbg.__wbg_new_a7442b4b19c1a356 = function(arg0, arg1) {
        const ret = new Error(getStringFromWasm0(arg0, arg1));
        return ret;
    };
    imports.wbg.__wbg_new_e17d9f43105b08be = function() {
        const ret = new Array();
        return ret;
    };
    imports.wbg.__wbg_new_from_slice_92f4d78ca282a2d2 = function(arg0, arg1) {
        const ret = new Uint8Array(getArrayU8FromWasm0(arg0, arg1));
        return ret;
    };
    imports.wbg.__wbg_new_no_args_ee98eee5275000a4 = function(arg0, arg1) {
        const ret = new Function(getStringFromWasm0(arg0, arg1));
        return ret;
    };
    imports.wbg.__wbg_new_with_configuration_a1cd3f4d87d41876 = function() { return handleError(function (arg0) {
        const ret = new RTCPeerConnection(arg0);
        return ret;
    }, arguments) };
    imports.wbg.__wbg_new_with_length_01aa0dc35aa13543 = function(arg0) {
        const ret = new Uint8Array(arg0 >>> 0);
        return ret;
    };
    imports.wbg.__wbg_new_with_str_and_init_0ae7728b6ec367b1 = function() { return handleError(function (arg0, arg1, arg2) {
        const ret = new Request(getStringFromWasm0(arg0, arg1), arg2);
        return ret;
    }, arguments) };
    imports.wbg.__wbg_new_with_u8_array_sequence_and_options_0c1d0bd56d93d25a = function() { return handleError(function (arg0, arg1) {
        const ret = new Blob(arg0, arg1);
        return ret;
    }, arguments) };
    imports.wbg.__wbg_new_with_u8_array_sequence_and_options_a6e99d59206a642c = function() { return handleError(function (arg0, arg1, arg2, arg3) {
        const ret = new File(arg0, getStringFromWasm0(arg1, arg2), arg3);
        return ret;
    }, arguments) };
    imports.wbg.__wbg_next_020810e0ae8ebcb0 = function() { return handleError(function (arg0) {
        const ret = arg0.next();
        return ret;
    }, arguments) };
    imports.wbg.__wbg_next_2c826fe5dfec6b6a = function(arg0) {
        const ret = arg0.next;
        return ret;
    };
    imports.wbg.__wbg_node_905d3e251edff8a2 = function(arg0) {
        const ret = arg0.node;
        return ret;
    };
    imports.wbg.__wbg_now_2c95c9de01293173 = function(arg0) {
        const ret = arg0.now();
        return ret;
    };
    imports.wbg.__wbg_now_793306c526e2e3b6 = function() {
        const ret = Date.now();
        return ret;
    };
    imports.wbg.__wbg_objectStore_2aab1d8b165c62a6 = function() { return handleError(function (arg0, arg1, arg2) {
        const ret = arg0.objectStore(getStringFromWasm0(arg1, arg2));
        return ret;
    }, arguments) };
    imports.wbg.__wbg_oldVersion_54e2e7cdf22e05ff = function(arg0) {
        const ret = arg0.oldVersion;
        return ret;
    };
    imports.wbg.__wbg_on_eb7d13044c1fcf88 = function(arg0, arg1, arg2, arg3) {
        arg0.on(getStringFromWasm0(arg1, arg2), arg3);
    };
    imports.wbg.__wbg_open_9d8c51d122a5a6ea = function() { return handleError(function (arg0, arg1, arg2, arg3) {
        const ret = arg0.open(getStringFromWasm0(arg1, arg2), arg3 >>> 0);
        return ret;
    }, arguments) };
    imports.wbg.__wbg_open_a36354e60d7255fb = function() { return handleError(function (arg0, arg1, arg2) {
        const ret = arg0.open(getStringFromWasm0(arg1, arg2));
        return ret;
    }, arguments) };
    imports.wbg.__wbg_origin_2b5e7986f349f4f3 = function() { return handleError(function (arg0, arg1) {
        const ret = arg1.origin;
        const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len1 = WASM_VECTOR_LEN;
        getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
        getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
    }, arguments) };
    imports.wbg.__wbg_pathname_64db68c363ed9ab1 = function() { return handleError(function (arg0, arg1) {
        const ret = arg1.pathname;
        const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len1 = WASM_VECTOR_LEN;
        getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
        getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
    }, arguments) };
    imports.wbg.__wbg_performance_7a3ffd0b17f663ad = function(arg0) {
        const ret = arg0.performance;
        return ret;
    };
    imports.wbg.__wbg_ports_8e40765dbd2e03da = function(arg0) {
        const ret = arg0.ports;
        return ret;
    };
    imports.wbg.__wbg_postMessage_9fc51e3de91252bb = function() { return handleError(function (arg0, arg1) {
        arg0.postMessage(arg1);
    }, arguments) };
    imports.wbg.__wbg_postMessage_cea6d6f3b452f8bc = function() { return handleError(function (arg0, arg1) {
        arg0.postMessage(arg1);
    }, arguments) };
    imports.wbg.__wbg_prepend_b68199ea7a2dc28f = function() { return handleError(function (arg0, arg1) {
        arg0.prepend(arg1);
    }, arguments) };
    imports.wbg.__wbg_process_dc0fbacc7c1c06f7 = function(arg0) {
        const ret = arg0.process;
        return ret;
    };
    imports.wbg.__wbg_prototypesetcall_2a6620b6922694b2 = function(arg0, arg1, arg2) {
        Uint8Array.prototype.set.call(getArrayU8FromWasm0(arg0, arg1), arg2);
    };
    imports.wbg.__wbg_push_df81a39d04db858c = function(arg0, arg1) {
        const ret = arg0.push(arg1);
        return ret;
    };
    imports.wbg.__wbg_put_88678dd575c85637 = function() { return handleError(function (arg0, arg1, arg2) {
        const ret = arg0.put(arg1, arg2);
        return ret;
    }, arguments) };
    imports.wbg.__wbg_queueMicrotask_34d692c25c47d05b = function(arg0) {
        const ret = arg0.queueMicrotask;
        return ret;
    };
    imports.wbg.__wbg_queueMicrotask_9d76cacb20c84d58 = function(arg0) {
        queueMicrotask(arg0);
    };
    imports.wbg.__wbg_randomFillSync_ac0988aba3254290 = function() { return handleError(function (arg0, arg1) {
        arg0.randomFillSync(arg1);
    }, arguments) };
    imports.wbg.__wbg_readyState_05f4e3fc5e12587a = function(arg0) {
        const ret = arg0.readyState;
        return (__wbindgen_enum_RtcDataChannelState.indexOf(ret) + 1 || 5) - 1;
    };
    imports.wbg.__wbg_readyState_51d79bf8ebb8a05c = function(arg0) {
        const ret = arg0.readyState;
        return (__wbindgen_enum_IdbRequestReadyState.indexOf(ret) + 1 || 3) - 1;
    };
    imports.wbg.__wbg_ready_c418f356e6c91d23 = function() { return handleError(function (arg0) {
        const ret = arg0.ready;
        return ret;
    }, arguments) };
    imports.wbg.__wbg_register_9d2b67c40603092a = function(arg0, arg1, arg2) {
        const ret = arg0.register(getStringFromWasm0(arg1, arg2));
        return ret;
    };
    imports.wbg.__wbg_removeListener_83803bf863b12f7e = function(arg0, arg1, arg2, arg3) {
        arg0.removeListener(getStringFromWasm0(arg1, arg2), arg3);
    };
    imports.wbg.__wbg_replaceState_7f1e98a37cbc7559 = function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5) {
        arg0.replaceState(arg1, getStringFromWasm0(arg2, arg3), arg4 === 0 ? undefined : getStringFromWasm0(arg4, arg5));
    }, arguments) };
    imports.wbg.__wbg_request_7a4a4d6acbd1d3e6 = function() { return handleError(function (arg0, arg1) {
        const ret = arg0.request(RequestArguments.__wrap(arg1));
        return ret;
    }, arguments) };
    imports.wbg.__wbg_require_60cc747a6bc5215a = function() { return handleError(function () {
        const ret = module.require;
        return ret;
    }, arguments) };
    imports.wbg.__wbg_resolve_caf97c30b83f7053 = function(arg0) {
        const ret = Promise.resolve(arg0);
        return ret;
    };
    imports.wbg.__wbg_result_25e75004b82b9830 = function() { return handleError(function (arg0) {
        const ret = arg0.result;
        return ret;
    }, arguments) };
    imports.wbg.__wbg_sdp_10af2f75eeec709f = function(arg0, arg1) {
        const ret = arg1.sdp;
        const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len1 = WASM_VECTOR_LEN;
        getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
        getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
    };
    imports.wbg.__wbg_send_3d93963ba80250ff = function() { return handleError(function (arg0, arg1, arg2) {
        arg0.send(getArrayU8FromWasm0(arg1, arg2));
    }, arguments) };
    imports.wbg.__wbg_serviceWorker_30be3530c5eb9da9 = function(arg0) {
        const ret = arg0.serviceWorker;
        return ret;
    };
    imports.wbg.__wbg_setAttribute_9bad76f39609daac = function() { return handleError(function (arg0, arg1, arg2, arg3, arg4) {
        arg0.setAttribute(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4));
    }, arguments) };
    imports.wbg.__wbg_setLocalDescription_65cd8a6ccfe86e28 = function(arg0, arg1) {
        const ret = arg0.setLocalDescription(arg1);
        return ret;
    };
    imports.wbg.__wbg_setRemoteDescription_f77d823b45b689fb = function(arg0, arg1) {
        const ret = arg0.setRemoteDescription(arg1);
        return ret;
    };
    imports.wbg.__wbg_setTimeout_db2dbaeefb6f39c7 = function() { return handleError(function (arg0, arg1) {
        const ret = setTimeout(arg0, arg1);
        return ret;
    }, arguments) };
    imports.wbg.__wbg_setTimeout_eefe7f4c234b0c6b = function() { return handleError(function (arg0, arg1) {
        const ret = setTimeout(arg0, arg1);
        return ret;
    }, arguments) };
    imports.wbg.__wbg_set_3f1d0b984ed272ed = function(arg0, arg1, arg2) {
        arg0[arg1] = arg2;
    };
    imports.wbg.__wbg_set_8b342d8cd9d2a02c = function() { return handleError(function (arg0, arg1, arg2, arg3, arg4) {
        arg0.set(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4));
    }, arguments) };
    imports.wbg.__wbg_set_907fb406c34a251d = function(arg0, arg1, arg2) {
        const ret = arg0.set(arg1, arg2);
        return ret;
    };
    imports.wbg.__wbg_set_9e6516df7b7d0f19 = function(arg0, arg1, arg2) {
        arg0.set(getArrayU8FromWasm0(arg1, arg2));
    };
    imports.wbg.__wbg_set_auto_increment_f44ca0bef52b71d4 = function(arg0, arg1) {
        arg0.autoIncrement = arg1 !== 0;
    };
    imports.wbg.__wbg_set_binaryType_6a205bbace90450a = function(arg0, arg1) {
        arg0.binaryType = __wbindgen_enum_RtcDataChannelType[arg1];
    };
    imports.wbg.__wbg_set_body_3c365989753d61f4 = function(arg0, arg1) {
        arg0.body = arg1;
    };
    imports.wbg.__wbg_set_bufferedAmountLowThreshold_deaa9786f7682277 = function(arg0, arg1) {
        arg0.bufferedAmountLowThreshold = arg1 >>> 0;
    };
    imports.wbg.__wbg_set_c213c871859d6500 = function(arg0, arg1, arg2) {
        arg0[arg1 >>> 0] = arg2;
    };
    imports.wbg.__wbg_set_c2abbebe8b9ebee1 = function() { return handleError(function (arg0, arg1, arg2) {
        const ret = Reflect.set(arg0, arg1, arg2);
        return ret;
    }, arguments) };
    imports.wbg.__wbg_set_certificates_b1828928a6aa2347 = function(arg0, arg1) {
        arg0.certificates = arg1;
    };
    imports.wbg.__wbg_set_credentials_f621cd2d85c0c228 = function(arg0, arg1) {
        arg0.credentials = __wbindgen_enum_RequestCredentials[arg1];
    };
    imports.wbg.__wbg_set_headers_6926da238cd32ee4 = function(arg0, arg1) {
        arg0.headers = arg1;
    };
    imports.wbg.__wbg_set_id_cee4913343308bcf = function(arg0, arg1) {
        arg0.id = arg1;
    };
    imports.wbg.__wbg_set_innerHTML_fb5a7e25198fc344 = function(arg0, arg1, arg2) {
        arg0.innerHTML = getStringFromWasm0(arg1, arg2);
    };
    imports.wbg.__wbg_set_last_modified_b39676546f08496d = function(arg0, arg1) {
        arg0.lastModified = arg1;
    };
    imports.wbg.__wbg_set_method_c02d8cbbe204ac2d = function(arg0, arg1, arg2) {
        arg0.method = getStringFromWasm0(arg1, arg2);
    };
    imports.wbg.__wbg_set_mode_52ef73cfa79639cb = function(arg0, arg1) {
        arg0.mode = __wbindgen_enum_RequestMode[arg1];
    };
    imports.wbg.__wbg_set_negotiated_86e8a86b4d15cd03 = function(arg0, arg1) {
        arg0.negotiated = arg1 !== 0;
    };
    imports.wbg.__wbg_set_onabort_6957ef4f3e5c91eb = function(arg0, arg1) {
        arg0.onabort = arg1;
    };
    imports.wbg.__wbg_set_onbufferedamountlow_ef7dc077f5c1c670 = function(arg0, arg1) {
        arg0.onbufferedamountlow = arg1;
    };
    imports.wbg.__wbg_set_onclick_c8804e02c9e814a7 = function(arg0, arg1) {
        arg0.onclick = arg1;
    };
    imports.wbg.__wbg_set_onclose_96fc333f2e09724c = function(arg0, arg1) {
        arg0.onclose = arg1;
    };
    imports.wbg.__wbg_set_oncomplete_71dbeb19a31158ae = function(arg0, arg1) {
        arg0.oncomplete = arg1;
    };
    imports.wbg.__wbg_set_ondatachannel_d3587e15167e215b = function(arg0, arg1) {
        arg0.ondatachannel = arg1;
    };
    imports.wbg.__wbg_set_onerror_2a8ad6135dc1ec74 = function(arg0, arg1) {
        arg0.onerror = arg1;
    };
    imports.wbg.__wbg_set_onerror_dc82fea584ffccaa = function(arg0, arg1) {
        arg0.onerror = arg1;
    };
    imports.wbg.__wbg_set_oninput_5e2c39b1271c87e0 = function(arg0, arg1) {
        arg0.oninput = arg1;
    };
    imports.wbg.__wbg_set_onmessage_3a8d6c4b2e1962d7 = function(arg0, arg1) {
        arg0.onmessage = arg1;
    };
    imports.wbg.__wbg_set_onopen_ac1685e688d0fbcf = function(arg0, arg1) {
        arg0.onopen = arg1;
    };
    imports.wbg.__wbg_set_onsuccess_f367d002b462109e = function(arg0, arg1) {
        arg0.onsuccess = arg1;
    };
    imports.wbg.__wbg_set_onupgradeneeded_0a519a73284a1418 = function(arg0, arg1) {
        arg0.onupgradeneeded = arg1;
    };
    imports.wbg.__wbg_set_sdp_b8e3939b73c0ad0f = function(arg0, arg1, arg2) {
        arg0.sdp = getStringFromWasm0(arg1, arg2);
    };
    imports.wbg.__wbg_set_signal_dda2cf7ccb6bee0f = function(arg0, arg1) {
        arg0.signal = arg1;
    };
    imports.wbg.__wbg_set_type_2e7e493a02f43e56 = function(arg0, arg1) {
        arg0.type = __wbindgen_enum_RtcSdpType[arg1];
    };
    imports.wbg.__wbg_set_type_63fa4c18251f6545 = function(arg0, arg1, arg2) {
        arg0.type = getStringFromWasm0(arg1, arg2);
    };
    imports.wbg.__wbg_set_type_ae55c1868f94fdd1 = function(arg0, arg1, arg2) {
        arg0.type = getStringFromWasm0(arg1, arg2);
    };
    imports.wbg.__wbg_signal_4db5aa055bf9eb9a = function(arg0) {
        const ret = arg0.signal;
        return ret;
    };
    imports.wbg.__wbg_size_0a5a003dbf5dfee8 = function(arg0) {
        const ret = arg0.size;
        return ret;
    };
    imports.wbg.__wbg_slice_4947c0e051116da5 = function() { return handleError(function (arg0, arg1, arg2) {
        const ret = arg0.slice(arg1, arg2);
        return ret;
    }, arguments) };
    imports.wbg.__wbg_static_accessor_GLOBAL_89e1d9ac6a1b250e = function() {
        const ret = typeof global === 'undefined' ? null : global;
        return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
    };
    imports.wbg.__wbg_static_accessor_GLOBAL_THIS_8b530f326a9e48ac = function() {
        const ret = typeof globalThis === 'undefined' ? null : globalThis;
        return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
    };
    imports.wbg.__wbg_static_accessor_SELF_6fdf4b64710cc91b = function() {
        const ret = typeof self === 'undefined' ? null : self;
        return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
    };
    imports.wbg.__wbg_static_accessor_WINDOW_b45bfc5a37f6cfa2 = function() {
        const ret = typeof window === 'undefined' ? null : window;
        return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
    };
    imports.wbg.__wbg_status_de7eed5a7a5bfd5d = function(arg0) {
        const ret = arg0.status;
        return ret;
    };
    imports.wbg.__wbg_stringify_b5fb28f6465d9c3e = function() { return handleError(function (arg0) {
        const ret = JSON.stringify(arg0);
        return ret;
    }, arguments) };
    imports.wbg.__wbg_subarray_480600f3d6a9f26c = function(arg0, arg1, arg2) {
        const ret = arg0.subarray(arg1 >>> 0, arg2 >>> 0);
        return ret;
    };
    imports.wbg.__wbg_target_1447f5d3a6fa6fe0 = function(arg0) {
        const ret = arg0.target;
        return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
    };
    imports.wbg.__wbg_then_4f46f6544e6b4a28 = function(arg0, arg1) {
        const ret = arg0.then(arg1);
        return ret;
    };
    imports.wbg.__wbg_then_70d05cf780a18d77 = function(arg0, arg1, arg2) {
        const ret = arg0.then(arg1, arg2);
        return ret;
    };
    imports.wbg.__wbg_toString_7da7c8dbec78fcb8 = function(arg0) {
        const ret = arg0.toString();
        return ret;
    };
    imports.wbg.__wbg_toString_8eec07f6f4c057e4 = function(arg0) {
        const ret = arg0.toString();
        return ret;
    };
    imports.wbg.__wbg_transaction_0aa5a83862993431 = function(arg0) {
        const ret = arg0.transaction;
        return ret;
    };
    imports.wbg.__wbg_transaction_253405fd3a30ed91 = function() { return handleError(function (arg0, arg1, arg2, arg3) {
        const ret = arg0.transaction(getStringFromWasm0(arg1, arg2), __wbindgen_enum_IdbTransactionMode[arg3]);
        return ret;
    }, arguments) };
    imports.wbg.__wbg_type_d5d4dbe840f65b14 = function(arg0, arg1) {
        const ret = arg1.type;
        const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len1 = WASM_VECTOR_LEN;
        getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
        getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
    };
    imports.wbg.__wbg_update_3ae0915701ce210b = function() { return handleError(function (arg0) {
        const ret = arg0.update();
        return ret;
    }, arguments) };
    imports.wbg.__wbg_upperBound_8d0e896ba7b1d0ea = function() { return handleError(function (arg0, arg1) {
        const ret = IDBKeyRange.upperBound(arg0, arg1 !== 0);
        return ret;
    }, arguments) };
    imports.wbg.__wbg_url_b36d2a5008eb056f = function(arg0, arg1) {
        const ret = arg1.url;
        const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len1 = WASM_VECTOR_LEN;
        getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
        getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
    };
    imports.wbg.__wbg_userAgent_b20949aa6be940a6 = function() { return handleError(function (arg0, arg1) {
        const ret = arg1.userAgent;
        const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len1 = WASM_VECTOR_LEN;
        getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
        getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
    }, arguments) };
    imports.wbg.__wbg_value_692627309814bb8c = function(arg0) {
        const ret = arg0.value;
        return ret;
    };
    imports.wbg.__wbg_value_998b2dfe93506065 = function(arg0, arg1) {
        const ret = arg1.value;
        const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len1 = WASM_VECTOR_LEN;
        getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
        getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
    };
    imports.wbg.__wbg_value_f470db44e5a60ad8 = function(arg0, arg1) {
        const ret = arg1.value;
        const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len1 = WASM_VECTOR_LEN;
        getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
        getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
    };
    imports.wbg.__wbg_versions_c01dfd4722a88165 = function(arg0) {
        const ret = arg0.versions;
        return ret;
    };
    imports.wbg.__wbg_warn_1d74dddbe2fd1dbb = function(arg0) {
        console.warn(arg0);
    };
    imports.wbg.__wbindgen_cast_0927299a76e21bff = function(arg0, arg1) {
        // Cast intrinsic for `Closure(Closure { dtor_idx: 1017, function: Function { arguments: [NamedExternref("MessageEvent")], shim_idx: 1018, ret: Unit, inner_ret: Some(Unit) }, mutable: true }) -> Externref`.
        const ret = makeMutClosure(arg0, arg1, wasm.wasm_bindgen__closure__destroy__h5ad2cc04f4e806d8, wasm_bindgen__convert__closures_____invoke__h02a1a5a325cc1eca);
        return ret;
    };
    imports.wbg.__wbindgen_cast_0c5e7ae44b547949 = function(arg0, arg1) {
        // Cast intrinsic for `Closure(Closure { dtor_idx: 1737, function: Function { arguments: [Externref], shim_idx: 1738, ret: Unit, inner_ret: Some(Unit) }, mutable: true }) -> Externref`.
        const ret = makeMutClosure(arg0, arg1, wasm.wasm_bindgen__closure__destroy__h13af293e6c7c845a, wasm_bindgen__convert__closures_____invoke__h9c0ae922cc046d1d);
        return ret;
    };
    imports.wbg.__wbindgen_cast_2241b6af4c4b2941 = function(arg0, arg1) {
        // Cast intrinsic for `Ref(String) -> Externref`.
        const ret = getStringFromWasm0(arg0, arg1);
        return ret;
    };
    imports.wbg.__wbindgen_cast_25a0a844437d0e92 = function(arg0, arg1) {
        var v0 = getArrayJsValueFromWasm0(arg0, arg1).slice();
        wasm.__wbindgen_free(arg0, arg1 * 4, 4);
        // Cast intrinsic for `Vector(NamedExternref("string")) -> Externref`.
        const ret = v0;
        return ret;
    };
    imports.wbg.__wbindgen_cast_4625c577ab2ec9ee = function(arg0) {
        // Cast intrinsic for `U64 -> Externref`.
        const ret = BigInt.asUintN(64, arg0);
        return ret;
    };
    imports.wbg.__wbindgen_cast_4c1794f8c32ef0b4 = function(arg0, arg1) {
        // Cast intrinsic for `Closure(Closure { dtor_idx: 796, function: Function { arguments: [NamedExternref("Event")], shim_idx: 799, ret: Unit, inner_ret: Some(Unit) }, mutable: true }) -> Externref`.
        const ret = makeMutClosure(arg0, arg1, wasm.wasm_bindgen__closure__destroy__h4e7c9205dcc62f87, wasm_bindgen__convert__closures_____invoke__h1b9dadbe13f4df00);
        return ret;
    };
    imports.wbg.__wbindgen_cast_56d97f747ab7ffa8 = function(arg0, arg1) {
        // Cast intrinsic for `Closure(Closure { dtor_idx: 1756, function: Function { arguments: [], shim_idx: 1757, ret: Unit, inner_ret: Some(Unit) }, mutable: true }) -> Externref`.
        const ret = makeMutClosure(arg0, arg1, wasm.wasm_bindgen__closure__destroy__h418f9dbe6d6229fb, wasm_bindgen__convert__closures_____invoke__h87e2b5b49ec422e1);
        return ret;
    };
    imports.wbg.__wbindgen_cast_6c2a6a64ddfcf4e0 = function(arg0, arg1) {
        // Cast intrinsic for `Closure(Closure { dtor_idx: 796, function: Function { arguments: [], shim_idx: 797, ret: Unit, inner_ret: Some(Unit) }, mutable: true }) -> Externref`.
        const ret = makeMutClosure(arg0, arg1, wasm.wasm_bindgen__closure__destroy__h4e7c9205dcc62f87, wasm_bindgen__convert__closures_____invoke__hb4b123985bbb9e04);
        return ret;
    };
    imports.wbg.__wbindgen_cast_77bc3e92745e9a35 = function(arg0, arg1) {
        var v0 = getArrayU8FromWasm0(arg0, arg1).slice();
        wasm.__wbindgen_free(arg0, arg1 * 1, 1);
        // Cast intrinsic for `Vector(U8) -> Externref`.
        const ret = v0;
        return ret;
    };
    imports.wbg.__wbindgen_cast_9a4fc78cce893b0c = function(arg0, arg1) {
        // Cast intrinsic for `Closure(Closure { dtor_idx: 188, function: Function { arguments: [NamedExternref("IDBVersionChangeEvent")], shim_idx: 189, ret: Result(Unit), inner_ret: Some(Result(Unit)) }, mutable: true }) -> Externref`.
        const ret = makeMutClosure(arg0, arg1, wasm.wasm_bindgen__closure__destroy__hb47eeaacebe06f89, wasm_bindgen__convert__closures_____invoke__h43c36ca2f90d11b7);
        return ret;
    };
    imports.wbg.__wbindgen_cast_9ae0607507abb057 = function(arg0) {
        // Cast intrinsic for `I64 -> Externref`.
        const ret = arg0;
        return ret;
    };
    imports.wbg.__wbindgen_cast_9d3948cb8a2e20a8 = function(arg0, arg1) {
        // Cast intrinsic for `Closure(Closure { dtor_idx: 1335, function: Function { arguments: [], shim_idx: 1336, ret: Unit, inner_ret: Some(Unit) }, mutable: true }) -> Externref`.
        const ret = makeMutClosure(arg0, arg1, wasm.wasm_bindgen__closure__destroy__h528d1af9a8dea9fd, wasm_bindgen__convert__closures_____invoke__he1f0b0ce9a4c8d18);
        return ret;
    };
    imports.wbg.__wbindgen_cast_cb9088102bce6b30 = function(arg0, arg1) {
        // Cast intrinsic for `Ref(Slice(U8)) -> NamedExternref("Uint8Array")`.
        const ret = getArrayU8FromWasm0(arg0, arg1);
        return ret;
    };
    imports.wbg.__wbindgen_cast_d6cd19b81560fd6e = function(arg0) {
        // Cast intrinsic for `F64 -> Externref`.
        const ret = arg0;
        return ret;
    };
    imports.wbg.__wbindgen_cast_d888960ee798bfa7 = function(arg0, arg1) {
        // Cast intrinsic for `Closure(Closure { dtor_idx: 1017, function: Function { arguments: [NamedExternref("RTCDataChannelEvent")], shim_idx: 1018, ret: Unit, inner_ret: Some(Unit) }, mutable: true }) -> Externref`.
        const ret = makeMutClosure(arg0, arg1, wasm.wasm_bindgen__closure__destroy__h5ad2cc04f4e806d8, wasm_bindgen__convert__closures_____invoke__h02a1a5a325cc1eca);
        return ret;
    };
    imports.wbg.__wbindgen_cast_e3d37513484a0ed5 = function(arg0, arg1) {
        // Cast intrinsic for `Closure(Closure { dtor_idx: 1017, function: Function { arguments: [NamedExternref("Event")], shim_idx: 1018, ret: Unit, inner_ret: Some(Unit) }, mutable: true }) -> Externref`.
        const ret = makeMutClosure(arg0, arg1, wasm.wasm_bindgen__closure__destroy__h5ad2cc04f4e806d8, wasm_bindgen__convert__closures_____invoke__h02a1a5a325cc1eca);
        return ret;
    };
    imports.wbg.__wbindgen_init_externref_table = function() {
        const table = wasm.__wbindgen_externrefs;
        const offset = table.grow(4);
        table.set(0, undefined);
        table.set(offset + 0, undefined);
        table.set(offset + 1, null);
        table.set(offset + 2, true);
        table.set(offset + 3, false);
        ;
    };

    return imports;
}

function __wbg_finalize_init(instance, module) {
    wasm = instance.exports;
    __wbg_init.__wbindgen_wasm_module = module;
    cachedDataViewMemory0 = null;
    cachedUint8ArrayMemory0 = null;


    wasm.__wbindgen_start();
    return wasm;
}

function initSync(module) {
    if (wasm !== undefined) return wasm;


    if (typeof module !== 'undefined') {
        if (Object.getPrototypeOf(module) === Object.prototype) {
            ({module} = module)
        } else {
            console.warn('using deprecated parameters for `initSync()`; pass a single object instead')
        }
    }

    const imports = __wbg_get_imports();

    if (!(module instanceof WebAssembly.Module)) {
        module = new WebAssembly.Module(module);
    }

    const instance = new WebAssembly.Instance(module, imports);

    return __wbg_finalize_init(instance, module);
}

async function __wbg_init(module_or_path) {
    if (wasm !== undefined) return wasm;


    if (typeof module_or_path !== 'undefined') {
        if (Object.getPrototypeOf(module_or_path) === Object.prototype) {
            ({module_or_path} = module_or_path)
        } else {
            console.warn('using deprecated parameters for the initialization function; pass a single object instead')
        }
    }

    if (typeof module_or_path === 'undefined') {
        module_or_path = new URL('weeb_3_bg.wasm', import.meta.url);
    }
    const imports = __wbg_get_imports();

    if (typeof module_or_path === 'string' || (typeof Request === 'function' && module_or_path instanceof Request) || (typeof URL === 'function' && module_or_path instanceof URL)) {
        module_or_path = fetch(module_or_path);
    }

    const { instance, module } = await __wbg_load(await module_or_path, imports);

    return __wbg_finalize_init(instance, module);
}

export { initSync };
export default __wbg_init;
