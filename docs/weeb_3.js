/* @ts-self-types="./weeb_3.d.ts" */
import { get_provider_js } from './snippets/web3-0742d85b024bb6f5/inline0.js';
import { loadHls } from './snippets/weeb_3-03f860286800ffdb/static/hls_loader.js';

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
            const retptr = wasm.__wbindgen_add_to_stack_pointer(-16);
            wasm.requestarguments_method(retptr, this.__wbg_ptr);
            var r0 = getDataViewMemory0().getInt32(retptr + 4 * 0, true);
            var r1 = getDataViewMemory0().getInt32(retptr + 4 * 1, true);
            deferred1_0 = r0;
            deferred1_1 = r1;
            return getStringFromWasm0(r0, r1);
        } finally {
            wasm.__wbindgen_add_to_stack_pointer(16);
            wasm.__wbindgen_export4(deferred1_0, deferred1_1, 1);
        }
    }
    /**
     * @returns {Array<any>}
     */
    get params() {
        const ret = wasm.requestarguments_params(this.__wbg_ptr);
        return takeObject(ret);
    }
}
if (Symbol.dispose) RequestArguments.prototype[Symbol.dispose] = RequestArguments.prototype.free;

export class Weeb3No103 {
    __destroy_into_raw() {
        const ptr = this.__wbg_ptr;
        this.__wbg_ptr = 0;
        Weeb3No103Finalization.unregister(this);
        return ptr;
    }
    free() {
        const ptr = this.__destroy_into_raw();
        wasm.__wbg_weeb3no103_free(ptr, 0);
    }
    /**
     * @param {string} owner
     * @param {string} topic
     * @returns {Promise<object>}
     */
    acquireFeedBytes(owner, topic) {
        const ptr0 = passStringToWasm0(owner, wasm.__wbindgen_export, wasm.__wbindgen_export2);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passStringToWasm0(topic, wasm.__wbindgen_export, wasm.__wbindgen_export2);
        const len1 = WASM_VECTOR_LEN;
        const ret = wasm.weeb3no103_acquireFeedBytes(this.__wbg_ptr, ptr0, len0, ptr1, len1);
        return takeObject(ret);
    }
    /**
     * @param {HTMLMediaElement} media
     * @param {string} owner
     * @param {string} topic
     * @param {HlsStart} start
     * @returns {Promise<void>}
     */
    attachStream(media, owner, topic, start) {
        const ptr0 = passStringToWasm0(owner, wasm.__wbindgen_export, wasm.__wbindgen_export2);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passStringToWasm0(topic, wasm.__wbindgen_export, wasm.__wbindgen_export2);
        const len1 = WASM_VECTOR_LEN;
        const ptr2 = passStringToWasm0(start, wasm.__wbindgen_export, wasm.__wbindgen_export2);
        const len2 = WASM_VECTOR_LEN;
        const ret = wasm.weeb3no103_attachStream(this.__wbg_ptr, addHeapObject(media), ptr0, len0, ptr1, len1, ptr2, len2);
        return takeObject(ret);
    }
    /**
     * @param {number} depth
     * @param {number} validity_days
     * @returns {Promise<object>}
     */
    batchState(depth, validity_days) {
        const ret = wasm.weeb3no103_batchState(this.__wbg_ptr, depth, validity_days);
        return takeObject(ret);
    }
    /**
     * @param {number} depth
     * @param {number} validity_days
     * @returns {Promise<object>}
     */
    buyBatch(depth, validity_days) {
        const ret = wasm.weeb3no103_buyBatch(this.__wbg_ptr, depth, validity_days);
        return takeObject(ret);
    }
    /**
     * @returns {Promise<object>}
     */
    deployChequebook() {
        const ret = wasm.weeb3no103_deployChequebook(this.__wbg_ptr);
        return takeObject(ret);
    }
    /**
     * @param {string} amount
     * @returns {Promise<object>}
     */
    depositChequebook(amount) {
        const ptr0 = passStringToWasm0(amount, wasm.__wbindgen_export, wasm.__wbindgen_export2);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.weeb3no103_depositChequebook(this.__wbg_ptr, ptr0, len0);
        return takeObject(ret);
    }
    /**
     * @returns {Promise<Array<any>>}
     */
    logs() {
        const ret = wasm.weeb3no103_logs(this.__wbg_ptr);
        return takeObject(ret);
    }
    /**
     * @returns {Promise<object>}
     */
    networkState() {
        const ret = wasm.weeb3no103_networkState(this.__wbg_ptr);
        return takeObject(ret);
    }
    /**
     * @param {string | null} [shared_worker_url]
     */
    constructor(shared_worker_url) {
        var ptr0 = isLikeNone(shared_worker_url) ? 0 : passStringToWasm0(shared_worker_url, wasm.__wbindgen_export, wasm.__wbindgen_export2);
        var len0 = WASM_VECTOR_LEN;
        const ret = wasm.weeb3no103_new(ptr0, len0);
        this.__wbg_ptr = ret >>> 0;
        Weeb3No103Finalization.register(this, this.__wbg_ptr, this);
        return this;
    }
    /**
     * @returns {object}
     */
    openSecureVault() {
        const ret = wasm.weeb3no103_openSecureVault(this.__wbg_ptr);
        return takeObject(ret);
    }
    /**
     * @param {string} topic
     * @param {Uint8Array} bytes
     * @param {string} mime
     * @param {string} filename
     * @param {boolean} encryption
     * @returns {Promise<object>}
     */
    postFeedBytes(topic, bytes, mime, filename, encryption) {
        const ptr0 = passStringToWasm0(topic, wasm.__wbindgen_export, wasm.__wbindgen_export2);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passStringToWasm0(mime, wasm.__wbindgen_export, wasm.__wbindgen_export2);
        const len1 = WASM_VECTOR_LEN;
        const ptr2 = passStringToWasm0(filename, wasm.__wbindgen_export, wasm.__wbindgen_export2);
        const len2 = WASM_VECTOR_LEN;
        const ret = wasm.weeb3no103_postFeedBytes(this.__wbg_ptr, ptr0, len0, addHeapObject(bytes), ptr1, len1, ptr2, len2, encryption);
        return takeObject(ret);
    }
    /**
     * @param {Uint8Array} data
     * @param {boolean} soc
     * @param {Uint8Array} chunk_address
     * @param {Uint8Array} stamp
     * @returns {Promise<string>}
     */
    postPushChunk(data, soc, chunk_address, stamp) {
        const ret = wasm.weeb3no103_postPushChunk(this.__wbg_ptr, addHeapObject(data), soc, addHeapObject(chunk_address), addHeapObject(stamp));
        return takeObject(ret);
    }
    /**
     * @param {Uint8Array} bytes
     * @param {string} mime
     * @param {string} filename
     * @param {boolean} encryption
     * @param {boolean} add_to_feed
     * @param {string} feed_topic
     * @returns {Promise<object>}
     */
    postUploadBytes(bytes, mime, filename, encryption, add_to_feed, feed_topic) {
        const ptr0 = passStringToWasm0(mime, wasm.__wbindgen_export, wasm.__wbindgen_export2);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passStringToWasm0(filename, wasm.__wbindgen_export, wasm.__wbindgen_export2);
        const len1 = WASM_VECTOR_LEN;
        const ptr2 = passStringToWasm0(feed_topic, wasm.__wbindgen_export, wasm.__wbindgen_export2);
        const len2 = WASM_VECTOR_LEN;
        const ret = wasm.weeb3no103_postUploadBytes(this.__wbg_ptr, addHeapObject(bytes), ptr0, len0, ptr1, len1, encryption, add_to_feed, ptr2, len2);
        return takeObject(ret);
    }
    /**
     * @param {Uint8Array} bytes
     * @param {string} mime
     * @param {string} filename
     * @param {boolean} encryption
     * @param {UploadRedundancyLevel} redundancy_level
     * @param {boolean} add_to_feed
     * @param {string} feed_topic
     * @returns {Promise<object>}
     */
    postUploadBytesWithRedundancy(bytes, mime, filename, encryption, redundancy_level, add_to_feed, feed_topic) {
        const ptr0 = passStringToWasm0(mime, wasm.__wbindgen_export, wasm.__wbindgen_export2);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passStringToWasm0(filename, wasm.__wbindgen_export, wasm.__wbindgen_export2);
        const len1 = WASM_VECTOR_LEN;
        const ptr2 = passStringToWasm0(feed_topic, wasm.__wbindgen_export, wasm.__wbindgen_export2);
        const len2 = WASM_VECTOR_LEN;
        const ret = wasm.weeb3no103_postUploadBytesWithRedundancy(this.__wbg_ptr, addHeapObject(bytes), ptr0, len0, ptr1, len1, encryption, redundancy_level, add_to_feed, ptr2, len2);
        return takeObject(ret);
    }
    /**
     * @param {number} seen_revision
     * @returns {Promise<object>}
     */
    progressSnapshot(seen_revision) {
        const ret = wasm.weeb3no103_progressSnapshot(this.__wbg_ptr, seen_revision);
        return takeObject(ret);
    }
    /**
     * @param {number} min_connections
     * @param {number} timeout_ms
     * @returns {Promise<boolean>}
     */
    ready(min_connections, timeout_ms) {
        const ret = wasm.weeb3no103_ready(this.__wbg_ptr, min_connections, timeout_ms);
        return takeObject(ret);
    }
    /**
     * @param {Element} container
     * @returns {object}
     */
    renderInterface(container) {
        const ret = wasm.weeb3no103_renderInterface(this.__wbg_ptr, addHeapObject(container));
        return takeObject(ret);
    }
    /**
     * @returns {Promise<object>}
     */
    resetStamp() {
        const ret = wasm.weeb3no103_resetStamp(this.__wbg_ptr);
        return takeObject(ret);
    }
    /**
     * @param {string} address
     * @returns {Promise<Array<any>>}
     */
    retrieve(address) {
        const ptr0 = passStringToWasm0(address, wasm.__wbindgen_export, wasm.__wbindgen_export2);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.weeb3no103_retrieve(this.__wbg_ptr, ptr0, len0);
        return takeObject(ret);
    }
    /**
     * @param {string} address
     * @returns {Promise<Uint8Array>}
     */
    retrieveBytes(address) {
        const ptr0 = passStringToWasm0(address, wasm.__wbindgen_export, wasm.__wbindgen_export2);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.weeb3no103_retrieveBytes(this.__wbg_ptr, ptr0, len0);
        return takeObject(ret);
    }
    /**
     * @param {string} address
     * @returns {Promise<Uint8Array>}
     */
    retrieveChunk(address) {
        const ptr0 = passStringToWasm0(address, wasm.__wbindgen_export, wasm.__wbindgen_export2);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.weeb3no103_retrieveChunk(this.__wbg_ptr, ptr0, len0);
        return takeObject(ret);
    }
    /**
     * @param {any | null} [options]
     */
    start(options) {
        wasm.weeb3no103_start(this.__wbg_ptr, isLikeNone(options) ? 0 : addHeapObject(options));
    }
    /**
     * @param {string} mode
     * @returns {Promise<object>}
     */
    switchNetwork(mode) {
        const ptr0 = passStringToWasm0(mode, wasm.__wbindgen_export, wasm.__wbindgen_export2);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.weeb3no103_switchNetwork(this.__wbg_ptr, ptr0, len0);
        return takeObject(ret);
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
        const ptr0 = passStringToWasm0(index_string, wasm.__wbindgen_export, wasm.__wbindgen_export2);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passStringToWasm0(feed_topic, wasm.__wbindgen_export, wasm.__wbindgen_export2);
        const len1 = WASM_VECTOR_LEN;
        const ret = wasm.weeb3no103_upload(this.__wbg_ptr, addHeapObject(file), encryption, ptr0, len0, add_to_feed, ptr1, len1);
        return takeObject(ret);
    }
    /**
     * @param {File} file
     * @param {boolean} encryption
     * @param {UploadRedundancyLevel} redundancy_level
     * @param {string} index_string
     * @param {boolean} add_to_feed
     * @param {string} feed_topic
     * @returns {Promise<object>}
     */
    uploadWithRedundancy(file, encryption, redundancy_level, index_string, add_to_feed, feed_topic) {
        const ptr0 = passStringToWasm0(index_string, wasm.__wbindgen_export, wasm.__wbindgen_export2);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passStringToWasm0(feed_topic, wasm.__wbindgen_export, wasm.__wbindgen_export2);
        const len1 = WASM_VECTOR_LEN;
        const ret = wasm.weeb3no103_uploadWithRedundancy(this.__wbg_ptr, addHeapObject(file), encryption, redundancy_level, ptr0, len0, add_to_feed, ptr1, len1);
        return takeObject(ret);
    }
}
if (Symbol.dispose) Weeb3No103.prototype[Symbol.dispose] = Weeb3No103.prototype.free;

