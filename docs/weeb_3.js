/* @ts-self-types="./weeb_3.d.ts" */
import { get_provider_js } from './snippets/web3-0742d85b024bb6f5/inline0.js';

export class BootstrapNode {
    static __unwrap(jsValue) {
        if (!(jsValue instanceof BootstrapNode)) {
            return 0;
        }
        return jsValue.__destroy_into_raw();
    }
    __destroy_into_raw() {
        const ptr = this.__wbg_ptr;
        this.__wbg_ptr = 0;
        BootstrapNodeFinalization.unregister(this);
        return ptr;
    }
    free() {
        const ptr = this.__destroy_into_raw();
        wasm.__wbg_bootstrapnode_free(ptr, 0);
    }
    /**
     * @returns {string}
     */
    get multiaddr() {
        let deferred1_0;
        let deferred1_1;
        try {
            const ret = wasm.bootstrapnode_multiaddr(this.__wbg_ptr);
            deferred1_0 = ret[0];
            deferred1_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
        }
    }
    /**
     * @param {string} multiaddr
     * @param {boolean} usable
     */
    constructor(multiaddr, usable) {
        const ptr0 = passStringToWasm0(multiaddr, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.bootstrapnode_new(ptr0, len0, usable);
        this.__wbg_ptr = ret >>> 0;
        BootstrapNodeFinalization.register(this, this.__wbg_ptr, this);
        return this;
    }
    /**
     * @returns {boolean}
     */
    get usable() {
        const ret = wasm.bootstrapnode_usable(this.__wbg_ptr);
        return ret !== 0;
    }
}
if (Symbol.dispose) BootstrapNode.prototype[Symbol.dispose] = BootstrapNode.prototype.free;

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
     * @param {string} address
     * @returns {Promise<Uint8Array>}
     */
    acquire(address) {
        const ptr0 = passStringToWasm0(address, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.sekirei_acquire(this.__wbg_ptr, ptr0, len0);
        return ret;
    }
    /**
     * @param {string} address
     * @param {string} _id
     * @param {boolean} usable_in_protocols
     * @returns {Promise<Uint8Array>}
     */
    change_bootnode_address(address, _id, usable_in_protocols) {
        const ptr0 = passStringToWasm0(address, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passStringToWasm0(_id, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len1 = WASM_VECTOR_LEN;
        const ret = wasm.sekirei_change_bootnode_address(this.__wbg_ptr, ptr0, len0, ptr1, len1, usable_in_protocols);
        return ret;
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
     * @returns {Promise<bigint>}
     */
    get_ongoing_connections() {
        const ret = wasm.sekirei_get_ongoing_connections(this.__wbg_ptr);
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
     * @param {Uint8Array} d
     * @param {boolean} soc
     * @param {Uint8Array} chunk_address
     * @param {Uint8Array} stamp
     * @returns {Promise<Uint8Array>}
     */
    post_push_chunk(d, soc, chunk_address, stamp) {
        const ptr0 = passArray8ToWasm0(d, wasm.__wbindgen_malloc);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passArray8ToWasm0(chunk_address, wasm.__wbindgen_malloc);
        const len1 = WASM_VECTOR_LEN;
        const ptr2 = passArray8ToWasm0(stamp, wasm.__wbindgen_malloc);
        const len2 = WASM_VECTOR_LEN;
        const ret = wasm.sekirei_post_push_chunk(this.__wbg_ptr, ptr0, len0, soc, ptr1, len1, ptr2, len2);
        return ret;
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
     * @param {string} _st
     * @returns {Promise<void>}
     */
    run(_st) {
        const ptr0 = passStringToWasm0(_st, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.sekirei_run(this.__wbg_ptr, ptr0, len0);
        return ret;
    }
}
if (Symbol.dispose) Sekirei.prototype[Symbol.dispose] = Sekirei.prototype.free;

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
    constructor() {
        const ret = wasm.sekireino103_new();
        this.__wbg_ptr = ret >>> 0;
        SekireiNo103Finalization.register(this, this.__wbg_ptr, this);
        return this;
    }
    /**
     * @param {Uint8Array} data
     * @param {boolean} soc
     * @param {Uint8Array} chunk_address
     * @param {Uint8Array} stamp
     * @returns {Promise<string>}
     */
    postPushChunk(data, soc, chunk_address, stamp) {
        const ptr0 = passArray8ToWasm0(data, wasm.__wbindgen_malloc);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passArray8ToWasm0(chunk_address, wasm.__wbindgen_malloc);
        const len1 = WASM_VECTOR_LEN;
        const ptr2 = passArray8ToWasm0(stamp, wasm.__wbindgen_malloc);
        const len2 = WASM_VECTOR_LEN;
        const ret = wasm.sekireino103_postPushChunk(this.__wbg_ptr, ptr0, len0, soc, ptr1, len1, ptr2, len2);
        return ret;
    }
    /**
     * @returns {Promise<object>}
     */
    resetStamp() {
        const ret = wasm.sekireino103_resetStamp(this.__wbg_ptr);
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
    /**
     * @param {BootstrapNode[]} bootstrap_nodes
     * @param {string} network_id
     */
    start(bootstrap_nodes, network_id) {
        const ptr0 = passArrayJsValueToWasm0(bootstrap_nodes, wasm.__wbindgen_malloc);
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
}
if (Symbol.dispose) SekireiNo103.prototype[Symbol.dispose] = SekireiNo103.prototype.free;

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

function __wbg_get_imports() {
    const import0 = {
        __proto__: null,
        __wbg_Error_83742b46f01ce22d: function(arg0, arg1) {
            const ret = Error(getStringFromWasm0(arg0, arg1));
            return ret;
        },
        __wbg_Number_a5a435bd7bbec835: function(arg0) {
            const ret = Number(arg0);
            return ret;
        },
        __wbg_String_8564e559799eccda: function(arg0, arg1) {
            const ret = String(arg1);
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_Window_70131fc0c91e4b3c: function(arg0) {
            const ret = arg0.Window;
            return ret;
        },
        __wbg_WorkerGlobalScope_0f395e12ab05d182: function(arg0) {
            const ret = arg0.WorkerGlobalScope;
            return ret;
        },
        __wbg_WorkerGlobalScope_601c48015b8cc78e: function(arg0) {
            const ret = arg0.WorkerGlobalScope;
            return ret;
        },
        __wbg___wbindgen_bigint_get_as_i64_447a76b5c6ef7bda: function(arg0, arg1) {
            const v = arg1;
            const ret = typeof(v) === 'bigint' ? v : undefined;
            getDataViewMemory0().setBigInt64(arg0 + 8 * 1, isLikeNone(ret) ? BigInt(0) : ret, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, !isLikeNone(ret), true);
        },
        __wbg___wbindgen_boolean_get_c0f3f60bac5a78d1: function(arg0) {
            const v = arg0;
            const ret = typeof(v) === 'boolean' ? v : undefined;
            return isLikeNone(ret) ? 0xFFFFFF : ret ? 1 : 0;
        },
        __wbg___wbindgen_debug_string_5398f5bb970e0daa: function(arg0, arg1) {
            const ret = debugString(arg1);
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg___wbindgen_in_41dbb8413020e076: function(arg0, arg1) {
            const ret = arg0 in arg1;
            return ret;
        },
        __wbg___wbindgen_is_bigint_e2141d4f045b7eda: function(arg0) {
            const ret = typeof(arg0) === 'bigint';
            return ret;
        },
        __wbg___wbindgen_is_function_3c846841762788c1: function(arg0) {
            const ret = typeof(arg0) === 'function';
            return ret;
        },
        __wbg___wbindgen_is_null_0b605fc6b167c56f: function(arg0) {
            const ret = arg0 === null;
            return ret;
        },
        __wbg___wbindgen_is_object_781bc9f159099513: function(arg0) {
            const val = arg0;
            const ret = typeof(val) === 'object' && val !== null;
            return ret;
        },
        __wbg___wbindgen_is_string_7ef6b97b02428fae: function(arg0) {
            const ret = typeof(arg0) === 'string';
            return ret;
        },
        __wbg___wbindgen_is_undefined_52709e72fb9f179c: function(arg0) {
            const ret = arg0 === undefined;
            return ret;
        },
        __wbg___wbindgen_jsval_eq_ee31bfad3e536463: function(arg0, arg1) {
            const ret = arg0 === arg1;
            return ret;
        },
        __wbg___wbindgen_jsval_loose_eq_5bcc3bed3c69e72b: function(arg0, arg1) {
            const ret = arg0 == arg1;
            return ret;
        },
        __wbg___wbindgen_number_get_34bb9d9dcfa21373: function(arg0, arg1) {
            const obj = arg1;
            const ret = typeof(obj) === 'number' ? obj : undefined;
            getDataViewMemory0().setFloat64(arg0 + 8 * 1, isLikeNone(ret) ? 0 : ret, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, !isLikeNone(ret), true);
        },
        __wbg___wbindgen_string_get_395e606bd0ee4427: function(arg0, arg1) {
            const obj = arg1;
            const ret = typeof(obj) === 'string' ? obj : undefined;
            var ptr1 = isLikeNone(ret) ? 0 : passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            var len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg___wbindgen_throw_6ddd609b62940d55: function(arg0, arg1) {
            throw new Error(getStringFromWasm0(arg0, arg1));
        },
        __wbg__wbg_cb_unref_6b5b6b8576d35cb1: function(arg0) {
            arg0._wbg_cb_unref();
        },
        __wbg_abort_5ef96933660780b7: function(arg0) {
            arg0.abort();
        },
        __wbg_abort_60dcb252ae0031fc: function() { return handleError(function (arg0) {
            arg0.abort();
        }, arguments); },
        __wbg_active_08a5c0d88f83a934: function(arg0) {
            const ret = arg0.active;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_addEventListener_2d985aa8a656f6dc: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            arg0.addEventListener(getStringFromWasm0(arg1, arg2), arg3);
        }, arguments); },
        __wbg_alert_fafdd3ea48e916e5: function() { return handleError(function (arg0, arg1, arg2) {
            arg0.alert(getStringFromWasm0(arg1, arg2));
        }, arguments); },
        __wbg_append_608dfb635ee8998f: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4) {
            arg0.append(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4));
        }, arguments); },
        __wbg_arrayBuffer_7ff5e58aa85a64f7: function(arg0) {
            const ret = arg0.arrayBuffer();
            return ret;
        },
        __wbg_arrayBuffer_eb8e9ca620af2a19: function() { return handleError(function (arg0) {
            const ret = arg0.arrayBuffer();
            return ret;
        }, arguments); },
        __wbg_bootstrapnode_unwrap: function(arg0) {
            const ret = BootstrapNode.__unwrap(arg0);
            return ret;
        },
        __wbg_bound_4e343b4fbe5419fa: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            const ret = IDBKeyRange.bound(arg0, arg1, arg2 !== 0, arg3 !== 0);
            return ret;
        }, arguments); },
        __wbg_bufferedAmount_1a73099b19f45ec6: function(arg0) {
            const ret = arg0.bufferedAmount;
            return ret;
        },
        __wbg_call_2d781c1f4d5c0ef8: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.call(arg1, arg2);
            return ret;
        }, arguments); },
        __wbg_call_e133b57c9155d22c: function() { return handleError(function (arg0, arg1) {
            const ret = arg0.call(arg1);
            return ret;
        }, arguments); },
        __wbg_checked_7b8b07c4341e3e6c: function(arg0) {
            const ret = arg0.checked;
            return ret;
        },
        __wbg_clearInterval_15af97604b78d7cf: function(arg0, arg1) {
            arg0.clearInterval(arg1);
        },
        __wbg_clearInterval_1cf7b4d7d9952d6e: function(arg0, arg1) {
            arg0.clearInterval(arg1);
        },
        __wbg_clearTimeout_01406e55473040f6: function(arg0) {
            const ret = clearTimeout(arg0);
            return ret;
        },
        __wbg_clearTimeout_113b1cde814ec762: function(arg0) {
            const ret = clearTimeout(arg0);
            return ret;
        },
        __wbg_close_a86fff250f8aa14f: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            arg0.close(arg1, getStringFromWasm0(arg2, arg3));
        }, arguments); },
        __wbg_close_cbf870bdad0aad99: function(arg0) {
            arg0.close();
        },
        __wbg_commit_e9c1332714c53826: function() { return handleError(function (arg0) {
            arg0.commit();
        }, arguments); },
        __wbg_confirm_21e2365fd195da87: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.confirm(getStringFromWasm0(arg1, arg2));
            return ret;
        }, arguments); },
        __wbg_createElement_9b0aab265c549ded: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.createElement(getStringFromWasm0(arg1, arg2));
            return ret;
        }, arguments); },
        __wbg_createObjectStore_4709de9339ffc6c0: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            const ret = arg0.createObjectStore(getStringFromWasm0(arg1, arg2), arg3);
            return ret;
        }, arguments); },
        __wbg_createObjectURL_f141426bcc1f70aa: function() { return handleError(function (arg0, arg1) {
            const ret = URL.createObjectURL(arg1);
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        }, arguments); },
        __wbg_crypto_38df2bab126b63dc: function(arg0) {
            const ret = arg0.crypto;
            return ret;
        },
        __wbg_data_a3d9ff9cdd801002: function(arg0) {
            const ret = arg0.data;
            return ret;
        },
        __wbg_deleteDatabase_4cdf2c29aa0ce0ca: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.deleteDatabase(getStringFromWasm0(arg1, arg2));
            return ret;
        }, arguments); },
        __wbg_document_c0320cd4183c6d9b: function(arg0) {
            const ret = arg0.document;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_done_08ce71ee07e3bd17: function(arg0) {
            const ret = arg0.done;
            return ret;
        },
        __wbg_entries_e8a20ff8c9757101: function(arg0) {
            const ret = Object.entries(arg0);
            return ret;
        },
        __wbg_error_74898554122344a8: function() { return handleError(function (arg0) {
            const ret = arg0.error;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        }, arguments); },
        __wbg_error_8d9a8e04cd1d3588: function(arg0) {
            console.error(arg0);
        },
        __wbg_fetch_5550a88cf343aaa9: function(arg0, arg1) {
            const ret = arg0.fetch(arg1);
            return ret;
        },
        __wbg_fetch_fda7bc27c982b1f3: function(arg0) {
            const ret = fetch(arg0);
            return ret;
        },
        __wbg_files_920845eefcb0aa23: function(arg0) {
            const ret = arg0.files;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_from_4bdf88943703fd48: function(arg0) {
            const ret = Array.from(arg0);
            return ret;
        },
        __wbg_getElementById_d1f25d287b19a833: function(arg0, arg1, arg2) {
            const ret = arg0.getElementById(getStringFromWasm0(arg1, arg2));
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_getRandomValues_3f44b700395062e5: function() { return handleError(function (arg0, arg1) {
            globalThis.crypto.getRandomValues(getArrayU8FromWasm0(arg0, arg1));
        }, arguments); },
        __wbg_getRandomValues_c44a50d8cfdaebeb: function() { return handleError(function (arg0, arg1) {
            arg0.getRandomValues(arg1);
        }, arguments); },
        __wbg_getRegistration_8eca97e38d206614: function(arg0) {
            const ret = arg0.getRegistration();
            return ret;
        },
        __wbg_get_326e41e095fb2575: function() { return handleError(function (arg0, arg1) {
            const ret = Reflect.get(arg0, arg1);
            return ret;
        }, arguments); },
        __wbg_get_3ef1eba1850ade27: function() { return handleError(function (arg0, arg1) {
            const ret = Reflect.get(arg0, arg1);
            return ret;
        }, arguments); },
        __wbg_get_6ac8c8119f577720: function() { return handleError(function (arg0, arg1) {
            const ret = arg0.get(arg1);
            return ret;
        }, arguments); },
        __wbg_get_a8ee5c45dabc1b3b: function(arg0, arg1) {
            const ret = arg0[arg1 >>> 0];
            return ret;
        },
        __wbg_get_provider_js_92bb8eb887ed425c: function() { return handleError(function () {
            const ret = get_provider_js();
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        }, arguments); },
        __wbg_get_unchecked_329cfe50afab7352: function(arg0, arg1) {
            const ret = arg0[arg1 >>> 0];
            return ret;
        },
        __wbg_get_with_ref_key_6412cf3094599694: function(arg0, arg1) {
            const ret = arg0[arg1];
            return ret;
        },
        __wbg_global_e30ac0b7684506d0: function(arg0) {
            const ret = arg0.global;
            return ret;
        },
        __wbg_has_926ef2ff40b308cf: function() { return handleError(function (arg0, arg1) {
            const ret = Reflect.has(arg0, arg1);
            return ret;
        }, arguments); },
        __wbg_headers_eb2234545f9ff993: function(arg0) {
            const ret = arg0.headers;
            return ret;
        },
        __wbg_history_26b8c29b7753d0e8: function() { return handleError(function (arg0) {
            const ret = arg0.history;
            return ret;
        }, arguments); },
        __wbg_href_82f7f7056b4e8510: function() { return handleError(function (arg0, arg1) {
            const ret = arg1.href;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        }, arguments); },
        __wbg_indexedDB_2ae2128d487c6ebc: function() { return handleError(function (arg0) {
            const ret = arg0.indexedDB;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        }, arguments); },
        __wbg_indexedDB_a2139150e2ea2a08: function() { return handleError(function (arg0) {
            const ret = arg0.indexedDB;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        }, arguments); },
        __wbg_indexedDB_c83feb7151bbde52: function() { return handleError(function (arg0) {
            const ret = arg0.indexedDB;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        }, arguments); },
        __wbg_instanceof_ArrayBuffer_101e2bf31071a9f6: function(arg0) {
            let result;
            try {
                result = arg0 instanceof ArrayBuffer;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_DomException_2bdcf7791a2d7d09: function(arg0) {
            let result;
            try {
                result = arg0 instanceof DOMException;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Error_4691a5b466e32a80: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Error;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_File_a301c444111d30cb: function(arg0) {
            let result;
            try {
                result = arg0 instanceof File;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_HtmlButtonElement_36cd452d9a7140c4: function(arg0) {
            let result;
            try {
                result = arg0 instanceof HTMLButtonElement;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_HtmlElement_ef05df8222c2b81b: function(arg0) {
            let result;
            try {
                result = arg0 instanceof HTMLElement;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_HtmlInputElement_f6b9c8ea98b1980f: function(arg0) {
            let result;
            try {
                result = arg0 instanceof HTMLInputElement;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_HtmlSelectElement_e92712a401f2c8ce: function(arg0) {
            let result;
            try {
                result = arg0 instanceof HTMLSelectElement;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_HtmlSpanElement_0abf367fb5dffd9f: function(arg0) {
            let result;
            try {
                result = arg0 instanceof HTMLSpanElement;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_IdbDatabase_5f436cc89cc07f14: function(arg0) {
            let result;
            try {
                result = arg0 instanceof IDBDatabase;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_IdbRequest_6a0e24572d4f1d46: function(arg0) {
            let result;
            try {
                result = arg0 instanceof IDBRequest;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Map_f194b366846aca0c: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Map;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_MessagePort_0103f83d2ba09ecb: function(arg0) {
            let result;
            try {
                result = arg0 instanceof MessagePort;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Object_be1962063fcc0c9f: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Object;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Promise_7c3bdd7805c2c6e6: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Promise;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Response_9b4d9fd451e051b1: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Response;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_ServiceWorkerRegistration_9ae9c5811e83cd7e: function(arg0) {
            let result;
            try {
                result = arg0 instanceof ServiceWorkerRegistration;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Uint8Array_740438561a5b956d: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Uint8Array;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Window_23e677d2c6843922: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Window;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_isArray_33b91feb269ff46e: function(arg0) {
            const ret = Array.isArray(arg0);
            return ret;
        },
        __wbg_isSafeInteger_ecd6a7f9c3e053cd: function(arg0) {
            const ret = Number.isSafeInteger(arg0);
            return ret;
        },
        __wbg_item_f5272ae87ff19e35: function(arg0, arg1) {
            const ret = arg0.item(arg1 >>> 0);
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_iterator_d8f549ec8fb061b1: function() {
            const ret = Symbol.iterator;
            return ret;
        },
        __wbg_length_b3416cf66a5452c8: function(arg0) {
            const ret = arg0.length;
            return ret;
        },
        __wbg_length_ea16607d7b61445b: function(arg0) {
            const ret = arg0.length;
            return ret;
        },
        __wbg_location_bb196e19ef6c7fa6: function(arg0) {
            const ret = arg0.location;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_location_fc8d47802682dd93: function(arg0) {
            const ret = arg0.location;
            return ret;
        },
        __wbg_log_524eedafa26daa59: function(arg0) {
            console.log(arg0);
        },
        __wbg_lowerBound_7dd256f30bc73b4e: function() { return handleError(function (arg0, arg1) {
            const ret = IDBKeyRange.lowerBound(arg0, arg1 !== 0);
            return ret;
        }, arguments); },
        __wbg_message_00d63f20c41713dd: function(arg0) {
            const ret = arg0.message;
            return ret;
        },
        __wbg_message_e959edc81e4b6cb7: function(arg0, arg1) {
            const ret = arg1.message;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_msCrypto_bd5a034af96bcba6: function(arg0) {
            const ret = arg0.msCrypto;
            return ret;
        },
        __wbg_name_3393c0574942cc57: function(arg0, arg1) {
            const ret = arg1.name;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_name_619aa58297c2f80e: function(arg0, arg1) {
            const ret = arg1.name;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_name_7a3bbd030d0afa16: function(arg0, arg1) {
            const ret = arg1.name;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_navigator_9cebf56f28aa719b: function(arg0) {
            const ret = arg0.navigator;
            return ret;
        },
        __wbg_newVersion_a398d586ede180a7: function(arg0, arg1) {
            const ret = arg1.newVersion;
            getDataViewMemory0().setFloat64(arg0 + 8 * 1, isLikeNone(ret) ? 0 : ret, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, !isLikeNone(ret), true);
        },
        __wbg_new_0837727332ac86ba: function() { return handleError(function () {
            const ret = new Headers();
            return ret;
        }, arguments); },
        __wbg_new_49d5571bd3f0c4d4: function() {
            const ret = new Map();
            return ret;
        },
        __wbg_new_5f486cdf45a04d78: function(arg0) {
            const ret = new Uint8Array(arg0);
            return ret;
        },
        __wbg_new_a70fbab9066b301f: function() {
            const ret = new Array();
            return ret;
        },
        __wbg_new_ab79df5bd7c26067: function() {
            const ret = new Object();
            return ret;
        },
        __wbg_new_c518c60af666645b: function() { return handleError(function () {
            const ret = new AbortController();
            return ret;
        }, arguments); },
        __wbg_new_d098e265629cd10f: function(arg0, arg1) {
            try {
                var state0 = {a: arg0, b: arg1};
                var cb0 = (arg0, arg1) => {
                    const a = state0.a;
                    state0.a = 0;
                    try {
                        return wasm_bindgen__convert__closures_____invoke__h025f346370eef057(a, state0.b, arg0, arg1);
                    } finally {
                        state0.a = a;
                    }
                };
                const ret = new Promise(cb0);
                return ret;
            } finally {
                state0.a = state0.b = 0;
            }
        },
        __wbg_new_d15cb560a6a0e5f0: function(arg0, arg1) {
            const ret = new Error(getStringFromWasm0(arg0, arg1));
            return ret;
        },
        __wbg_new_dd50bcc3f60ba434: function() { return handleError(function (arg0, arg1) {
            const ret = new WebSocket(getStringFromWasm0(arg0, arg1));
            return ret;
        }, arguments); },
        __wbg_new_f7708ba82c4c12f6: function() { return handleError(function () {
            const ret = new MessageChannel();
            return ret;
        }, arguments); },
        __wbg_new_from_slice_22da9388ac046e50: function(arg0, arg1) {
            const ret = new Uint8Array(getArrayU8FromWasm0(arg0, arg1));
            return ret;
        },
        __wbg_new_typed_aaaeaf29cf802876: function(arg0, arg1) {
            try {
                var state0 = {a: arg0, b: arg1};
                var cb0 = (arg0, arg1) => {
                    const a = state0.a;
                    state0.a = 0;
                    try {
                        return wasm_bindgen__convert__closures_____invoke__h025f346370eef057(a, state0.b, arg0, arg1);
                    } finally {
                        state0.a = a;
                    }
                };
                const ret = new Promise(cb0);
                return ret;
            } finally {
                state0.a = state0.b = 0;
            }
        },
        __wbg_new_typed_bccac67128ed885a: function() {
            const ret = new Array();
            return ret;
        },
        __wbg_new_with_length_825018a1616e9e55: function(arg0) {
            const ret = new Uint8Array(arg0 >>> 0);
            return ret;
        },
        __wbg_new_with_str_and_init_b4b54d1a819bc724: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = new Request(getStringFromWasm0(arg0, arg1), arg2);
            return ret;
        }, arguments); },
        __wbg_new_with_u8_array_sequence_and_options_b1b2f94d6abf19c6: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            const ret = new File(arg0, getStringFromWasm0(arg1, arg2), arg3);
            return ret;
        }, arguments); },
        __wbg_new_with_u8_array_sequence_and_options_de38f663e19ad899: function() { return handleError(function (arg0, arg1) {
            const ret = new Blob(arg0, arg1);
            return ret;
        }, arguments); },
        __wbg_next_11b99ee6237339e3: function() { return handleError(function (arg0) {
            const ret = arg0.next();
            return ret;
        }, arguments); },
        __wbg_next_e01a967809d1aa68: function(arg0) {
            const ret = arg0.next;
            return ret;
        },
        __wbg_node_84ea875411254db1: function(arg0) {
            const ret = arg0.node;
            return ret;
        },
        __wbg_now_16f0c993d5dd6c27: function() {
            const ret = Date.now();
            return ret;
        },
        __wbg_now_e7c6795a7f81e10f: function(arg0) {
            const ret = arg0.now();
            return ret;
        },
        __wbg_objectStore_f314ab152a5c7bd0: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.objectStore(getStringFromWasm0(arg1, arg2));
            return ret;
        }, arguments); },
        __wbg_of_8bf7ed3eca00ea43: function(arg0) {
            const ret = Array.of(arg0);
            return ret;
        },
        __wbg_oldVersion_c28aefdefa84030a: function(arg0) {
            const ret = arg0.oldVersion;
            return ret;
        },
        __wbg_on_f4145ffe4fe22bf2: function(arg0, arg1, arg2, arg3) {
            arg0.on(getStringFromWasm0(arg1, arg2), arg3);
        },
        __wbg_open_e7a9d3d6344572f6: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            const ret = arg0.open(getStringFromWasm0(arg1, arg2), arg3 >>> 0);
            return ret;
        }, arguments); },
        __wbg_open_f3dc09caa3990bc4: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.open(getStringFromWasm0(arg1, arg2));
            return ret;
        }, arguments); },
        __wbg_origin_bac5c3119fe40a90: function() { return handleError(function (arg0, arg1) {
            const ret = arg1.origin;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        }, arguments); },
        __wbg_pathname_91da77877cfd2b76: function() { return handleError(function (arg0, arg1) {
            const ret = arg1.pathname;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        }, arguments); },
        __wbg_performance_3fcf6e32a7e1ed0a: function(arg0) {
            const ret = arg0.performance;
            return ret;
        },
        __wbg_port1_869a7ef90538dbdf: function(arg0) {
            const ret = arg0.port1;
            return ret;
        },
        __wbg_port2_947a51b8ba00adc9: function(arg0) {
            const ret = arg0.port2;
            return ret;
        },
        __wbg_ports_2d0c862bfd0ee6d3: function(arg0) {
            const ret = arg0.ports;
            return ret;
        },
        __wbg_postMessage_17ffa1d7b4f1338d: function() { return handleError(function (arg0, arg1, arg2) {
            arg0.postMessage(arg1, arg2);
        }, arguments); },
        __wbg_postMessage_c89a8b5edbf59ad0: function() { return handleError(function (arg0, arg1) {
            arg0.postMessage(arg1);
        }, arguments); },
        __wbg_prepend_ebebf3d32ea17a06: function() { return handleError(function (arg0, arg1) {
            arg0.prepend(arg1);
        }, arguments); },
        __wbg_process_44c7a14e11e9f69e: function(arg0) {
            const ret = arg0.process;
            return ret;
        },
        __wbg_prototypesetcall_d62e5099504357e6: function(arg0, arg1, arg2) {
            Uint8Array.prototype.set.call(getArrayU8FromWasm0(arg0, arg1), arg2);
        },
        __wbg_push_e87b0e732085a946: function(arg0, arg1) {
            const ret = arg0.push(arg1);
            return ret;
        },
        __wbg_put_f1673d719f93ce22: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.put(arg1, arg2);
            return ret;
        }, arguments); },
        __wbg_queueMicrotask_0c399741342fb10f: function(arg0) {
            const ret = arg0.queueMicrotask;
            return ret;
        },
        __wbg_queueMicrotask_a082d78ce798393e: function(arg0) {
            queueMicrotask(arg0);
        },
        __wbg_randomFillSync_6c25eac9869eb53c: function() { return handleError(function (arg0, arg1) {
            arg0.randomFillSync(arg1);
        }, arguments); },
        __wbg_readyState_1f1e7f1bdf9f4d42: function(arg0) {
            const ret = arg0.readyState;
            return ret;
        },
        __wbg_readyState_57fa0866477cc0c4: function(arg0) {
            const ret = arg0.readyState;
            return (__wbindgen_enum_IdbRequestReadyState.indexOf(ret) + 1 || 3) - 1;
        },
        __wbg_ready_21cb3b1575756f01: function() { return handleError(function (arg0) {
            const ret = arg0.ready;
            return ret;
        }, arguments); },
        __wbg_register_ceb9254161d48007: function(arg0, arg1, arg2) {
            const ret = arg0.register(getStringFromWasm0(arg1, arg2));
            return ret;
        },
        __wbg_removeListener_250a33965641c61b: function(arg0, arg1, arg2, arg3) {
            arg0.removeListener(getStringFromWasm0(arg1, arg2), arg3);
        },
        __wbg_replaceState_2dd9c86c164b292e: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5) {
            arg0.replaceState(arg1, getStringFromWasm0(arg2, arg3), arg4 === 0 ? undefined : getStringFromWasm0(arg4, arg5));
        }, arguments); },
        __wbg_request_23303be4687a2540: function() { return handleError(function (arg0, arg1) {
            const ret = arg0.request(RequestArguments.__wrap(arg1));
            return ret;
        }, arguments); },
        __wbg_require_b4edbdcf3e2a1ef0: function() { return handleError(function () {
            const ret = module.require;
            return ret;
        }, arguments); },
        __wbg_resolve_ae8d83246e5bcc12: function(arg0) {
            const ret = Promise.resolve(arg0);
            return ret;
        },
        __wbg_result_c5baa2d3d690a01a: function() { return handleError(function (arg0) {
            const ret = arg0.result;
            return ret;
        }, arguments); },
        __wbg_send_d31a693c975dea74: function() { return handleError(function (arg0, arg1, arg2) {
            arg0.send(getArrayU8FromWasm0(arg1, arg2));
        }, arguments); },
        __wbg_serviceWorker_9d2f0ce49eb04a32: function(arg0) {
            const ret = arg0.serviceWorker;
            return ret;
        },
        __wbg_setAttribute_f20d3b966749ab64: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4) {
            arg0.setAttribute(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4));
        }, arguments); },
        __wbg_setInterval_095d3d874943f47a: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            const ret = arg0.setInterval(arg1, arg2, ...arg3);
            return ret;
        }, arguments); },
        __wbg_setInterval_68f9d1294acca85b: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            const ret = arg0.setInterval(arg1, arg2, ...arg3);
            return ret;
        }, arguments); },
        __wbg_setTimeout_613a21b62dc655a1: function() { return handleError(function (arg0, arg1) {
            const ret = setTimeout(arg0, arg1);
            return ret;
        }, arguments); },
        __wbg_setTimeout_ef24d2fc3ad97385: function() { return handleError(function (arg0, arg1) {
            const ret = setTimeout(arg0, arg1);
            return ret;
        }, arguments); },
        __wbg_set_282384002438957f: function(arg0, arg1, arg2) {
            arg0[arg1 >>> 0] = arg2;
        },
        __wbg_set_6be42768c690e380: function(arg0, arg1, arg2) {
            arg0[arg1] = arg2;
        },
        __wbg_set_7eaa4f96924fd6b3: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = Reflect.set(arg0, arg1, arg2);
            return ret;
        }, arguments); },
        __wbg_set_8c0b3ffcf05d61c2: function(arg0, arg1, arg2) {
            arg0.set(getArrayU8FromWasm0(arg1, arg2));
        },
        __wbg_set_auto_increment_ffc3cd6470763a4c: function(arg0, arg1) {
            arg0.autoIncrement = arg1 !== 0;
        },
        __wbg_set_bf7251625df30a02: function(arg0, arg1, arg2) {
            const ret = arg0.set(arg1, arg2);
            return ret;
        },
        __wbg_set_binaryType_3dcf8281ec100a8f: function(arg0, arg1) {
            arg0.binaryType = __wbindgen_enum_BinaryType[arg1];
        },
        __wbg_set_body_a3d856b097dfda04: function(arg0, arg1) {
            arg0.body = arg1;
        },
        __wbg_set_credentials_ed63183445882c65: function(arg0, arg1) {
            arg0.credentials = __wbindgen_enum_RequestCredentials[arg1];
        },
        __wbg_set_headers_3c8fecc693b75327: function(arg0, arg1) {
            arg0.headers = arg1;
        },
        __wbg_set_innerHTML_97039584c4ab4c83: function(arg0, arg1, arg2) {
            arg0.innerHTML = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_last_modified_34f064e31abd4ab5: function(arg0, arg1) {
            arg0.lastModified = arg1;
        },
        __wbg_set_method_8c015e8bcafd7be1: function(arg0, arg1, arg2) {
            arg0.method = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_mode_5a87f2c809cf37c2: function(arg0, arg1) {
            arg0.mode = __wbindgen_enum_RequestMode[arg1];
        },
        __wbg_set_onabort_63885d8d7841a8d5: function(arg0, arg1) {
            arg0.onabort = arg1;
        },
        __wbg_set_onclick_99ae877274f3f4db: function(arg0, arg1) {
            arg0.onclick = arg1;
        },
        __wbg_set_onclose_8da801226bdd7a7b: function(arg0, arg1) {
            arg0.onclose = arg1;
        },
        __wbg_set_oncomplete_f31e6dc6d16c1ff8: function(arg0, arg1) {
            arg0.oncomplete = arg1;
        },
        __wbg_set_onerror_8a268cb237177bba: function(arg0, arg1) {
            arg0.onerror = arg1;
        },
        __wbg_set_onerror_901ca711f94a5bbb: function(arg0, arg1) {
            arg0.onerror = arg1;
        },
        __wbg_set_onerror_c1ecd6233c533c08: function(arg0, arg1) {
            arg0.onerror = arg1;
        },
        __wbg_set_oninput_edd4f56777aef6e0: function(arg0, arg1) {
            arg0.oninput = arg1;
        },
        __wbg_set_onmessage_6f80ab771bf151aa: function(arg0, arg1) {
            arg0.onmessage = arg1;
        },
        __wbg_set_onmessage_f939f8b6d08ca76b: function(arg0, arg1) {
            arg0.onmessage = arg1;
        },
        __wbg_set_onopen_34e3e24cf9337ddd: function(arg0, arg1) {
            arg0.onopen = arg1;
        },
        __wbg_set_onsuccess_fca94ded107b64af: function(arg0, arg1) {
            arg0.onsuccess = arg1;
        },
        __wbg_set_onupgradeneeded_860ce42184f987e7: function(arg0, arg1) {
            arg0.onupgradeneeded = arg1;
        },
        __wbg_set_signal_0cebecb698f25d21: function(arg0, arg1) {
            arg0.signal = arg1;
        },
        __wbg_set_type_33e79f1b45a78c37: function(arg0, arg1, arg2) {
            arg0.type = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_type_bb8080a0307b2ef7: function(arg0, arg1, arg2) {
            arg0.type = getStringFromWasm0(arg1, arg2);
        },
        __wbg_signal_166e1da31adcac18: function(arg0) {
            const ret = arg0.signal;
            return ret;
        },
        __wbg_size_819df95195daae81: function(arg0) {
            const ret = arg0.size;
            return ret;
        },
        __wbg_slice_f55d290dcc799204: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.slice(arg1, arg2);
            return ret;
        }, arguments); },
        __wbg_static_accessor_GLOBAL_8adb955bd33fac2f: function() {
            const ret = typeof global === 'undefined' ? null : global;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_static_accessor_GLOBAL_THIS_ad356e0db91c7913: function() {
            const ret = typeof globalThis === 'undefined' ? null : globalThis;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_static_accessor_SELF_f207c857566db248: function() {
            const ret = typeof self === 'undefined' ? null : self;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_static_accessor_WINDOW_bb9f1ba69d61b386: function() {
            const ret = typeof window === 'undefined' ? null : window;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_status_318629ab93a22955: function(arg0) {
            const ret = arg0.status;
            return ret;
        },
        __wbg_stringify_5ae93966a84901ac: function() { return handleError(function (arg0) {
            const ret = JSON.stringify(arg0);
            return ret;
        }, arguments); },
        __wbg_subarray_a068d24e39478a8a: function(arg0, arg1, arg2) {
            const ret = arg0.subarray(arg1 >>> 0, arg2 >>> 0);
            return ret;
        },
        __wbg_target_7bc90f314634b37b: function(arg0) {
            const ret = arg0.target;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_then_098abe61755d12f6: function(arg0, arg1) {
            const ret = arg0.then(arg1);
            return ret;
        },
        __wbg_then_9e335f6dd892bc11: function(arg0, arg1, arg2) {
            const ret = arg0.then(arg1, arg2);
            return ret;
        },
        __wbg_toString_3272fa0dfd05dd87: function(arg0) {
            const ret = arg0.toString();
            return ret;
        },
        __wbg_transaction_1309b463c399d2b3: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            const ret = arg0.transaction(getStringFromWasm0(arg1, arg2), __wbindgen_enum_IdbTransactionMode[arg3]);
            return ret;
        }, arguments); },
        __wbg_transaction_5eb9f1f16e8c769b: function(arg0) {
            const ret = arg0.transaction;
            return ret;
        },
        __wbg_type_7a6bb36555a59d6d: function(arg0, arg1) {
            const ret = arg1.type;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_update_8a72b950fc834630: function() { return handleError(function (arg0) {
            const ret = arg0.update();
            return ret;
        }, arguments); },
        __wbg_upperBound_482c10cb5e387300: function() { return handleError(function (arg0, arg1) {
            const ret = IDBKeyRange.upperBound(arg0, arg1 !== 0);
            return ret;
        }, arguments); },
        __wbg_url_7fefc1820fba4e0c: function(arg0, arg1) {
            const ret = arg1.url;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_value_21fc78aab0322612: function(arg0) {
            const ret = arg0.value;
            return ret;
        },
        __wbg_value_567d71719bef8eda: function(arg0, arg1) {
            const ret = arg1.value;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_value_6aa0a31ba8758e68: function(arg0, arg1) {
            const ret = arg1.value;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_versions_276b2795b1c6a219: function(arg0) {
            const ret = arg0.versions;
            return ret;
        },
        __wbg_warn_69424c2d92a2fa73: function(arg0) {
            console.warn(arg0);
        },
        __wbindgen_cast_0000000000000001: function(arg0, arg1) {
            // Cast intrinsic for `Closure(Closure { dtor_idx: 1219, function: Function { arguments: [NamedExternref("CloseEvent")], shim_idx: 1220, ret: Unit, inner_ret: Some(Unit) }, mutable: true }) -> Externref`.
            const ret = makeMutClosure(arg0, arg1, wasm.wasm_bindgen__closure__destroy__h3c65b065dd300ac3, wasm_bindgen__convert__closures_____invoke__h6624c5095f57530c);
            return ret;
        },
        __wbindgen_cast_0000000000000002: function(arg0, arg1) {
            // Cast intrinsic for `Closure(Closure { dtor_idx: 1219, function: Function { arguments: [NamedExternref("Event")], shim_idx: 1220, ret: Unit, inner_ret: Some(Unit) }, mutable: true }) -> Externref`.
            const ret = makeMutClosure(arg0, arg1, wasm.wasm_bindgen__closure__destroy__h3c65b065dd300ac3, wasm_bindgen__convert__closures_____invoke__h6624c5095f57530c_1);
            return ret;
        },
        __wbindgen_cast_0000000000000003: function(arg0, arg1) {
            // Cast intrinsic for `Closure(Closure { dtor_idx: 1219, function: Function { arguments: [NamedExternref("MessageEvent")], shim_idx: 1220, ret: Unit, inner_ret: Some(Unit) }, mutable: true }) -> Externref`.
            const ret = makeMutClosure(arg0, arg1, wasm.wasm_bindgen__closure__destroy__h3c65b065dd300ac3, wasm_bindgen__convert__closures_____invoke__h6624c5095f57530c_2);
            return ret;
        },
        __wbindgen_cast_0000000000000004: function(arg0, arg1) {
            // Cast intrinsic for `Closure(Closure { dtor_idx: 2055, function: Function { arguments: [Externref], shim_idx: 2056, ret: Unit, inner_ret: Some(Unit) }, mutable: true }) -> Externref`.
            const ret = makeMutClosure(arg0, arg1, wasm.wasm_bindgen__closure__destroy__hc247d80033958570, wasm_bindgen__convert__closures_____invoke__he907fb981620901a);
            return ret;
        },
        __wbindgen_cast_0000000000000005: function(arg0, arg1) {
            // Cast intrinsic for `Closure(Closure { dtor_idx: 2070, function: Function { arguments: [], shim_idx: 2071, ret: Unit, inner_ret: Some(Unit) }, mutable: true }) -> Externref`.
            const ret = makeMutClosure(arg0, arg1, wasm.wasm_bindgen__closure__destroy__hc31550165bc6001b, wasm_bindgen__convert__closures_____invoke__h1cc47d20eab369f7);
            return ret;
        },
        __wbindgen_cast_0000000000000006: function(arg0, arg1) {
            // Cast intrinsic for `Closure(Closure { dtor_idx: 2284, function: Function { arguments: [Externref], shim_idx: 2285, ret: Result(Unit), inner_ret: Some(Result(Unit)) }, mutable: true }) -> Externref`.
            const ret = makeMutClosure(arg0, arg1, wasm.wasm_bindgen__closure__destroy__h3cc5c66b7a5e0475, wasm_bindgen__convert__closures_____invoke__hbd5b09e5a07c0730);
            return ret;
        },
        __wbindgen_cast_0000000000000007: function(arg0, arg1) {
            // Cast intrinsic for `Closure(Closure { dtor_idx: 2298, function: Function { arguments: [], shim_idx: 2299, ret: Unit, inner_ret: Some(Unit) }, mutable: true }) -> Externref`.
            const ret = makeMutClosure(arg0, arg1, wasm.wasm_bindgen__closure__destroy__h483aefe82405ac38, wasm_bindgen__convert__closures_____invoke__h12cdbd0b5481bbdc);
            return ret;
        },
        __wbindgen_cast_0000000000000008: function(arg0, arg1) {
            // Cast intrinsic for `Closure(Closure { dtor_idx: 466, function: Function { arguments: [NamedExternref("IDBVersionChangeEvent")], shim_idx: 467, ret: Result(Unit), inner_ret: Some(Result(Unit)) }, mutable: true }) -> Externref`.
            const ret = makeMutClosure(arg0, arg1, wasm.wasm_bindgen__closure__destroy__ha0a027c7be291bfe, wasm_bindgen__convert__closures_____invoke__h19fda539fea930c1);
            return ret;
        },
        __wbindgen_cast_0000000000000009: function(arg0, arg1) {
            // Cast intrinsic for `Closure(Closure { dtor_idx: 947, function: Function { arguments: [NamedExternref("Event")], shim_idx: 948, ret: Unit, inner_ret: Some(Unit) }, mutable: true }) -> Externref`.
            const ret = makeMutClosure(arg0, arg1, wasm.wasm_bindgen__closure__destroy__h8d40991c835ccc2c, wasm_bindgen__convert__closures_____invoke__hf9871495c501e09e);
            return ret;
        },
        __wbindgen_cast_000000000000000a: function(arg0, arg1) {
            // Cast intrinsic for `Closure(Closure { dtor_idx: 947, function: Function { arguments: [], shim_idx: 950, ret: Unit, inner_ret: Some(Unit) }, mutable: true }) -> Externref`.
            const ret = makeMutClosure(arg0, arg1, wasm.wasm_bindgen__closure__destroy__h8d40991c835ccc2c, wasm_bindgen__convert__closures_____invoke__ha0287ba3365b576b);
            return ret;
        },
        __wbindgen_cast_000000000000000b: function(arg0) {
            // Cast intrinsic for `F64 -> Externref`.
            const ret = arg0;
            return ret;
        },
        __wbindgen_cast_000000000000000c: function(arg0) {
            // Cast intrinsic for `I64 -> Externref`.
            const ret = arg0;
            return ret;
        },
        __wbindgen_cast_000000000000000d: function(arg0, arg1) {
            // Cast intrinsic for `Ref(Slice(U8)) -> NamedExternref("Uint8Array")`.
            const ret = getArrayU8FromWasm0(arg0, arg1);
            return ret;
        },
        __wbindgen_cast_000000000000000e: function(arg0, arg1) {
            // Cast intrinsic for `Ref(String) -> Externref`.
            const ret = getStringFromWasm0(arg0, arg1);
            return ret;
        },
        __wbindgen_cast_000000000000000f: function(arg0) {
            // Cast intrinsic for `U64 -> Externref`.
            const ret = BigInt.asUintN(64, arg0);
            return ret;
        },
        __wbindgen_cast_0000000000000010: function(arg0, arg1) {
            var v0 = getArrayJsValueFromWasm0(arg0, arg1).slice();
            wasm.__wbindgen_free(arg0, arg1 * 4, 4);
            // Cast intrinsic for `Vector(NamedExternref("string")) -> Externref`.
            const ret = v0;
            return ret;
        },
        __wbindgen_cast_0000000000000011: function(arg0, arg1) {
            var v0 = getArrayU8FromWasm0(arg0, arg1).slice();
            wasm.__wbindgen_free(arg0, arg1 * 1, 1);
            // Cast intrinsic for `Vector(U8) -> Externref`.
            const ret = v0;
            return ret;
        },
        __wbindgen_init_externref_table: function() {
            const table = wasm.__wbindgen_externrefs;
            const offset = table.grow(4);
            table.set(0, undefined);
            table.set(offset + 0, undefined);
            table.set(offset + 1, null);
            table.set(offset + 2, true);
            table.set(offset + 3, false);
        },
    };
    return {
        __proto__: null,
        "./weeb_3_bg.js": import0,
    };
}

function wasm_bindgen__convert__closures_____invoke__h1cc47d20eab369f7(arg0, arg1) {
    wasm.wasm_bindgen__convert__closures_____invoke__h1cc47d20eab369f7(arg0, arg1);
}

function wasm_bindgen__convert__closures_____invoke__h12cdbd0b5481bbdc(arg0, arg1) {
    wasm.wasm_bindgen__convert__closures_____invoke__h12cdbd0b5481bbdc(arg0, arg1);
}

function wasm_bindgen__convert__closures_____invoke__ha0287ba3365b576b(arg0, arg1) {
    wasm.wasm_bindgen__convert__closures_____invoke__ha0287ba3365b576b(arg0, arg1);
}

function wasm_bindgen__convert__closures_____invoke__h6624c5095f57530c(arg0, arg1, arg2) {
    wasm.wasm_bindgen__convert__closures_____invoke__h6624c5095f57530c(arg0, arg1, arg2);
}

function wasm_bindgen__convert__closures_____invoke__h6624c5095f57530c_1(arg0, arg1, arg2) {
    wasm.wasm_bindgen__convert__closures_____invoke__h6624c5095f57530c_1(arg0, arg1, arg2);
}

function wasm_bindgen__convert__closures_____invoke__h6624c5095f57530c_2(arg0, arg1, arg2) {
    wasm.wasm_bindgen__convert__closures_____invoke__h6624c5095f57530c_2(arg0, arg1, arg2);
}

function wasm_bindgen__convert__closures_____invoke__he907fb981620901a(arg0, arg1, arg2) {
    wasm.wasm_bindgen__convert__closures_____invoke__he907fb981620901a(arg0, arg1, arg2);
}

function wasm_bindgen__convert__closures_____invoke__hf9871495c501e09e(arg0, arg1, arg2) {
    wasm.wasm_bindgen__convert__closures_____invoke__hf9871495c501e09e(arg0, arg1, arg2);
}

function wasm_bindgen__convert__closures_____invoke__hbd5b09e5a07c0730(arg0, arg1, arg2) {
    const ret = wasm.wasm_bindgen__convert__closures_____invoke__hbd5b09e5a07c0730(arg0, arg1, arg2);
    if (ret[1]) {
        throw takeFromExternrefTable0(ret[0]);
    }
}

function wasm_bindgen__convert__closures_____invoke__h19fda539fea930c1(arg0, arg1, arg2) {
    const ret = wasm.wasm_bindgen__convert__closures_____invoke__h19fda539fea930c1(arg0, arg1, arg2);
    if (ret[1]) {
        throw takeFromExternrefTable0(ret[0]);
    }
}

function wasm_bindgen__convert__closures_____invoke__h025f346370eef057(arg0, arg1, arg2, arg3) {
    wasm.wasm_bindgen__convert__closures_____invoke__h025f346370eef057(arg0, arg1, arg2, arg3);
}


const __wbindgen_enum_BinaryType = ["blob", "arraybuffer"];


const __wbindgen_enum_IdbRequestReadyState = ["pending", "done"];


const __wbindgen_enum_IdbTransactionMode = ["readonly", "readwrite", "versionchange", "readwriteflush", "cleanup"];


const __wbindgen_enum_RequestCredentials = ["omit", "same-origin", "include"];


const __wbindgen_enum_RequestMode = ["same-origin", "no-cors", "cors", "navigate"];
const BootstrapNodeFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_bootstrapnode_free(ptr >>> 0, 1));
const RequestArgumentsFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_requestarguments_free(ptr >>> 0, 1));
const SekireiFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_sekirei_free(ptr >>> 0, 1));
const SekireiNo103Finalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_sekireino103_free(ptr >>> 0, 1));
const WingsFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_wings_free(ptr >>> 0, 1));

function addToExternrefTable0(obj) {
    const idx = wasm.__externref_table_alloc();
    wasm.__wbindgen_externrefs.set(idx, obj);
    return idx;
}

const CLOSURE_DTORS = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(state => state.dtor(state.a, state.b));

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

function getArrayU8FromWasm0(ptr, len) {
    ptr = ptr >>> 0;
    return getUint8ArrayMemory0().subarray(ptr / 1, ptr / 1 + len);
}

let cachedDataViewMemory0 = null;
function getDataViewMemory0() {
    if (cachedDataViewMemory0 === null || cachedDataViewMemory0.buffer.detached === true || (cachedDataViewMemory0.buffer.detached === undefined && cachedDataViewMemory0.buffer !== wasm.memory.buffer)) {
        cachedDataViewMemory0 = new DataView(wasm.memory.buffer);
    }
    return cachedDataViewMemory0;
}

function getStringFromWasm0(ptr, len) {
    ptr = ptr >>> 0;
    return decodeText(ptr, len);
}

let cachedUint8ArrayMemory0 = null;
function getUint8ArrayMemory0() {
    if (cachedUint8ArrayMemory0 === null || cachedUint8ArrayMemory0.byteLength === 0) {
        cachedUint8ArrayMemory0 = new Uint8Array(wasm.memory.buffer);
    }
    return cachedUint8ArrayMemory0;
}