export class Weeb3WorkerRuntime {
    __destroy_into_raw() {
        const ptr = this.__wbg_ptr;
        this.__wbg_ptr = 0;
        Weeb3WorkerRuntimeFinalization.unregister(this);
        return ptr;
    }
    free() {
        const ptr = this.__destroy_into_raw();
        wasm.__wbg_weeb3workerruntime_free(ptr, 0);
    }
    /**
     * @param {any} message
     * @returns {Promise<object>}
     */
    handleMessage(message) {
        const ret = wasm.weeb3workerruntime_handleMessage(this.__wbg_ptr, addHeapObject(message));
        return takeObject(ret);
    }
    constructor() {
        const ret = wasm.weeb3workerruntime_new();
        this.__wbg_ptr = ret >>> 0;
        Weeb3WorkerRuntimeFinalization.register(this, this.__wbg_ptr, this);
        return this;
    }
    /**
     * @param {any} options
     * @returns {Promise<object>}
     */
    start(options) {
        const ret = wasm.weeb3workerruntime_start(this.__wbg_ptr, addHeapObject(options));
        return takeObject(ret);
    }
}
if (Symbol.dispose) Weeb3WorkerRuntime.prototype[Symbol.dispose] = Weeb3WorkerRuntime.prototype.free;

/**
 * @param {string} _st
 * @returns {Promise<void>}
 */
export function interweeb(_st) {
    const ptr0 = passStringToWasm0(_st, wasm.__wbindgen_export, wasm.__wbindgen_export2);
    const len0 = WASM_VECTOR_LEN;
    const ret = wasm.interweeb(ptr0, len0);
    return takeObject(ret);
}

function __wbg_get_imports() {
    const import0 = {
        __proto__: null,
        __wbg_Error_8c4e43fe74559d73: function(arg0, arg1) {
            const ret = Error(getStringFromWasm0(arg0, arg1));
            return addHeapObject(ret);
        },
        __wbg_Number_04624de7d0e8332d: function(arg0) {
            const ret = Number(getObject(arg0));
            return ret;
        },
        __wbg_Window_41559019033ede94: function(arg0) {
            const ret = getObject(arg0).Window;
            return addHeapObject(ret);
        },
        __wbg_WorkerGlobalScope_d324bffbeaef9f3a: function(arg0) {
            const ret = getObject(arg0).WorkerGlobalScope;
            return addHeapObject(ret);
        },
        __wbg_WorkerGlobalScope_f6889a7bbe9f0d2d: function(arg0) {
            const ret = getObject(arg0).WorkerGlobalScope;
            return addHeapObject(ret);
        },
        __wbg___wbindgen_bigint_get_as_i64_8fcf4ce7f1ca72a2: function(arg0, arg1) {
            const v = getObject(arg1);
            const ret = typeof(v) === 'bigint' ? v : undefined;
            getDataViewMemory0().setBigInt64(arg0 + 8 * 1, isLikeNone(ret) ? BigInt(0) : ret, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, !isLikeNone(ret), true);
        },
        __wbg___wbindgen_boolean_get_bbbb1c18aa2f5e25: function(arg0) {
            const v = getObject(arg0);
            const ret = typeof(v) === 'boolean' ? v : undefined;
            return isLikeNone(ret) ? 0xFFFFFF : ret ? 1 : 0;
        },
        __wbg___wbindgen_debug_string_0bc8482c6e3508ae: function(arg0, arg1) {
            const ret = debugString(getObject(arg1));
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_export, wasm.__wbindgen_export2);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg___wbindgen_in_47fa6863be6f2f25: function(arg0, arg1) {
            const ret = getObject(arg0) in getObject(arg1);
            return ret;
        },
        __wbg___wbindgen_is_bigint_31b12575b56f32fc: function(arg0) {
            const ret = typeof(getObject(arg0)) === 'bigint';
            return ret;
        },
        __wbg___wbindgen_is_function_0095a73b8b156f76: function(arg0) {
            const ret = typeof(getObject(arg0)) === 'function';
            return ret;
        },
        __wbg___wbindgen_is_null_ac34f5003991759a: function(arg0) {
            const ret = getObject(arg0) === null;
            return ret;
        },
        __wbg___wbindgen_is_object_5ae8e5880f2c1fbd: function(arg0) {
            const val = getObject(arg0);
            const ret = typeof(val) === 'object' && val !== null;
            return ret;
        },
        __wbg___wbindgen_is_string_cd444516edc5b180: function(arg0) {
            const ret = typeof(getObject(arg0)) === 'string';
            return ret;
        },
        __wbg___wbindgen_is_undefined_9e4d92534c42d778: function(arg0) {
            const ret = getObject(arg0) === undefined;
            return ret;
        },
        __wbg___wbindgen_jsval_eq_11888390b0186270: function(arg0, arg1) {
            const ret = getObject(arg0) === getObject(arg1);
            return ret;
        },
        __wbg___wbindgen_jsval_loose_eq_9dd77d8cd6671811: function(arg0, arg1) {
            const ret = getObject(arg0) == getObject(arg1);
            return ret;
        },
        __wbg___wbindgen_number_get_8ff4255516ccad3e: function(arg0, arg1) {
            const obj = getObject(arg1);
            const ret = typeof(obj) === 'number' ? obj : undefined;
            getDataViewMemory0().setFloat64(arg0 + 8 * 1, isLikeNone(ret) ? 0 : ret, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, !isLikeNone(ret), true);
        },
        __wbg___wbindgen_string_get_72fb696202c56729: function(arg0, arg1) {
            const obj = getObject(arg1);
            const ret = typeof(obj) === 'string' ? obj : undefined;
            var ptr1 = isLikeNone(ret) ? 0 : passStringToWasm0(ret, wasm.__wbindgen_export, wasm.__wbindgen_export2);
            var len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg___wbindgen_throw_be289d5034ed271b: function(arg0, arg1) {
            throw new Error(getStringFromWasm0(arg0, arg1));
        },
        __wbg__wbg_cb_unref_d9b87ff7982e3b21: function(arg0) {
            getObject(arg0)._wbg_cb_unref();
        },
        __wbg_abort_2f0584e03e8e3950: function(arg0) {
            getObject(arg0).abort();
        },
        __wbg_abort_5151361027dc87df: function() { return handleError(function (arg0) {
            getObject(arg0).abort();
        }, arguments); },
        __wbg_abort_d549b92d3c665de1: function(arg0, arg1) {
            getObject(arg0).abort(getObject(arg1));
        },
        __wbg_active_171b65111a0bc3af: function(arg0) {
            const ret = getObject(arg0).active;
            return isLikeNone(ret) ? 0 : addHeapObject(ret);
        },
        __wbg_addEventListener_3acb0aad4483804c: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            getObject(arg0).addEventListener(getStringFromWasm0(arg1, arg2), getObject(arg3));
        }, arguments); },
        __wbg_alert_6961e77eb545540e: function() { return handleError(function (arg0, arg1, arg2) {
            getObject(arg0).alert(getStringFromWasm0(arg1, arg2));
        }, arguments); },
        __wbg_appendChild_dea38765a26d346d: function() { return handleError(function (arg0, arg1) {
            const ret = getObject(arg0).appendChild(getObject(arg1));
            return addHeapObject(ret);
        }, arguments); },
        __wbg_append_a992ccc37aa62dc4: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4) {
            getObject(arg0).append(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4));
        }, arguments); },
        __wbg_apply_ada2ee1a60ac7b3c: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = getObject(arg0).apply(getObject(arg1), getObject(arg2));
            return addHeapObject(ret);
        }, arguments); },
        __wbg_arrayBuffer_05ce1af23e9064e8: function(arg0) {
            const ret = getObject(arg0).arrayBuffer();
            return addHeapObject(ret);
        },
        __wbg_arrayBuffer_bb54076166006c39: function() { return handleError(function (arg0) {
            const ret = getObject(arg0).arrayBuffer();
            return addHeapObject(ret);
        }, arguments); },
        __wbg_assign_4589c32a26ec6e6f: function() { return handleError(function (arg0, arg1, arg2) {
            getObject(arg0).assign(getStringFromWasm0(arg1, arg2));
        }, arguments); },
        __wbg_attachMedia_82d779df2fa8e98f: function() { return handleError(function (arg0, arg1) {
            getObject(arg0).attachMedia(getObject(arg1));
        }, arguments); },
        __wbg_body_f67922363a220026: function(arg0) {
            const ret = getObject(arg0).body;
            return isLikeNone(ret) ? 0 : addHeapObject(ret);
        },
        __wbg_bufferedAmount_0d42d0dc52062133: function(arg0) {
            const ret = getObject(arg0).bufferedAmount;
            return ret;
        },
        __wbg_buffered_1ec0d730a6813836: function(arg0) {
            const ret = getObject(arg0).buffered;
            return addHeapObject(ret);
        },
        __wbg_call_389efe28435a9388: function() { return handleError(function (arg0, arg1) {
            const ret = getObject(arg0).call(getObject(arg1));
            return addHeapObject(ret);
        }, arguments); },
        __wbg_call_4708e0c13bdc8e95: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = getObject(arg0).call(getObject(arg1), getObject(arg2));
            return addHeapObject(ret);
        }, arguments); },
        __wbg_canPlayType_941a61eb2660d5fa: function(arg0, arg1, arg2, arg3) {
            const ret = getObject(arg1).canPlayType(getStringFromWasm0(arg2, arg3));
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_export, wasm.__wbindgen_export2);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_checked_04db83ac6810bc82: function(arg0) {
            const ret = getObject(arg0).checked;
            return ret;
        },
        __wbg_childElementCount_d9ab73273b028870: function(arg0) {
            const ret = getObject(arg0).childElementCount;
            return ret;
        },
        __wbg_clearInterval_905868a11bace7cc: function(arg0, arg1) {
            getObject(arg0).clearInterval(arg1);
        },
        __wbg_clearInterval_c75df0651e74fbb8: function(arg0, arg1) {
            getObject(arg0).clearInterval(arg1);
        },
        __wbg_clearTimeout_42d9ccd50822fd3a: function(arg0) {
            const ret = clearTimeout(takeObject(arg0));
            return addHeapObject(ret);
        },
        __wbg_clearTimeout_5a54f8841c30079a: function(arg0) {
            const ret = clearTimeout(takeObject(arg0));
            return addHeapObject(ret);
        },
        __wbg_clearTimeout_96804de0ab838f26: function(arg0) {
            const ret = clearTimeout(takeObject(arg0));
            return addHeapObject(ret);
        },
        __wbg_click_0e9c20848b655ed3: function(arg0) {
            getObject(arg0).click();
        },
        __wbg_close_eef7356f70a62f3c: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            getObject(arg0).close(arg1, getStringFromWasm0(arg2, arg3));
        }, arguments); },
        __wbg_close_fad2f0ee451926ed: function(arg0) {
            getObject(arg0).close();
        },
        __wbg_closed_52189129f6f45fcb: function() { return handleError(function (arg0) {
            const ret = getObject(arg0).closed;
            return ret;
        }, arguments); },
        __wbg_commit_a54edce65f3858f2: function() { return handleError(function (arg0) {
            getObject(arg0).commit();
        }, arguments); },
        __wbg_confirm_0951f3942a645061: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = getObject(arg0).confirm(getStringFromWasm0(arg1, arg2));
            return ret;
        }, arguments); },
        __wbg_construct_86626e847de3b629: function() { return handleError(function (arg0, arg1) {
            const ret = Reflect.construct(getObject(arg0), getObject(arg1));
            return addHeapObject(ret);
        }, arguments); },
        __wbg_controller_a9760d306f05dccc: function(arg0) {
            const ret = getObject(arg0).controller;
            return isLikeNone(ret) ? 0 : addHeapObject(ret);
        },
        __wbg_createDocumentFragment_5a5d3f757bb2d1c0: function(arg0) {
            const ret = getObject(arg0).createDocumentFragment();
            return addHeapObject(ret);
        },
        __wbg_createElement_49f60fdcaae809c8: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = getObject(arg0).createElement(getStringFromWasm0(arg1, arg2));
            return addHeapObject(ret);
        }, arguments); },
        __wbg_createObjectStore_f75f59d55a549868: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            const ret = getObject(arg0).createObjectStore(getStringFromWasm0(arg1, arg2), getObject(arg3));
            return addHeapObject(ret);
        }, arguments); },
        __wbg_createObjectURL_918185db6a10a0c8: function() { return handleError(function (arg0, arg1) {
            const ret = URL.createObjectURL(getObject(arg1));
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_export, wasm.__wbindgen_export2);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        }, arguments); },
        __wbg_crypto_86f2631e91b51511: function(arg0) {
            const ret = getObject(arg0).crypto;
            return addHeapObject(ret);
        },
        __wbg_currentTime_3550761b514bd17a: function(arg0) {
            const ret = getObject(arg0).currentTime;
            return ret;
        },
        __wbg_data_5330da50312d0bc1: function(arg0) {
            const ret = getObject(arg0).data;
            return addHeapObject(ret);
        },
        __wbg_decodeURIComponent_c0a12f850c93e3c4: function() { return handleError(function (arg0, arg1) {
            const ret = decodeURIComponent(getStringFromWasm0(arg0, arg1));
            return addHeapObject(ret);
        }, arguments); },
        __wbg_destroy_91b983e1d85d5306: function() { return handleError(function (arg0) {
            getObject(arg0).destroy();
        }, arguments); },
        __wbg_document_ee35a3d3ae34ef6c: function(arg0) {
            const ret = getObject(arg0).document;
            return isLikeNone(ret) ? 0 : addHeapObject(ret);
        },
        __wbg_done_57b39ecd9addfe81: function(arg0) {
            const ret = getObject(arg0).done;
            return ret;
        },
        __wbg_duration_56d63063c1dcc6d7: function(arg0) {
            const ret = getObject(arg0).duration;
            return ret;
        },
        __wbg_encodeURIComponent_46e9eafddd38cb39: function(arg0, arg1) {
            const ret = encodeURIComponent(getStringFromWasm0(arg0, arg1));
            return addHeapObject(ret);
        },
        __wbg_end_7abba6e476a96dc5: function() { return handleError(function (arg0, arg1) {
            const ret = getObject(arg0).end(arg1 >>> 0);
            return ret;
        }, arguments); },
        __wbg_entries_58c7934c745daac7: function(arg0) {
            const ret = Object.entries(getObject(arg0));
            return addHeapObject(ret);
        },
        __wbg_error_6afb95c784775817: function() { return handleError(function (arg0) {
            const ret = getObject(arg0).error;
            return isLikeNone(ret) ? 0 : addHeapObject(ret);
        }, arguments); },
        __wbg_error_9a7fe3f932034cde: function(arg0) {
            console.error(getObject(arg0));
        },
        __wbg_fetch_6bbc32f991730587: function(arg0) {
            const ret = fetch(getObject(arg0));
            return addHeapObject(ret);
        },
        __wbg_fetch_afb6a4b6cacf876d: function(arg0, arg1) {
            const ret = getObject(arg0).fetch(getObject(arg1));
            return addHeapObject(ret);
        },
        __wbg_files_f9461f097760ef70: function(arg0) {
            const ret = getObject(arg0).files;
            return isLikeNone(ret) ? 0 : addHeapObject(ret);
        },
        __wbg_focus_e46131175db6a49d: function() { return handleError(function (arg0) {
            getObject(arg0).focus();
        }, arguments); },
        __wbg_from_bddd64e7d5ff6941: function(arg0) {
            const ret = Array.from(getObject(arg0));
            return addHeapObject(ret);
        },
        __wbg_getAttribute_b9f6fc4b689c71b0: function(arg0, arg1, arg2, arg3) {
            const ret = getObject(arg1).getAttribute(getStringFromWasm0(arg2, arg3));
            var ptr1 = isLikeNone(ret) ? 0 : passStringToWasm0(ret, wasm.__wbindgen_export, wasm.__wbindgen_export2);
            var len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_getElementById_e34377b79d7285f6: function(arg0, arg1, arg2) {
            const ret = getObject(arg0).getElementById(getStringFromWasm0(arg1, arg2));
            return isLikeNone(ret) ? 0 : addHeapObject(ret);
        },
        __wbg_getRandomValues_a8ddca022803a145: function() { return handleError(function (arg0, arg1) {
            globalThis.crypto.getRandomValues(getArrayU8FromWasm0(arg0, arg1));
        }, arguments); },
        __wbg_getRandomValues_b3f15fcbfabb0f8b: function() { return handleError(function (arg0, arg1) {
            getObject(arg0).getRandomValues(getObject(arg1));
        }, arguments); },
        __wbg_getRegistration_b0de80b77170d9ce: function(arg0) {
            const ret = getObject(arg0).getRegistration();
            return addHeapObject(ret);
        },
        __wbg_get_5e856edb32ac1289: function() { return handleError(function (arg0, arg1) {
            const ret = getObject(arg0).get(getObject(arg1));
            return addHeapObject(ret);
        }, arguments); },
        __wbg_get_9b94d73e6221f75c: function(arg0, arg1) {
            const ret = getObject(arg0)[arg1 >>> 0];
            return addHeapObject(ret);
        },
        __wbg_get_a558225cd5dd57cd: function(arg0, arg1, arg2, arg3) {
            const ret = getObject(arg1).get(getStringFromWasm0(arg2, arg3));
            var ptr1 = isLikeNone(ret) ? 0 : passStringToWasm0(ret, wasm.__wbindgen_export, wasm.__wbindgen_export2);
            var len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_get_b3ed3ad4be2bc8ac: function() { return handleError(function (arg0, arg1) {
            const ret = Reflect.get(getObject(arg0), getObject(arg1));
            return addHeapObject(ret);
        }, arguments); },
        __wbg_get_c60162cf03da5a6e: function(arg0, arg1) {
            const ret = getObject(arg0).get(getObject(arg1));
            return addHeapObject(ret);
        },
        __wbg_get_provider_js_3e4ff84c8bf90fbc: function() { return handleError(function () {
            const ret = get_provider_js();
            return isLikeNone(ret) ? 0 : addHeapObject(ret);
        }, arguments); },
        __wbg_get_with_ref_key_1dc361bd10053bfe: function(arg0, arg1) {
            const ret = getObject(arg0)[getObject(arg1)];
            return addHeapObject(ret);
        },
        __wbg_global_f5c2926e57ba457f: function(arg0) {
            const ret = getObject(arg0).global;
            return addHeapObject(ret);
        },
        __wbg_hasAttribute_5d8f52cb4c739280: function(arg0, arg1, arg2) {
            const ret = getObject(arg0).hasAttribute(getStringFromWasm0(arg1, arg2));
            return ret;
        },
        __wbg_has_d4e53238966c12b6: function() { return handleError(function (arg0, arg1) {
            const ret = Reflect.has(getObject(arg0), getObject(arg1));
            return ret;
        }, arguments); },
        __wbg_hash_90eadad0e1447454: function() { return handleError(function (arg0, arg1) {
            const ret = getObject(arg1).hash;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_export, wasm.__wbindgen_export2);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        }, arguments); },
        __wbg_headers_59a2938db9f80985: function(arg0) {
            const ret = getObject(arg0).headers;
            return addHeapObject(ret);
        },
        __wbg_hidden_8ce6a98b8c12451c: function(arg0) {
            const ret = getObject(arg0).hidden;
            return ret;
        },
        __wbg_history_d6c2f9abc6e74880: function() { return handleError(function (arg0) {
            const ret = getObject(arg0).history;
            return addHeapObject(ret);
        }, arguments); },
        __wbg_href_67854c3dd511f6f3: function() { return handleError(function (arg0, arg1) {
            const ret = getObject(arg1).href;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_export, wasm.__wbindgen_export2);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        }, arguments); },
        __wbg_href_874205b16220eda9: function(arg0, arg1) {
            const ret = getObject(arg1).href;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_export, wasm.__wbindgen_export2);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_indexedDB_54f01430b1e194e8: function() { return handleError(function (arg0) {
            const ret = getObject(arg0).indexedDB;
            return isLikeNone(ret) ? 0 : addHeapObject(ret);
        }, arguments); },
        __wbg_indexedDB_782f0610ea9fb144: function() { return handleError(function (arg0) {
            const ret = getObject(arg0).indexedDB;
            return isLikeNone(ret) ? 0 : addHeapObject(ret);
        }, arguments); },
        __wbg_indexedDB_9ddfb31df70de83b: function() { return handleError(function (arg0) {
            const ret = getObject(arg0).indexedDB;
            return isLikeNone(ret) ? 0 : addHeapObject(ret);
        }, arguments); },
        __wbg_installing_860506570e3bb33d: function(arg0) {
            const ret = getObject(arg0).installing;
            return isLikeNone(ret) ? 0 : addHeapObject(ret);
        },
        __wbg_instanceof_ArrayBuffer_c367199e2fa2aa04: function(arg0) {
            let result;
            try {
                result = getObject(arg0) instanceof ArrayBuffer;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_DomException_99c177193e554b75: function(arg0) {
            let result;
            try {
                result = getObject(arg0) instanceof DOMException;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Error_8573fe0b0b480f46: function(arg0) {
            let result;
            try {
                result = getObject(arg0) instanceof Error;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_EventTarget_546fcb80b9e6e593: function(arg0) {
            let result;
            try {
                result = getObject(arg0) instanceof EventTarget;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_File_21240124aa87092d: function(arg0) {
            let result;
            try {
                result = getObject(arg0) instanceof File;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_HtmlButtonElement_8de22afc17b28c76: function(arg0) {
            let result;
            try {
                result = getObject(arg0) instanceof HTMLButtonElement;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_HtmlElement_5abfac207260fd6f: function(arg0) {
            let result;
            try {
                result = getObject(arg0) instanceof HTMLElement;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_HtmlInputElement_c10b7260b4e0710a: function(arg0) {
            let result;
            try {
                result = getObject(arg0) instanceof HTMLInputElement;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_HtmlMediaElement_d46c5b9e4bc946c0: function(arg0) {
            let result;
            try {
                result = getObject(arg0) instanceof HTMLMediaElement;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_HtmlSelectElement_a1820832b22b0d1e: function(arg0) {
            let result;
            try {
                result = getObject(arg0) instanceof HTMLSelectElement;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_HtmlSpanElement_e8235c42a0941e0d: function(arg0) {
            let result;
            try {
                result = getObject(arg0) instanceof HTMLSpanElement;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_IdbDatabase_8d723b3ff4761c2d: function(arg0) {
            let result;
            try {
                result = getObject(arg0) instanceof IDBDatabase;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_IdbRequest_6388508cc77f8da0: function(arg0) {
            let result;
            try {
                result = getObject(arg0) instanceof IDBRequest;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Map_53af74335dec57f4: function(arg0) {
            let result;
            try {
                result = getObject(arg0) instanceof Map;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_MessagePort_b42e3b3e52b04e04: function(arg0) {
            let result;
            try {
                result = getObject(arg0) instanceof MessagePort;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Object_1c6af87502b733ed: function(arg0) {
            let result;
            try {
                result = getObject(arg0) instanceof Object;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Promise_0094681e3519d6ec: function(arg0) {
            let result;
            try {
                result = getObject(arg0) instanceof Promise;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Response_ee1d54d79ae41977: function(arg0) {
            let result;
            try {
                result = getObject(arg0) instanceof Response;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_ServiceWorkerContainer_fdb5166e52ab7e46: function(arg0) {
            let result;
            try {
                result = getObject(arg0) instanceof ServiceWorkerContainer;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_ServiceWorkerRegistration_8e649dbc7bef531c: function(arg0) {
            let result;
            try {
                result = getObject(arg0) instanceof ServiceWorkerRegistration;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Uint8Array_9b9075935c74707c: function(arg0) {
            let result;
            try {
                result = getObject(arg0) instanceof Uint8Array;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Window_ed49b2db8df90359: function(arg0) {
            let result;
            try {
                result = getObject(arg0) instanceof Window;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_isArray_d314bb98fcf08331: function(arg0) {
            const ret = Array.isArray(getObject(arg0));
            return ret;
        },
        __wbg_isConnected_6110bc2c5462f90e: function(arg0) {
            const ret = getObject(arg0).isConnected;
            return ret;
        },
        __wbg_isSafeInteger_bfbc7332a9768d2a: function(arg0) {
            const ret = Number.isSafeInteger(getObject(arg0));
            return ret;
        },
        __wbg_is_f29129f676e5410c: function(arg0, arg1) {
            const ret = Object.is(getObject(arg0), getObject(arg1));
            return ret;
        },
        __wbg_item_c79c0bccbcfd8735: function(arg0, arg1) {
            const ret = getObject(arg0).item(arg1 >>> 0);
            return isLikeNone(ret) ? 0 : addHeapObject(ret);
        },
        __wbg_iterator_6ff6560ca1568e55: function() {
            const ret = Symbol.iterator;
            return addHeapObject(ret);
        },
        __wbg_lastElementChild_573dd113d3e3444f: function(arg0) {
            const ret = getObject(arg0).lastElementChild;
            return isLikeNone(ret) ? 0 : addHeapObject(ret);
        },
        __wbg_length_107d36902d56348c: function(arg0) {
            const ret = getObject(arg0).length;
            return ret;
        },
        __wbg_length_32ed9a279acd054c: function(arg0) {
            const ret = getObject(arg0).length;
            return ret;
        },
        __wbg_length_35a7bace40f36eac: function(arg0) {
            const ret = getObject(arg0).length;
            return ret;
        },
        __wbg_loadHls_5978d37148239824: function() {
            const ret = loadHls();
            return addHeapObject(ret);
        },
        __wbg_loadSource_c8e9cd20de2dd2c3: function() { return handleError(function (arg0, arg1, arg2) {
            getObject(arg0).loadSource(getStringFromWasm0(arg1, arg2));
        }, arguments); },
        __wbg_load_fd01b3436024937e: function(arg0) {
            getObject(arg0).load();
        },
        __wbg_location_df7ca06c93e51763: function(arg0) {
            const ret = getObject(arg0).location;
            return addHeapObject(ret);
        },
        __wbg_log_6b5ca2e6124b2808: function(arg0) {
            console.log(getObject(arg0));
        },
        __wbg_matchMedia_91d4fc9729dc3c84: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = getObject(arg0).matchMedia(getStringFromWasm0(arg1, arg2));
            return isLikeNone(ret) ? 0 : addHeapObject(ret);
        }, arguments); },
        __wbg_matches_4b5c22bd830f7bb3: function(arg0) {
            const ret = getObject(arg0).matches;
            return ret;
        },
        __wbg_message_0b2b0298a231b0d4: function(arg0, arg1) {
            const ret = getObject(arg1).message;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_export, wasm.__wbindgen_export2);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_message_9ddc4b9a62a7c379: function(arg0) {
            const ret = getObject(arg0).message;
            return addHeapObject(ret);
        },
        __wbg_msCrypto_d562bbe83e0d4b91: function(arg0) {
            const ret = getObject(arg0).msCrypto;
            return addHeapObject(ret);
        },
        __wbg_name_171cddfde96a29c8: function(arg0, arg1) {
            const ret = getObject(arg1).name;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_export, wasm.__wbindgen_export2);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_name_242753e5110cd756: function(arg0, arg1) {
            const ret = getObject(arg1).name;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_export, wasm.__wbindgen_export2);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_navigator_43be698ba96fc088: function(arg0) {
            const ret = getObject(arg0).navigator;
            return addHeapObject(ret);
        },
        __wbg_newVersion_8c7b02a13111b216: function(arg0, arg1) {
            const ret = getObject(arg1).newVersion;
            getDataViewMemory0().setFloat64(arg0 + 8 * 1, isLikeNone(ret) ? 0 : ret, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, !isLikeNone(ret), true);
        },
        __wbg_new_057993d5b5e07835: function() { return handleError(function (arg0, arg1) {
            const ret = new WebSocket(getStringFromWasm0(arg0, arg1));
            return addHeapObject(ret);
        }, arguments); },
        __wbg_new_361308b2356cecd0: function() {
            const ret = new Object();
            return addHeapObject(ret);
        },
        __wbg_new_3eb36ae241fe6f44: function() {
            const ret = new Array();
            return addHeapObject(ret);
        },
        __wbg_new_64284bd487f9d239: function() { return handleError(function () {
            const ret = new Headers();
            return addHeapObject(ret);
        }, arguments); },
        __wbg_new_6f0524fbfa300c47: function() { return handleError(function () {
            const ret = new MessageChannel();
            return addHeapObject(ret);
        }, arguments); },
        __wbg_new_72b49615380db768: function(arg0, arg1) {
            const ret = new Error(getStringFromWasm0(arg0, arg1));
            return addHeapObject(ret);
        },
        __wbg_new_b5d9e2fb389fef91: function(arg0, arg1) {
            try {
                var state0 = {a: arg0, b: arg1};
                var cb0 = (arg0, arg1) => {
                    const a = state0.a;
                    state0.a = 0;
                    try {
                        return __wasm_bindgen_func_elem_1341(a, state0.b, arg0, arg1);
                    } finally {
                        state0.a = a;
                    }
                };
                const ret = new Promise(cb0);
                return addHeapObject(ret);
            } finally {
                state0.a = state0.b = 0;
            }
        },
        __wbg_new_b949e7f56150a5d1: function() { return handleError(function () {
            const ret = new AbortController();
            return addHeapObject(ret);
        }, arguments); },
        __wbg_new_c2f21774701ddac7: function() { return handleError(function (arg0, arg1) {
            const ret = new URL(getStringFromWasm0(arg0, arg1));
            return addHeapObject(ret);
        }, arguments); },
        __wbg_new_dca287b076112a51: function() {
            const ret = new Map();
            return addHeapObject(ret);
        },
        __wbg_new_dd2b680c8bf6ae29: function(arg0) {
            const ret = new Uint8Array(getObject(arg0));
            return addHeapObject(ret);
        },
        __wbg_new_from_slice_a3d2629dc1826784: function(arg0, arg1) {
            const ret = new Uint8Array(getArrayU8FromWasm0(arg0, arg1));
            return addHeapObject(ret);
        },
        __wbg_new_no_args_1c7c842f08d00ebb: function(arg0, arg1) {
            const ret = new Function(getStringFromWasm0(arg0, arg1));
            return addHeapObject(ret);
        },
        __wbg_new_with_args_7bba34e94b1cfa40: function(arg0, arg1, arg2, arg3) {
            const ret = new Function(getStringFromWasm0(arg0, arg1), getStringFromWasm0(arg2, arg3));
            return addHeapObject(ret);
        },
        __wbg_new_with_base_332baf6f4ebbaae9: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            const ret = new URL(getStringFromWasm0(arg0, arg1), getStringFromWasm0(arg2, arg3));
            return addHeapObject(ret);
        }, arguments); },
        __wbg_new_with_length_a2c39cbe88fd8ff1: function(arg0) {
            const ret = new Uint8Array(arg0 >>> 0);
            return addHeapObject(ret);
        },
        __wbg_new_with_str_and_init_a61cbc6bdef21614: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = new Request(getStringFromWasm0(arg0, arg1), getObject(arg2));
            return addHeapObject(ret);
        }, arguments); },
        __wbg_new_with_u8_array_sequence_and_options_bacb98f8fd0b9307: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            const ret = new File(getObject(arg0), getStringFromWasm0(arg1, arg2), getObject(arg3));
            return addHeapObject(ret);
        }, arguments); },
        __wbg_new_with_u8_array_sequence_and_options_cc0f8f2c1ef62e68: function() { return handleError(function (arg0, arg1) {
            const ret = new Blob(getObject(arg0), getObject(arg1));
            return addHeapObject(ret);
        }, arguments); },
        __wbg_new_with_worker_options_be34d2a1ca5cefd8: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = new SharedWorker(getStringFromWasm0(arg0, arg1), getObject(arg2));
            return addHeapObject(ret);
        }, arguments); },
        __wbg_next_3482f54c49e8af19: function() { return handleError(function (arg0) {
            const ret = getObject(arg0).next();
            return addHeapObject(ret);
        }, arguments); },
        __wbg_next_418f80d8f5303233: function(arg0) {
            const ret = getObject(arg0).next;
            return addHeapObject(ret);
        },
        __wbg_node_e1f24f89a7336c2e: function(arg0) {
            const ret = getObject(arg0).node;
            return addHeapObject(ret);
        },
        __wbg_now_2c95c9de01293173: function(arg0) {
            const ret = getObject(arg0).now();
            return ret;
        },
        __wbg_now_a3af9a2f4bbaa4d1: function() {
            const ret = Date.now();
            return ret;
        },
        __wbg_objectStore_d56e603390dcc165: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = getObject(arg0).objectStore(getStringFromWasm0(arg1, arg2));
            return addHeapObject(ret);
        }, arguments); },
        __wbg_off_48421bfce5c22c3c: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            getObject(arg0).off(getStringFromWasm0(arg1, arg2), getObject(arg3));
        }, arguments); },
        __wbg_oldVersion_97e5f91fffb21425: function(arg0) {
            const ret = getObject(arg0).oldVersion;
            return ret;
        },
        __wbg_on_870d7e7822ccc88c: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            getObject(arg0).on(getStringFromWasm0(arg1, arg2), getObject(arg3));
        }, arguments); },
        __wbg_on_eb7d13044c1fcf88: function(arg0, arg1, arg2, arg3) {
            getObject(arg0).on(getStringFromWasm0(arg1, arg2), getObject(arg3));
        },
        __wbg_open_82db86fd5b087109: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            const ret = getObject(arg0).open(getStringFromWasm0(arg1, arg2), arg3 >>> 0);
            return addHeapObject(ret);
        }, arguments); },
        __wbg_open_d7691c490eaf9349: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6) {
            const ret = getObject(arg0).open(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), getStringFromWasm0(arg5, arg6));
            return isLikeNone(ret) ? 0 : addHeapObject(ret);
        }, arguments); },
        __wbg_origin_a9c891fa602b4d40: function() { return handleError(function (arg0, arg1) {
            const ret = getObject(arg1).origin;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_export, wasm.__wbindgen_export2);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        }, arguments); },
        __wbg_parentElement_75863410a8617953: function(arg0) {
            const ret = getObject(arg0).parentElement;
            return isLikeNone(ret) ? 0 : addHeapObject(ret);
        },
        __wbg_parentNode_d44bd5ec58601e45: function(arg0) {
            const ret = getObject(arg0).parentNode;
            return isLikeNone(ret) ? 0 : addHeapObject(ret);
        },
        __wbg_pathname_b2267fae358b49a7: function() { return handleError(function (arg0, arg1) {
            const ret = getObject(arg1).pathname;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_export, wasm.__wbindgen_export2);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        }, arguments); },
        __wbg_pathname_cd185643c5c70f28: function(arg0, arg1) {
            const ret = getObject(arg1).pathname;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_export, wasm.__wbindgen_export2);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_pause_f98f88bf9fe423be: function() { return handleError(function (arg0) {
            getObject(arg0).pause();
        }, arguments); },
        __wbg_paused_2bb3d50676dbc3ff: function(arg0) {
            const ret = getObject(arg0).paused;
            return ret;
        },
        __wbg_performance_7a3ffd0b17f663ad: function(arg0) {
            const ret = getObject(arg0).performance;
            return addHeapObject(ret);
        },
        __wbg_play_8086e7eec6569b52: function() { return handleError(function (arg0) {
            const ret = getObject(arg0).play();
            return addHeapObject(ret);
        }, arguments); },
        __wbg_port1_6251ddc5cf5c9287: function(arg0) {
            const ret = getObject(arg0).port1;
            return addHeapObject(ret);
        },
        __wbg_port2_b2a294b0ede1e13c: function(arg0) {
            const ret = getObject(arg0).port2;
            return addHeapObject(ret);
        },
        __wbg_port_a2316e0c63927f00: function(arg0) {
            const ret = getObject(arg0).port;
            return addHeapObject(ret);
        },
        __wbg_ports_1869961313e18e87: function(arg0) {
            const ret = getObject(arg0).ports;
            return addHeapObject(ret);
        },
        __wbg_postMessage_46eeeef39934b448: function() { return handleError(function (arg0, arg1) {
            getObject(arg0).postMessage(getObject(arg1));
        }, arguments); },
        __wbg_postMessage_82bcd6222195317e: function() { return handleError(function (arg0, arg1, arg2) {
            getObject(arg0).postMessage(getObject(arg1), getObject(arg2));
        }, arguments); },
        __wbg_postMessage_ff11d4f061a7184c: function() { return handleError(function (arg0, arg1, arg2) {
            getObject(arg0).postMessage(getObject(arg1), getObject(arg2));
        }, arguments); },
        __wbg_prepend_12ed960547d9dfe1: function() { return handleError(function (arg0, arg1) {
            getObject(arg0).prepend(getObject(arg1));
        }, arguments); },
        __wbg_process_3975fd6c72f520aa: function(arg0) {
            const ret = getObject(arg0).process;
            return addHeapObject(ret);
        },
        __wbg_prototypesetcall_bdcdcc5842e4d77d: function(arg0, arg1, arg2) {
            Uint8Array.prototype.set.call(getArrayU8FromWasm0(arg0, arg1), getObject(arg2));
        },
        __wbg_push_8ffdcb2063340ba5: function(arg0, arg1) {
            const ret = getObject(arg0).push(getObject(arg1));
            return ret;
        },
        __wbg_put_b34701a38436f20a: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = getObject(arg0).put(getObject(arg1), getObject(arg2));
            return addHeapObject(ret);
        }, arguments); },
        __wbg_querySelector_1be4292f202b1597: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = getObject(arg0).querySelector(getStringFromWasm0(arg1, arg2));
            return isLikeNone(ret) ? 0 : addHeapObject(ret);
        }, arguments); },
        __wbg_queueMicrotask_0aa0a927f78f5d98: function(arg0) {
            const ret = getObject(arg0).queueMicrotask;
            return addHeapObject(ret);
        },
        __wbg_queueMicrotask_5bb536982f78a56f: function(arg0) {
            queueMicrotask(getObject(arg0));
        },
        __wbg_randomFillSync_f8c153b79f285817: function() { return handleError(function (arg0, arg1) {
            getObject(arg0).randomFillSync(takeObject(arg1));
        }, arguments); },
        __wbg_readyState_16b23bf0e7fa2af3: function(arg0) {
            const ret = getObject(arg0).readyState;
            return (__wbindgen_enum_IdbRequestReadyState.indexOf(ret) + 1 || 3) - 1;
        },
        __wbg_readyState_1bb73ec7b8a54656: function(arg0) {
            const ret = getObject(arg0).readyState;
            return ret;
        },
        __wbg_readyState_ce115112bd7cd7fd: function(arg0) {
            const ret = getObject(arg0).readyState;
            return ret;
        },
        __wbg_ready_7150dfa3536a3a07: function() { return handleError(function (arg0) {
            const ret = getObject(arg0).ready;
            return addHeapObject(ret);
        }, arguments); },
        __wbg_recoverMediaError_7f7495a46789a6b2: function() { return handleError(function (arg0) {
            getObject(arg0).recoverMediaError();
        }, arguments); },
        __wbg_register_0ecfd4a6a9d6c588: function(arg0, arg1, arg2, arg3) {
            const ret = getObject(arg0).register(getStringFromWasm0(arg1, arg2), getObject(arg3));
            return addHeapObject(ret);
        },
        __wbg_reload_c8ca3f3b07f9e534: function() { return handleError(function (arg0) {
            getObject(arg0).reload();
        }, arguments); },
        __wbg_removeAttribute_87259aab06d9f286: function() { return handleError(function (arg0, arg1, arg2) {
            getObject(arg0).removeAttribute(getStringFromWasm0(arg1, arg2));
        }, arguments); },
        __wbg_removeChild_2f0b06213dbc49ca: function() { return handleError(function (arg0, arg1) {
            const ret = getObject(arg0).removeChild(getObject(arg1));
            return addHeapObject(ret);
        }, arguments); },
        __wbg_removeEventListener_e63328781a5b9af9: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            getObject(arg0).removeEventListener(getStringFromWasm0(arg1, arg2), getObject(arg3));
        }, arguments); },
        __wbg_removeListener_83803bf863b12f7e: function(arg0, arg1, arg2, arg3) {
            getObject(arg0).removeListener(getStringFromWasm0(arg1, arg2), getObject(arg3));
        },
        __wbg_remove_31c39325eee968fc: function(arg0) {
            getObject(arg0).remove();
        },
        __wbg_replaceState_aefa111958f68023: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5) {
            getObject(arg0).replaceState(getObject(arg1), getStringFromWasm0(arg2, arg3), arg4 === 0 ? undefined : getStringFromWasm0(arg4, arg5));
        }, arguments); },
        __wbg_request_7a4a4d6acbd1d3e6: function() { return handleError(function (arg0, arg1) {
            const ret = getObject(arg0).request(RequestArguments.__wrap(arg1));
            return addHeapObject(ret);
        }, arguments); },
        __wbg_require_b74f47fc2d022fd6: function() { return handleError(function () {
            const ret = module.require;
            return addHeapObject(ret);
        }, arguments); },
        __wbg_resolve_002c4b7d9d8f6b64: function(arg0) {
            const ret = Promise.resolve(getObject(arg0));
            return addHeapObject(ret);
        },
        __wbg_result_233b2d68aae87a05: function() { return handleError(function (arg0) {
            const ret = getObject(arg0).result;
            return addHeapObject(ret);
        }, arguments); },
        __wbg_revokeObjectURL_ba5712ef5af8bc9a: function() { return handleError(function (arg0, arg1) {
            URL.revokeObjectURL(getStringFromWasm0(arg0, arg1));
        }, arguments); },
        __wbg_scope_51e54d1739222d32: function(arg0, arg1) {
            const ret = getObject(arg1).scope;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_export, wasm.__wbindgen_export2);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_scriptURL_63085596b6e98d0c: function(arg0, arg1) {
            const ret = getObject(arg1).scriptURL;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_export, wasm.__wbindgen_export2);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_searchParams_cc6830288e02fa4f: function(arg0) {
            const ret = getObject(arg0).searchParams;
            return addHeapObject(ret);
        },
        __wbg_send_542f95dea2df7994: function() { return handleError(function (arg0, arg1, arg2) {
            getObject(arg0).send(getArrayU8FromWasm0(arg1, arg2));
        }, arguments); },
        __wbg_serviceWorker_7632c51ed73e8f80: function(arg0) {
            const ret = getObject(arg0).serviceWorker;
            return addHeapObject(ret);
        },
        __wbg_setAttribute_cc8e4c8a2a008508: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4) {
            getObject(arg0).setAttribute(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4));
        }, arguments); },
        __wbg_setInterval_0e3c8fcec0876733: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            const ret = getObject(arg0).setInterval(getObject(arg1), arg2, ...getObject(arg3));
            return ret;
        }, arguments); },
        __wbg_setInterval_b471c2130618eef6: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            const ret = getObject(arg0).setInterval(getObject(arg1), arg2, ...getObject(arg3));
            return ret;
        }, arguments); },
        __wbg_setTimeout_4ec014681668a581: function(arg0, arg1) {
            const ret = setTimeout(getObject(arg0), arg1);
            return addHeapObject(ret);
        },
        __wbg_setTimeout_db2dbaeefb6f39c7: function() { return handleError(function (arg0, arg1) {
            const ret = setTimeout(getObject(arg0), arg1);
            return addHeapObject(ret);
        }, arguments); },
        __wbg_setTimeout_eefe7f4c234b0c6b: function() { return handleError(function (arg0, arg1) {
            const ret = setTimeout(getObject(arg0), arg1);
            return addHeapObject(ret);
        }, arguments); },
        __wbg_setTimeout_eff32631ea138533: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = getObject(arg0).setTimeout(getObject(arg1), arg2);
            return ret;
        }, arguments); },
        __wbg_set_093aa61d3bfc9905: function(arg0, arg1, arg2, arg3, arg4) {
            getObject(arg0).set(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4));
        },
        __wbg_set_1eb0999cf5d27fc8: function(arg0, arg1, arg2) {
            const ret = getObject(arg0).set(getObject(arg1), getObject(arg2));
            return addHeapObject(ret);
        },
        __wbg_set_3f1d0b984ed272ed: function(arg0, arg1, arg2) {
            getObject(arg0)[takeObject(arg1)] = takeObject(arg2);
        },
        __wbg_set_6cb8631f80447a67: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = Reflect.set(getObject(arg0), getObject(arg1), getObject(arg2));
            return ret;
        }, arguments); },
        __wbg_set_auto_increment_5ef604f4f193fa58: function(arg0, arg1) {
            getObject(arg0).autoIncrement = arg1 !== 0;
        },
        __wbg_set_autoplay_a63d23ab32fd1826: function(arg0, arg1) {
            getObject(arg0).autoplay = arg1 !== 0;
        },
        __wbg_set_binaryType_5bbf62e9f705dc1a: function(arg0, arg1) {
            getObject(arg0).binaryType = __wbindgen_enum_BinaryType[arg1];
        },
        __wbg_set_body_9a7e00afe3cfe244: function(arg0, arg1) {
            getObject(arg0).body = getObject(arg1);
        },
        __wbg_set_cache_315a3ed773a41543: function(arg0, arg1) {
            getObject(arg0).cache = __wbindgen_enum_RequestCache[arg1];
        },
        __wbg_set_cc56eefd2dd91957: function(arg0, arg1, arg2) {
            getObject(arg0).set(getArrayU8FromWasm0(arg1, arg2));
        },
        __wbg_set_className_c1d9e7362164af61: function(arg0, arg1, arg2) {
            getObject(arg0).className = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_credentials_c4a58d2e05ef24fb: function(arg0, arg1) {
            getObject(arg0).credentials = __wbindgen_enum_RequestCredentials[arg1];
        },
        __wbg_set_currentTime_d2a641e0a781a824: function(arg0, arg1) {
            getObject(arg0).currentTime = arg1;
        },
        __wbg_set_defaultMuted_0914fbde28650af7: function(arg0, arg1) {
            getObject(arg0).defaultMuted = arg1 !== 0;
        },
        __wbg_set_f43e577aea94465b: function(arg0, arg1, arg2) {
            getObject(arg0)[arg1 >>> 0] = takeObject(arg2);
        },
        __wbg_set_headers_cfc5f4b2c1f20549: function(arg0, arg1) {
            getObject(arg0).headers = getObject(arg1);
        },
        __wbg_set_id_9b8330f661385753: function(arg0, arg1, arg2) {
            getObject(arg0).id = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_innerHTML_edd39677e3460291: function(arg0, arg1, arg2) {
            getObject(arg0).innerHTML = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_last_modified_dd637ca319aed24b: function(arg0, arg1) {
            getObject(arg0).lastModified = arg1;
        },
        __wbg_set_method_c3e20375f5ae7fac: function(arg0, arg1, arg2) {
            getObject(arg0).method = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_mode_b13642c312648202: function(arg0, arg1) {
            getObject(arg0).mode = __wbindgen_enum_RequestMode[arg1];
        },
        __wbg_set_muted_355ed82782e80d54: function(arg0, arg1) {
            getObject(arg0).muted = arg1 !== 0;
        },
        __wbg_set_name_49b9ecba7f58f2e5: function(arg0, arg1, arg2) {
            getObject(arg0).name = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_onabort_5b85743a64489257: function(arg0, arg1) {
            getObject(arg0).onabort = getObject(arg1);
        },
        __wbg_set_onclick_c73aea04bc926cc1: function(arg0, arg1) {
            getObject(arg0).onclick = getObject(arg1);
        },
        __wbg_set_onclose_d382f3e2c2b850eb: function(arg0, arg1) {
            getObject(arg0).onclose = getObject(arg1);
        },
        __wbg_set_oncomplete_76d4a772a6c8cab6: function(arg0, arg1) {
            getObject(arg0).oncomplete = getObject(arg1);
        },
        __wbg_set_onerror_377f18bf4569bf85: function(arg0, arg1) {
            getObject(arg0).onerror = getObject(arg1);
        },
        __wbg_set_onerror_d0db7c6491b9399d: function(arg0, arg1) {
            getObject(arg0).onerror = getObject(arg1);
        },
        __wbg_set_onerror_dc0e606b09e1792f: function(arg0, arg1) {
            getObject(arg0).onerror = getObject(arg1);
        },
        __wbg_set_oninput_8727122fb0d95766: function(arg0, arg1) {
            getObject(arg0).oninput = getObject(arg1);
        },
        __wbg_set_onmessage_0e1ffb1c0d91d2ad: function(arg0, arg1) {
            getObject(arg0).onmessage = getObject(arg1);
        },
        __wbg_set_onmessage_2114aa5f4f53051e: function(arg0, arg1) {
            getObject(arg0).onmessage = getObject(arg1);
        },
        __wbg_set_onopen_b7b52d519d6c0f11: function(arg0, arg1) {
            getObject(arg0).onopen = getObject(arg1);
        },
        __wbg_set_onsuccess_0edec1acb4124784: function(arg0, arg1) {
            getObject(arg0).onsuccess = getObject(arg1);
        },
        __wbg_set_onupgradeneeded_c887b74722b6ce77: function(arg0, arg1) {
            getObject(arg0).onupgradeneeded = getObject(arg1);
        },
        __wbg_set_scope_c18e6f8045aa3843: function(arg0, arg1, arg2) {
            getObject(arg0).scope = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_signal_f2d3f8599248896d: function(arg0, arg1) {
            getObject(arg0).signal = getObject(arg1);
        },
        __wbg_set_src_92bdcfc72902719f: function(arg0, arg1, arg2) {
            getObject(arg0).src = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_textContent_3e87dba095d9cdbc: function(arg0, arg1, arg2) {
            getObject(arg0).textContent = arg1 === 0 ? undefined : getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_type_148de20768639245: function(arg0, arg1, arg2) {
            getObject(arg0).type = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_type_5c3a3a007a54fc83: function(arg0, arg1, arg2) {
            getObject(arg0).type = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_type_e127f26b78d78bfc: function(arg0, arg1) {
            getObject(arg0).type = __wbindgen_enum_WorkerType[arg1];
        },
        __wbg_set_value_62a965e38b22b38c: function(arg0, arg1, arg2) {
            getObject(arg0).value = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_volume_b333c7b50c44390f: function(arg0, arg1) {
            getObject(arg0).volume = arg1;
        },
        __wbg_signal_d1285ecab4ebc5ad: function(arg0) {
            const ret = getObject(arg0).signal;
            return addHeapObject(ret);
        },
        __wbg_size_e05d31cc6049815f: function(arg0) {
            const ret = getObject(arg0).size;
            return ret;
        },
        __wbg_slice_a4d15492574b99a1: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = getObject(arg0).slice(arg1, arg2);
            return addHeapObject(ret);
        }, arguments); },
        __wbg_source_2eae659af2a96fd0: function(arg0) {
            const ret = getObject(arg0).source;
            return isLikeNone(ret) ? 0 : addHeapObject(ret);
        },
        __wbg_startLoad_fd7412d1aeb0bf78: function() { return handleError(function (arg0, arg1) {
            getObject(arg0).startLoad(arg1);
        }, arguments); },
        __wbg_start_057e5648d7201c07: function() { return handleError(function (arg0, arg1) {
            const ret = getObject(arg0).start(arg1 >>> 0);
            return ret;
        }, arguments); },
        __wbg_start_ffb4b426b1e661bd: function(arg0) {
            getObject(arg0).start();
        },
        __wbg_static_accessor_GLOBAL_12837167ad935116: function() {
            const ret = typeof global === 'undefined' ? null : global;
            return isLikeNone(ret) ? 0 : addHeapObject(ret);
        },
        __wbg_static_accessor_GLOBAL_THIS_e628e89ab3b1c95f: function() {
            const ret = typeof globalThis === 'undefined' ? null : globalThis;
            return isLikeNone(ret) ? 0 : addHeapObject(ret);
        },
        __wbg_static_accessor_SELF_a621d3dfbb60d0ce: function() {
            const ret = typeof self === 'undefined' ? null : self;
            return isLikeNone(ret) ? 0 : addHeapObject(ret);
        },
        __wbg_static_accessor_WINDOW_f8727f0cf888e0bd: function() {
            const ret = typeof window === 'undefined' ? null : window;
            return isLikeNone(ret) ? 0 : addHeapObject(ret);
        },
        __wbg_status_89d7e803db911ee7: function(arg0) {
            const ret = getObject(arg0).status;
            return ret;
        },
        __wbg_stopLoad_0087851ff3940d57: function() { return handleError(function (arg0) {
            getObject(arg0).stopLoad();
        }, arguments); },
        __wbg_stringify_8d1cc6ff383e8bae: function() { return handleError(function (arg0) {
            const ret = JSON.stringify(getObject(arg0));
            return addHeapObject(ret);
        }, arguments); },
        __wbg_subarray_a96e1fef17ed23cb: function(arg0, arg1, arg2) {
            const ret = getObject(arg0).subarray(arg1 >>> 0, arg2 >>> 0);
            return addHeapObject(ret);
        },
        __wbg_target_521be630ab05b11e: function(arg0) {
            const ret = getObject(arg0).target;
            return isLikeNone(ret) ? 0 : addHeapObject(ret);
        },
        __wbg_then_0d9fe2c7b1857d32: function(arg0, arg1, arg2) {
            const ret = getObject(arg0).then(getObject(arg1), getObject(arg2));
            return addHeapObject(ret);
        },
        __wbg_then_b9e7b3b5f1a9e1b5: function(arg0, arg1) {
            const ret = getObject(arg0).then(getObject(arg1));
            return addHeapObject(ret);
        },
        __wbg_toString_964ff7fe6eca8362: function(arg0) {
            const ret = getObject(arg0).toString();
            return addHeapObject(ret);
        },
        __wbg_transaction_55ceb96f4b852417: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            const ret = getObject(arg0).transaction(getStringFromWasm0(arg1, arg2), __wbindgen_enum_IdbTransactionMode[arg3]);
            return addHeapObject(ret);
        }, arguments); },
        __wbg_transaction_aca174e659bb0e68: function(arg0) {
            const ret = getObject(arg0).transaction;
            return addHeapObject(ret);
        },
        __wbg_type_4edffca24c42b74d: function(arg0, arg1) {
            const ret = getObject(arg1).type;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_export, wasm.__wbindgen_export2);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_type_e8c7fade6d73451b: function(arg0, arg1) {
            const ret = getObject(arg1).type;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_export, wasm.__wbindgen_export2);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_update_6c2d8c6ecfd90513: function() { return handleError(function (arg0) {
            const ret = getObject(arg0).update();
            return addHeapObject(ret);
        }, arguments); },
        __wbg_url_c484c26b1fbf5126: function(arg0, arg1) {
            const ret = getObject(arg1).url;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_export, wasm.__wbindgen_export2);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_value_0546255b415e96c1: function(arg0) {
            const ret = getObject(arg0).value;
            return addHeapObject(ret);
        },
        __wbg_value_d402dce7dcb16251: function(arg0, arg1) {
            const ret = getObject(arg1).value;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_export, wasm.__wbindgen_export2);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_value_e506a07878790ca0: function(arg0, arg1) {
            const ret = getObject(arg1).value;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_export, wasm.__wbindgen_export2);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_versions_4e31226f5e8dc909: function(arg0) {
            const ret = getObject(arg0).versions;
            return addHeapObject(ret);
        },
        __wbg_waiting_6c1a093c2fa2102b: function(arg0) {
            const ret = getObject(arg0).waiting;
            return isLikeNone(ret) ? 0 : addHeapObject(ret);
        },
        __wbg_warn_f7ae1b2e66ccb930: function(arg0) {
            console.warn(getObject(arg0));
        },
        __wbindgen_cast_0000000000000001: function(arg0, arg1) {
            // Cast intrinsic for `Closure(Closure { dtor_idx: 3, function: Function { arguments: [Externref, Externref], shim_idx: 27, ret: Unit, inner_ret: Some(Unit) }, mutable: true }) -> Externref`.
            const ret = makeMutClosure(arg0, arg1, wasm.__wasm_bindgen_func_elem_561, __wasm_bindgen_func_elem_1341);
            return addHeapObject(ret);
        },
        __wbindgen_cast_0000000000000002: function(arg0, arg1) {
            // Cast intrinsic for `Closure(Closure { dtor_idx: 3, function: Function { arguments: [Externref], shim_idx: 4, ret: Unit, inner_ret: Some(Unit) }, mutable: true }) -> Externref`.
            const ret = makeMutClosure(arg0, arg1, wasm.__wasm_bindgen_func_elem_561, __wasm_bindgen_func_elem_3180);
            return addHeapObject(ret);
        },
        __wbindgen_cast_0000000000000003: function(arg0, arg1) {
            // Cast intrinsic for `Closure(Closure { dtor_idx: 3, function: Function { arguments: [NamedExternref("CloseEvent")], shim_idx: 4, ret: Unit, inner_ret: Some(Unit) }, mutable: true }) -> Externref`.
            const ret = makeMutClosure(arg0, arg1, wasm.__wasm_bindgen_func_elem_561, __wasm_bindgen_func_elem_3180);
            return addHeapObject(ret);
        },
        __wbindgen_cast_0000000000000004: function(arg0, arg1) {
            // Cast intrinsic for `Closure(Closure { dtor_idx: 3, function: Function { arguments: [NamedExternref("Event")], shim_idx: 4, ret: Unit, inner_ret: Some(Unit) }, mutable: true }) -> Externref`.
            const ret = makeMutClosure(arg0, arg1, wasm.__wasm_bindgen_func_elem_561, __wasm_bindgen_func_elem_3180);
            return addHeapObject(ret);
        },
        __wbindgen_cast_0000000000000005: function(arg0, arg1) {
            // Cast intrinsic for `Closure(Closure { dtor_idx: 3, function: Function { arguments: [NamedExternref("IDBVersionChangeEvent")], shim_idx: 29, ret: Result(Unit), inner_ret: Some(Result(Unit)) }, mutable: true }) -> Externref`.
            const ret = makeMutClosure(arg0, arg1, wasm.__wasm_bindgen_func_elem_561, __wasm_bindgen_func_elem_1343);
            return addHeapObject(ret);
        },
        __wbindgen_cast_0000000000000006: function(arg0, arg1) {
            // Cast intrinsic for `Closure(Closure { dtor_idx: 3, function: Function { arguments: [NamedExternref("MessageEvent")], shim_idx: 4, ret: Unit, inner_ret: Some(Unit) }, mutable: true }) -> Externref`.
            const ret = makeMutClosure(arg0, arg1, wasm.__wasm_bindgen_func_elem_561, __wasm_bindgen_func_elem_3180);
            return addHeapObject(ret);
        },
        __wbindgen_cast_0000000000000007: function(arg0, arg1) {
            // Cast intrinsic for `Closure(Closure { dtor_idx: 3, function: Function { arguments: [], shim_idx: 95, ret: Unit, inner_ret: Some(Unit) }, mutable: true }) -> Externref`.
            const ret = makeMutClosure(arg0, arg1, wasm.__wasm_bindgen_func_elem_561, __wasm_bindgen_func_elem_3098);
            return addHeapObject(ret);
        },
        __wbindgen_cast_0000000000000008: function(arg0) {
            // Cast intrinsic for `F64 -> Externref`.
            const ret = arg0;
            return addHeapObject(ret);
        },
        __wbindgen_cast_0000000000000009: function(arg0) {
            // Cast intrinsic for `I64 -> Externref`.
            const ret = arg0;
            return addHeapObject(ret);
        },
        __wbindgen_cast_000000000000000a: function(arg0, arg1) {
            // Cast intrinsic for `Ref(Slice(U8)) -> NamedExternref("Uint8Array")`.
            const ret = getArrayU8FromWasm0(arg0, arg1);
            return addHeapObject(ret);
        },
        __wbindgen_cast_000000000000000b: function(arg0, arg1) {
            // Cast intrinsic for `Ref(String) -> Externref`.
            const ret = getStringFromWasm0(arg0, arg1);
            return addHeapObject(ret);
        },
        __wbindgen_cast_000000000000000c: function(arg0) {
            // Cast intrinsic for `U64 -> Externref`.
            const ret = BigInt.asUintN(64, arg0);
            return addHeapObject(ret);
        },
        __wbindgen_object_clone_ref: function(arg0) {
            const ret = getObject(arg0);
            return addHeapObject(ret);
        },
        __wbindgen_object_drop_ref: function(arg0) {
            takeObject(arg0);
        },
        __wbindgen_object_is_undefined: function(arg0) {
            const ret = getObject(arg0) === undefined;
            return ret;
        },
    };
    return {
        __proto__: null,
        "./weeb_3_bg.js": import0,
    };
}

function __wasm_bindgen_func_elem_3098(arg0, arg1) {
    wasm.__wasm_bindgen_func_elem_3098(arg0, arg1);
}

function __wasm_bindgen_func_elem_3180(arg0, arg1, arg2) {
    wasm.__wasm_bindgen_func_elem_3180(arg0, arg1, addHeapObject(arg2));
}

function __wasm_bindgen_func_elem_1341(arg0, arg1, arg2, arg3) {
    wasm.__wasm_bindgen_func_elem_1341(arg0, arg1, addHeapObject(arg2), addHeapObject(arg3));
}

function __wasm_bindgen_func_elem_1343(arg0, arg1, arg2) {
    try {
        const retptr = wasm.__wbindgen_add_to_stack_pointer(-16);
        wasm.__wasm_bindgen_func_elem_1343(retptr, arg0, arg1, addHeapObject(arg2));
        var r0 = getDataViewMemory0().getInt32(retptr + 4 * 0, true);
        var r1 = getDataViewMemory0().getInt32(retptr + 4 * 1, true);
        if (r1) {
            throw takeObject(r0);
        }
    } finally {
        wasm.__wbindgen_add_to_stack_pointer(16);
    }
}


const __wbindgen_enum_BinaryType = ["blob", "arraybuffer"];


const __wbindgen_enum_IdbRequestReadyState = ["pending", "done"];


const __wbindgen_enum_IdbTransactionMode = ["readonly", "readwrite", "versionchange", "readwriteflush", "cleanup"];


const __wbindgen_enum_RequestCache = ["default", "no-store", "reload", "no-cache", "force-cache", "only-if-cached"];


const __wbindgen_enum_RequestCredentials = ["omit", "same-origin", "include"];


const __wbindgen_enum_RequestMode = ["same-origin", "no-cors", "cors", "navigate"];


const __wbindgen_enum_WorkerType = ["classic", "module"];
const RequestArgumentsFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_requestarguments_free(ptr >>> 0, 1));
const Weeb3No103Finalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_weeb3no103_free(ptr >>> 0, 1));
const Weeb3WorkerRuntimeFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_weeb3workerruntime_free(ptr >>> 0, 1));

function addHeapObject(obj) {
    if (heap_next === heap.length) heap.push(heap.length + 1);
    const idx = heap_next;
    heap_next = heap[idx];

    heap[idx] = obj;
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

function dropObject(idx) {
    if (idx < 132) return;
    heap[idx] = heap_next;
    heap_next = idx;
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

function getObject(idx) { return heap[idx]; }

function handleError(f, args) {
    try {
        return f.apply(this, args);
    } catch (e) {
        wasm.__wbindgen_export3(addHeapObject(e));
    }
}

let heap = new Array(128).fill(undefined);
heap.push(undefined, null, true, false);

let heap_next = heap.length;

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

function takeObject(idx) {
    const ret = getObject(idx);
    dropObject(idx);
    return ret;
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