function handleError(f, args) {
    try {
        return f.apply(this, args);
    } catch (e) {
        const idx = addToExternrefTable0(e);
        wasm.__wbindgen_exn_store(idx);
    }
}

function isLikeNone(x) {
    return x === undefined || x === null;
}

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

function passArray8ToWasm0(arg, malloc) {
    const ptr = malloc(arg.length * 1, 1) >>> 0;
    getUint8ArrayMemory0().set(arg, ptr / 1);
    WASM_VECTOR_LEN = arg.length;
    return ptr;
}

function passArrayJsValueToWasm0(array, malloc) {
    const ptr = malloc(array.length * 4, 4) >>> 0;
    for (let i = 0; i < array.length; i++) {
        const add = addToExternrefTable0(array[i]);
        getDataViewMemory0().setUint32(ptr + 4 * i, add, true);
    }
    WASM_VECTOR_LEN = array.length;
    return ptr;
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

function takeFromExternrefTable0(idx) {
    const value = wasm.__wbindgen_externrefs.get(idx);
    wasm.__externref_table_dealloc(idx);
    return value;
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

const cachedTextEncoder = new TextEncoder();

if (!('encodeInto' in cachedTextEncoder)) {
    cachedTextEncoder.encodeInto = function (arg, view) {
        const buf = cachedTextEncoder.encode(arg);
        view.set(buf);
        return {
            read: arg.length,
            written: buf.length
        };
    };
}

let WASM_VECTOR_LEN = 0;

let wasmModule, wasm;
function __wbg_finalize_init(instance, module) {
    wasm = instance.exports;
    wasmModule = module;
    cachedDataViewMemory0 = null;
    cachedUint8ArrayMemory0 = null;
    wasm.__wbindgen_start();
    return wasm;
}

async function __wbg_load(module, imports) {
    if (typeof Response === 'function' && module instanceof Response) {
        if (typeof WebAssembly.instantiateStreaming === 'function') {
            try {
                return await WebAssembly.instantiateStreaming(module, imports);
            } catch (e) {
                const validResponse = module.ok && expectedResponseType(module.type);

                if (validResponse && module.headers.get('Content-Type') !== 'application/wasm') {
                    console.warn("`WebAssembly.instantiateStreaming` failed because your server does not serve Wasm with `application/wasm` MIME type. Falling back to `WebAssembly.instantiate` which is slower. Original error:\n", e);

                } else { throw e; }
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

    function expectedResponseType(type) {
        switch (type) {
            case 'basic': case 'cors': case 'default': return true;
        }
        return false;
    }
}

function initSync(module) {
    if (wasm !== undefined) return wasm;


    if (module !== undefined) {
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


    if (module_or_path !== undefined) {
        if (Object.getPrototypeOf(module_or_path) === Object.prototype) {
            ({module_or_path} = module_or_path)
        } else {
            console.warn('using deprecated parameters for the initialization function; pass a single object instead')
        }
    }

    if (module_or_path === undefined) {
        module_or_path = new URL('weeb_3_bg.wasm', import.meta.url);
    }
    const imports = __wbg_get_imports();

    if (typeof module_or_path === 'string' || (typeof Request === 'function' && module_or_path instanceof Request) || (typeof URL === 'function' && module_or_path instanceof URL)) {
        module_or_path = fetch(module_or_path);
    }

    const { instance, module } = await __wbg_load(await module_or_path, imports);

    return __wbg_finalize_init(instance, module);
}

export { initSync, __wbg_init as default };
