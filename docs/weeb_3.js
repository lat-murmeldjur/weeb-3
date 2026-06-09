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
        this.__wbg_ptr = ret;
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

export class Weeb3 {
    static __wrap(ptr) {
        const obj = Object.create(Weeb3.prototype);
        obj.__wbg_ptr = ptr;
        Weeb3Finalization.register(obj, obj.__wbg_ptr, obj);
        return obj;
    }
    __destroy_into_raw() {
        const ptr = this.__wbg_ptr;
        this.__wbg_ptr = 0;
        Weeb3Finalization.unregister(this);
        return ptr;
    }
    free() {
        const ptr = this.__destroy_into_raw();
        wasm.__wbg_weeb3_free(ptr, 0);
    }
    /**
     * @param {string} address
     * @returns {Promise<Uint8Array>}
     */
    acquire(address) {
        const ptr0 = passStringToWasm0(address, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.weeb3_acquire(this.__wbg_ptr, ptr0, len0);
        return ret;
    }
    /**
     * @param {string} owner
     * @param {string} topic
     * @returns {Promise<Uint8Array>}
     */
    acquire_feed(owner, topic) {
        const ptr0 = passStringToWasm0(owner, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passStringToWasm0(topic, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len1 = WASM_VECTOR_LEN;
        const ret = wasm.weeb3_acquire_feed(this.__wbg_ptr, ptr0, len0, ptr1, len1);
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
        const ret = wasm.weeb3_change_bootnode_address(this.__wbg_ptr, ptr0, len0, ptr1, len1, usable_in_protocols);
        return ret;
    }
    /**
     * @returns {Promise<bigint>}
     */
    get_connections() {
        const ret = wasm.weeb3_get_connections(this.__wbg_ptr);
        return ret;
    }
    /**
     * @returns {Promise<string[]>}
     */
    get_current_logs() {
        const ret = wasm.weeb3_get_current_logs(this.__wbg_ptr);
        return ret;
    }
    /**
     * @returns {Promise<bigint>}
     */
    get_network_id() {
        const ret = wasm.weeb3_get_network_id(this.__wbg_ptr);
        return ret;
    }
    /**
     * @returns {Promise<bigint>}
     */
    get_ongoing_connections() {
        const ret = wasm.weeb3_get_ongoing_connections(this.__wbg_ptr);
        return ret;
    }
    /**
     * @param {string} log0
     */
    interface_log(log0) {
        const ptr0 = passStringToWasm0(log0, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        wasm.weeb3_interface_log(this.__wbg_ptr, ptr0, len0);
    }
    /**
     * @param {string} _st
     * @returns {Weeb3}
     */
    static new(_st) {
        const ptr0 = passStringToWasm0(_st, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.weeb3_new(ptr0, len0);
        return Weeb3.__wrap(ret);
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
        const ret = wasm.weeb3_post_push_chunk(this.__wbg_ptr, ptr0, len0, soc, ptr1, len1, ptr2, len2);
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
        const ret = wasm.weeb3_post_upload(this.__wbg_ptr, file, encryption, ptr0, len0, add_to_feed, ptr1, len1);
        return ret;
    }
    /**
     * @returns {Promise<Uint8Array>}
     */
    reset_stamp() {
        const ret = wasm.weeb3_reset_stamp(this.__wbg_ptr);
        return ret;
    }
    /**
     * @param {string} address
     * @returns {Promise<Uint8Array>}
     */
    retrieve_bytes(address) {
        const ptr0 = passStringToWasm0(address, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.weeb3_retrieve_bytes(this.__wbg_ptr, ptr0, len0);
        return ret;
    }
    /**
     * @param {string} address
     * @returns {Promise<Uint8Array>}
     */
    retrieve_chunk_bytes(address) {
        const ptr0 = passStringToWasm0(address, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.weeb3_retrieve_chunk_bytes(this.__wbg_ptr, ptr0, len0);
        return ret;
    }
    /**
     * @param {string} _st
     * @returns {Promise<void>}
     */
    run(_st) {
        const ptr0 = passStringToWasm0(_st, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.weeb3_run(this.__wbg_ptr, ptr0, len0);
        return ret;
    }
    /**
     * @param {string} id
     * @returns {Promise<boolean>}
     */
    set_network_id(id) {
        const ptr0 = passStringToWasm0(id, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.weeb3_set_network_id(this.__wbg_ptr, ptr0, len0);
        return ret;
    }
    /**
     * @returns {Promise<boolean>}
     */
    toggle_transfer_pause() {
        const ret = wasm.weeb3_toggle_transfer_pause(this.__wbg_ptr);
        return ret;
    }
    /**
     * @returns {boolean}
     */
    transfer_paused() {
        const ret = wasm.weeb3_transfer_paused(this.__wbg_ptr);
        return ret !== 0;
    }
}
if (Symbol.dispose) Weeb3.prototype[Symbol.dispose] = Weeb3.prototype.free;

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
    acquireFeed(owner, topic) {
        const ptr0 = passStringToWasm0(owner, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passStringToWasm0(topic, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len1 = WASM_VECTOR_LEN;
        const ret = wasm.weeb3no103_acquireFeed(this.__wbg_ptr, ptr0, len0, ptr1, len1);
        return ret;
    }
    /**
     * @param {string} owner
     * @param {string} topic
     * @returns {Promise<object>}
     */
    acquireFeedBytes(owner, topic) {
        const ptr0 = passStringToWasm0(owner, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passStringToWasm0(topic, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len1 = WASM_VECTOR_LEN;
        const ret = wasm.weeb3no103_acquireFeedBytes(this.__wbg_ptr, ptr0, len0, ptr1, len1);
        return ret;
    }
    /**
     * @param {string} owner
     * @param {string} topic
     * @returns {Promise<object>}
     */
    acquire_feed(owner, topic) {
        const ptr0 = passStringToWasm0(owner, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passStringToWasm0(topic, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len1 = WASM_VECTOR_LEN;
        const ret = wasm.weeb3no103_acquire_feed(this.__wbg_ptr, ptr0, len0, ptr1, len1);
        return ret;
    }
    /**
     * @param {string} owner
     * @param {string} topic
     * @returns {Promise<object>}
     */
    acquire_feed_bytes(owner, topic) {
        const ptr0 = passStringToWasm0(owner, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passStringToWasm0(topic, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len1 = WASM_VECTOR_LEN;
        const ret = wasm.weeb3no103_acquire_feed_bytes(this.__wbg_ptr, ptr0, len0, ptr1, len1);
        return ret;
    }
    /**
     * @param {number} depth
     * @param {number} validity_days
     * @returns {Promise<object>}
     */
    batchState(depth, validity_days) {
        const ret = wasm.weeb3no103_batchState(this.__wbg_ptr, depth, validity_days);
        return ret;
    }
    /**
     * @param {number} depth
     * @param {number} validity_days
     * @returns {Promise<object>}
     */
    batch_state(depth, validity_days) {
        const ret = wasm.weeb3no103_batch_state(this.__wbg_ptr, depth, validity_days);
        return ret;
    }
    /**
     * @param {number} depth
     * @param {number} validity_days
     * @returns {Promise<object>}
     */
    buyBatch(depth, validity_days) {
        const ret = wasm.weeb3no103_buyBatch(this.__wbg_ptr, depth, validity_days);
        return ret;
    }
    /**
     * @param {number} depth
     * @param {number} validity_days
     * @returns {Promise<object>}
     */
    buy_batch(depth, validity_days) {
        const ret = wasm.weeb3no103_buy_batch(this.__wbg_ptr, depth, validity_days);
        return ret;
    }
    /**
     * @returns {Promise<object>}
     */
    connect() {
        const ret = wasm.weeb3no103_connect(this.__wbg_ptr);
        return ret;
    }
    /**
     * @param {string} mode
     * @returns {Promise<object>}
     */
    connectProfile(mode) {
        const ptr0 = passStringToWasm0(mode, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.weeb3no103_connectProfile(this.__wbg_ptr, ptr0, len0);
        return ret;
    }
    /**
     * @param {string} mode
     * @returns {Promise<object>}
     */
    connect_profile(mode) {
        const ptr0 = passStringToWasm0(mode, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.weeb3no103_connect_profile(this.__wbg_ptr, ptr0, len0);
        return ret;
    }
    /**
     * @returns {Promise<object>}
     */
    deployChequebook() {
        const ret = wasm.weeb3no103_deployChequebook(this.__wbg_ptr);
        return ret;
    }
    /**
     * @returns {Promise<object>}
     */
    deploy_chequebook() {
        const ret = wasm.weeb3no103_deploy_chequebook(this.__wbg_ptr);
        return ret;
    }
    /**
     * @param {string} amount
     * @returns {Promise<object>}
     */
    depositChequebook(amount) {
        const ptr0 = passStringToWasm0(amount, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.weeb3no103_depositChequebook(this.__wbg_ptr, ptr0, len0);
        return ret;
    }
    /**
     * @param {string} amount
     * @returns {Promise<object>}
     */
    deposit_chequebook(amount) {
        const ptr0 = passStringToWasm0(amount, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.weeb3no103_deposit_chequebook(this.__wbg_ptr, ptr0, len0);
        return ret;
    }
    /**
     * @param {string} topic
     * @returns {Promise<object>}
     */
    feedIdentity(topic) {
        const ptr0 = passStringToWasm0(topic, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.weeb3no103_feedIdentity(this.__wbg_ptr, ptr0, len0);
        return ret;
    }
    /**
     * @returns {Promise<object>}
     */
    feedOwner() {
        const ret = wasm.weeb3no103_feedOwner(this.__wbg_ptr);
        return ret;
    }
    /**
     * @param {string} topic
     * @returns {object}
     */
    feedTopic(topic) {
        const ptr0 = passStringToWasm0(topic, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.weeb3no103_feedTopic(this.__wbg_ptr, ptr0, len0);
        return ret;
    }
    /**
     * @param {string} topic
     * @returns {Promise<object>}
     */
    feed_identity(topic) {
        const ptr0 = passStringToWasm0(topic, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.weeb3no103_feed_identity(this.__wbg_ptr, ptr0, len0);
        return ret;
    }
    /**
     * @returns {Promise<object>}
     */
    feed_owner() {
        const ret = wasm.weeb3no103_feed_owner(this.__wbg_ptr);
        return ret;
    }
    /**
     * @param {string} topic
     * @returns {object}
     */
    feed_topic(topic) {
        const ptr0 = passStringToWasm0(topic, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.weeb3no103_feed_topic(this.__wbg_ptr, ptr0, len0);
        return ret;
    }
    /**
     * @returns {Promise<Array<any>>}
     */
    logs() {
        const ret = wasm.weeb3no103_logs(this.__wbg_ptr);
        return ret;
    }
    /**
     * @returns {Promise<object>}
     */
    networkState() {
        const ret = wasm.weeb3no103_networkState(this.__wbg_ptr);
        return ret;
    }
    /**
     * @returns {Promise<object>}
     */
    network_state() {
        const ret = wasm.weeb3no103_network_state(this.__wbg_ptr);
        return ret;
    }
    constructor() {
        const ret = wasm.weeb3no103_new();
        this.__wbg_ptr = ret;
        Weeb3No103Finalization.register(this, this.__wbg_ptr, this);
        return this;
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
        const ptr0 = passStringToWasm0(topic, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passArray8ToWasm0(bytes, wasm.__wbindgen_malloc);
        const len1 = WASM_VECTOR_LEN;
        const ptr2 = passStringToWasm0(mime, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len2 = WASM_VECTOR_LEN;
        const ptr3 = passStringToWasm0(filename, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len3 = WASM_VECTOR_LEN;
        const ret = wasm.weeb3no103_postFeedBytes(this.__wbg_ptr, ptr0, len0, ptr1, len1, ptr2, len2, ptr3, len3, encryption);
        return ret;
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
        const ret = wasm.weeb3no103_postPushChunk(this.__wbg_ptr, ptr0, len0, soc, ptr1, len1, ptr2, len2);
        return ret;
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
        const ptr0 = passArray8ToWasm0(bytes, wasm.__wbindgen_malloc);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passStringToWasm0(mime, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len1 = WASM_VECTOR_LEN;
        const ptr2 = passStringToWasm0(filename, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len2 = WASM_VECTOR_LEN;
        const ptr3 = passStringToWasm0(feed_topic, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len3 = WASM_VECTOR_LEN;
        const ret = wasm.weeb3no103_postUploadBytes(this.__wbg_ptr, ptr0, len0, ptr1, len1, ptr2, len2, encryption, add_to_feed, ptr3, len3);
        return ret;
    }
    /**
     * @param {string} topic
     * @param {Uint8Array} bytes
     * @param {string} mime
     * @param {string} filename
     * @param {boolean} encryption
     * @returns {Promise<object>}
     */
    post_feed_bytes(topic, bytes, mime, filename, encryption) {
        const ptr0 = passStringToWasm0(topic, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passArray8ToWasm0(bytes, wasm.__wbindgen_malloc);
        const len1 = WASM_VECTOR_LEN;
        const ptr2 = passStringToWasm0(mime, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len2 = WASM_VECTOR_LEN;
        const ptr3 = passStringToWasm0(filename, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len3 = WASM_VECTOR_LEN;
        const ret = wasm.weeb3no103_post_feed_bytes(this.__wbg_ptr, ptr0, len0, ptr1, len1, ptr2, len2, ptr3, len3, encryption);
        return ret;
    }
    /**
     * @param {Uint8Array} data
     * @param {boolean} soc
     * @param {Uint8Array} chunk_address
     * @param {Uint8Array} stamp
     * @returns {Promise<string>}
     */
    post_push_chunk(data, soc, chunk_address, stamp) {
        const ptr0 = passArray8ToWasm0(data, wasm.__wbindgen_malloc);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passArray8ToWasm0(chunk_address, wasm.__wbindgen_malloc);
        const len1 = WASM_VECTOR_LEN;
        const ptr2 = passArray8ToWasm0(stamp, wasm.__wbindgen_malloc);
        const len2 = WASM_VECTOR_LEN;
        const ret = wasm.weeb3no103_post_push_chunk(this.__wbg_ptr, ptr0, len0, soc, ptr1, len1, ptr2, len2);
        return ret;
    }
    /**
     * @param {File} file
     * @param {boolean} encryption
     * @param {string} index_string
     * @param {boolean} add_to_feed
     * @param {string} feed_topic
     * @returns {Promise<object>}
     */
    post_upload(file, encryption, index_string, add_to_feed, feed_topic) {
        const ptr0 = passStringToWasm0(index_string, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passStringToWasm0(feed_topic, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len1 = WASM_VECTOR_LEN;
        const ret = wasm.weeb3no103_post_upload(this.__wbg_ptr, file, encryption, ptr0, len0, add_to_feed, ptr1, len1);
        return ret;
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
    post_upload_bytes(bytes, mime, filename, encryption, add_to_feed, feed_topic) {
        const ptr0 = passArray8ToWasm0(bytes, wasm.__wbindgen_malloc);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passStringToWasm0(mime, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len1 = WASM_VECTOR_LEN;
        const ptr2 = passStringToWasm0(filename, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len2 = WASM_VECTOR_LEN;
        const ptr3 = passStringToWasm0(feed_topic, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len3 = WASM_VECTOR_LEN;
        const ret = wasm.weeb3no103_post_upload_bytes(this.__wbg_ptr, ptr0, len0, ptr1, len1, ptr2, len2, encryption, add_to_feed, ptr3, len3);
        return ret;
    }
    /**
     * @param {number} seen_revision
     * @returns {Promise<object>}
     */
    progressSnapshot(seen_revision) {
        const ret = wasm.weeb3no103_progressSnapshot(this.__wbg_ptr, seen_revision);
        return ret;
    }
    /**
     * @param {number} seen_revision
     * @returns {Promise<object>}
     */
    progress_snapshot(seen_revision) {
        const ret = wasm.weeb3no103_progress_snapshot(this.__wbg_ptr, seen_revision);
        return ret;
    }
    /**
     * @param {number} min_connections
     * @param {number} timeout_ms
     * @returns {Promise<boolean>}
     */
    ready(min_connections, timeout_ms) {
        const ret = wasm.weeb3no103_ready(this.__wbg_ptr, min_connections, timeout_ms);
        return ret;
    }
    /**
     * @param {number} min_connections
     * @param {number} timeout_ms
     * @returns {Promise<object>}
     */
    readyState(min_connections, timeout_ms) {
        const ret = wasm.weeb3no103_readyState(this.__wbg_ptr, min_connections, timeout_ms);
        return ret;
    }
    /**
     * @param {number} min_connections
     * @param {number} timeout_ms
     * @returns {Promise<object>}
     */
    ready_state(min_connections, timeout_ms) {
        const ret = wasm.weeb3no103_ready_state(this.__wbg_ptr, min_connections, timeout_ms);
        return ret;
    }
    /**
     * @param {Element} container
     * @returns {object}
     */
    renderInterface(container) {
        const ret = wasm.weeb3no103_renderInterface(this.__wbg_ptr, container);
        return ret;
    }
    /**
     * @param {Element} container
     * @returns {object}
     */
    render_interface(container) {
        const ret = wasm.weeb3no103_render_interface(this.__wbg_ptr, container);
        return ret;
    }
    /**
     * @returns {Promise<object>}
     */
    resetStamp() {
        const ret = wasm.weeb3no103_resetStamp(this.__wbg_ptr);
        return ret;
    }
    /**
     * @returns {Promise<object>}
     */
    reset_stamp() {
        const ret = wasm.weeb3no103_reset_stamp(this.__wbg_ptr);
        return ret;
    }
    /**
     * @param {string} address
     * @returns {Promise<Array<any>>}
     */
    retrieve(address) {
        const ptr0 = passStringToWasm0(address, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.weeb3no103_retrieve(this.__wbg_ptr, ptr0, len0);
        return ret;
    }
    /**
     * @param {string} address
     * @returns {Promise<Uint8Array>}
     */
    retrieveBytes(address) {
        const ptr0 = passStringToWasm0(address, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.weeb3no103_retrieveBytes(this.__wbg_ptr, ptr0, len0);
        return ret;
    }
    /**
     * @param {string} address
     * @returns {Promise<Uint8Array>}
     */
    retrieveChunk(address) {
        const ptr0 = passStringToWasm0(address, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.weeb3no103_retrieveChunk(this.__wbg_ptr, ptr0, len0);
        return ret;
    }
    /**
     * @param {string} address
     * @returns {Promise<Uint8Array>}
     */
    retrieve_bytes(address) {
        const ptr0 = passStringToWasm0(address, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.weeb3no103_retrieve_bytes(this.__wbg_ptr, ptr0, len0);
        return ret;
    }
    /**
     * @param {string} address
     * @returns {Promise<Uint8Array>}
     */
    retrieve_chunk(address) {
        const ptr0 = passStringToWasm0(address, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.weeb3no103_retrieve_chunk(this.__wbg_ptr, ptr0, len0);
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
        wasm.weeb3no103_start(this.__wbg_ptr, ptr0, len0, ptr1, len1);
    }
    /**
     * @returns {Promise<object>}
     */
    switchMainnet() {
        const ret = wasm.weeb3no103_switchMainnet(this.__wbg_ptr);
        return ret;
    }
    /**
     * @param {string} mode
     * @returns {Promise<object>}
     */
    switchNetwork(mode) {
        const ptr0 = passStringToWasm0(mode, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.weeb3no103_switchNetwork(this.__wbg_ptr, ptr0, len0);
        return ret;
    }
    /**
     * @returns {Promise<object>}
     */
    switchTestnet() {
        const ret = wasm.weeb3no103_switchTestnet(this.__wbg_ptr);
        return ret;
    }
    /**
     * @returns {Promise<object>}
     */
    switch_mainnet() {
        const ret = wasm.weeb3no103_switch_mainnet(this.__wbg_ptr);
        return ret;
    }
    /**
     * @param {string} mode
     * @returns {Promise<object>}
     */
    switch_network(mode) {
        const ptr0 = passStringToWasm0(mode, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.weeb3no103_switch_network(this.__wbg_ptr, ptr0, len0);
        return ret;
    }
    /**
     * @returns {Promise<object>}
     */
    switch_testnet() {
        const ret = wasm.weeb3no103_switch_testnet(this.__wbg_ptr);
        return ret;
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
        const ret = wasm.weeb3no103_upload(this.__wbg_ptr, file, encryption, ptr0, len0, add_to_feed, ptr1, len1);
        return ret;
    }
    /**
     * @param {number} depth
     * @param {number} validity_days
     * @returns {Promise<object>}
     */
    uploadPrerequisites(depth, validity_days) {
        const ret = wasm.weeb3no103_uploadPrerequisites(this.__wbg_ptr, depth, validity_days);
        return ret;
    }
    /**
     * @param {number} depth
     * @param {number} validity_days
     * @returns {Promise<object>}
     */
    upload_prerequisites(depth, validity_days) {
        const ret = wasm.weeb3no103_upload_prerequisites(this.__wbg_ptr, depth, validity_days);
        return ret;
    }
}
if (Symbol.dispose) Weeb3No103.prototype[Symbol.dispose] = Weeb3No103.prototype.free;

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
        __wbg_Error_9dc85fe1bc224456: function(arg0, arg1) {
            const ret = Error(getStringFromWasm0(arg0, arg1));
            return ret;
        },
        __wbg_Number_4779d427bae39753: function(arg0) {
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
        __wbg___wbindgen_bigint_get_as_i64_8ea6736501f396b6: function(arg0, arg1) {
            const v = arg1;
            const ret = typeof(v) === 'bigint' ? v : undefined;
            getDataViewMemory0().setBigInt64(arg0 + 8 * 1, isLikeNone(ret) ? BigInt(0) : ret, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, !isLikeNone(ret), true);
        },
        __wbg___wbindgen_boolean_get_b131b2f36d6b2f55: function(arg0) {
            const v = arg0;
            const ret = typeof(v) === 'boolean' ? v : undefined;
            return isLikeNone(ret) ? 0xFFFFFF : ret ? 1 : 0;
        },
        __wbg___wbindgen_debug_string_56c147eb1a51f0c4: function(arg0, arg1) {
            const ret = debugString(arg1);
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg___wbindgen_in_ce8569b2fc6f5088: function(arg0, arg1) {
            const ret = arg0 in arg1;
            return ret;
        },
        __wbg___wbindgen_is_bigint_df272c65456269c2: function(arg0) {
            const ret = typeof(arg0) === 'bigint';
            return ret;
        },
        __wbg___wbindgen_is_function_147961669f068cd4: function(arg0) {
            const ret = typeof(arg0) === 'function';
            return ret;
        },
        __wbg___wbindgen_is_null_ced4761460071341: function(arg0) {
            const ret = arg0 === null;
            return ret;
        },
        __wbg___wbindgen_is_object_3a2c414391dbf751: function(arg0) {
            const val = arg0;
            const ret = typeof(val) === 'object' && val !== null;
            return ret;
        },
        __wbg___wbindgen_is_string_6541b0f6ecd4e8e5: function(arg0) {
            const ret = typeof(arg0) === 'string';
            return ret;
        },
        __wbg___wbindgen_is_undefined_4410e3c20a99fa97: function(arg0) {
            const ret = arg0 === undefined;
            return ret;
        },
        __wbg___wbindgen_jsval_eq_174c93ec61bab0c5: function(arg0, arg1) {
            const ret = arg0 === arg1;
            return ret;
        },
        __wbg___wbindgen_jsval_loose_eq_e07e3b1f5db6da6c: function(arg0, arg1) {
            const ret = arg0 == arg1;
            return ret;
        },
        __wbg___wbindgen_number_get_588ed6b97f0d7e14: function(arg0, arg1) {
            const obj = arg1;
            const ret = typeof(obj) === 'number' ? obj : undefined;
            getDataViewMemory0().setFloat64(arg0 + 8 * 1, isLikeNone(ret) ? 0 : ret, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, !isLikeNone(ret), true);
        },
        __wbg___wbindgen_string_get_fa2687d531ed17a5: function(arg0, arg1) {
            const obj = arg1;
            const ret = typeof(obj) === 'string' ? obj : undefined;
            var ptr1 = isLikeNone(ret) ? 0 : passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            var len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg___wbindgen_throw_bbadd78c1bac3a77: function(arg0, arg1) {
            throw new Error(getStringFromWasm0(arg0, arg1));
        },
        __wbg__wbg_cb_unref_c2301a3c9b78104b: function(arg0) {
            arg0._wbg_cb_unref();
        },
        __wbg_abort_05c85b22c6f91af0: function() { return handleError(function (arg0) {
            arg0.abort();
        }, arguments); },
        __wbg_abort_f2416f4a9852003e: function(arg0) {
            arg0.abort();
        },
        __wbg_active_c596fc574389c1a8: function(arg0) {
            const ret = arg0.active;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_addEventListener_767c2a31836a5d6b: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            arg0.addEventListener(getStringFromWasm0(arg1, arg2), arg3);
        }, arguments); },
        __wbg_alert_7d51220e6bde1742: function() { return handleError(function (arg0, arg1, arg2) {
            arg0.alert(getStringFromWasm0(arg1, arg2));
        }, arguments); },
        __wbg_appendChild_5d12592005fe7b6f: function() { return handleError(function (arg0, arg1) {
            const ret = arg0.appendChild(arg1);
            return ret;
        }, arguments); },
        __wbg_append_c5a0c94a6a73b22e: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4) {
            arg0.append(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4));
        }, arguments); },
        __wbg_apply_5d99f956e2dda74d: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.apply(arg1, arg2);
            return ret;
        }, arguments); },
        __wbg_arrayBuffer_2d439f81a051f4c3: function(arg0) {
            const ret = arg0.arrayBuffer();
            return ret;
        },
        __wbg_arrayBuffer_cc7704fa158b907a: function() { return handleError(function (arg0) {
            const ret = arg0.arrayBuffer();
            return ret;
        }, arguments); },
        __wbg_assign_f605b430be1bf858: function() { return handleError(function (arg0, arg1, arg2) {
            arg0.assign(getStringFromWasm0(arg1, arg2));
        }, arguments); },
        __wbg_body_f995b59cdb3c2948: function(arg0) {
            const ret = arg0.body;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_bootstrapnode_unwrap: function(arg0) {
            const ret = BootstrapNode.__unwrap(arg0);
            return ret;
        },
        __wbg_bound_9f83e0951cd441f9: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            const ret = IDBKeyRange.bound(arg0, arg1, arg2 !== 0, arg3 !== 0);
            return ret;
        }, arguments); },
        __wbg_bufferedAmount_738b61dcd5a7a2df: function(arg0) {
            const ret = arg0.bufferedAmount;
            return ret;
        },
        __wbg_call_91f00ddc43e01490: function() { return handleError(function (arg0, arg1) {
            const ret = arg0.call(arg1);
            return ret;
        }, arguments); },
        __wbg_call_ec09a4cf93377d3a: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.call(arg1, arg2);
            return ret;
        }, arguments); },
        __wbg_checked_b34ca25ee3685328: function(arg0) {
            const ret = arg0.checked;
            return ret;
        },
        __wbg_clearInterval_1e9966ed68dbffe4: function(arg0, arg1) {
            arg0.clearInterval(arg1);
        },
        __wbg_clearInterval_8815d80baf6e59a1: function(arg0, arg1) {
            arg0.clearInterval(arg1);
        },
        __wbg_clearTimeout_113b1cde814ec762: function(arg0) {
            const ret = clearTimeout(arg0);
            return ret;
        },
        __wbg_clearTimeout_3629d6209dfcc46e: function(arg0) {
            const ret = clearTimeout(arg0);
            return ret;
        },
        __wbg_click_ba01ed7f845f446f: function(arg0) {
            arg0.click();
        },
        __wbg_close_7a6e6d18f74648be: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            arg0.close(arg1, getStringFromWasm0(arg2, arg3));
        }, arguments); },
        __wbg_commit_e9c1332714c53826: function() { return handleError(function (arg0) {
            arg0.commit();
        }, arguments); },
        __wbg_confirm_254ee0c8b9eb4f43: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.confirm(getStringFromWasm0(arg1, arg2));
            return ret;
        }, arguments); },
        __wbg_createElement_a068abd1135f508c: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.createElement(getStringFromWasm0(arg1, arg2));
            return ret;
        }, arguments); },
        __wbg_createObjectStore_44fc8d540465deda: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            const ret = arg0.createObjectStore(getStringFromWasm0(arg1, arg2), arg3);
            return ret;
        }, arguments); },
        __wbg_createObjectURL_cdc6fc3e90ad7c7c: function() { return handleError(function (arg0, arg1) {
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
        __wbg_data_8a04443c1e5a8cd3: function(arg0) {
            const ret = arg0.data;
            return ret;
        },
        __wbg_decodeURIComponent_43af8996f613fd99: function() { return handleError(function (arg0, arg1) {
            const ret = decodeURIComponent(getStringFromWasm0(arg0, arg1));
            return ret;
        }, arguments); },
        __wbg_document_d55773b5c3ef918f: function(arg0) {
            const ret = arg0.document;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_done_6a8439e544ec6206: function(arg0) {
            const ret = arg0.done;
            return ret;
        },
        __wbg_encodeURIComponent_e0cd09aae30089aa: function(arg0, arg1) {
            const ret = encodeURIComponent(getStringFromWasm0(arg0, arg1));
            return ret;
        },
        __wbg_entries_3fec13888267f28d: function(arg0) {
            const ret = arg0.entries();
            return ret;
        },
        __wbg_entries_5a6a7e7e0df09fe5: function(arg0) {
            const ret = Object.entries(arg0);
            return ret;
        },
        __wbg_error_5d40e0d23958cfcf: function() { return handleError(function (arg0) {
            const ret = arg0.error;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        }, arguments); },
        __wbg_fetch_22858a12fcf93152: function(arg0, arg1) {
            const ret = arg0.fetch(arg1);
            return ret;
        },
        __wbg_fetch_fda7bc27c982b1f3: function(arg0) {
            const ret = fetch(arg0);
            return ret;
        },
        __wbg_files_ba4972282951aedc: function(arg0) {
            const ret = arg0.files;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_focus_3e16748a4a32c2cd: function() { return handleError(function (arg0) {
            arg0.focus();
        }, arguments); },
        __wbg_from_8a57180716c586ee: function(arg0) {
            const ret = Array.from(arg0);
            return ret;
        },
        __wbg_getAttribute_eaf685537a7e8b8b: function(arg0, arg1, arg2, arg3) {
            const ret = arg1.getAttribute(getStringFromWasm0(arg2, arg3));
            var ptr1 = isLikeNone(ret) ? 0 : passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            var len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_getElementById_4d387870cd6176ee: function(arg0, arg1, arg2) {
            const ret = arg0.getElementById(getStringFromWasm0(arg1, arg2));
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_getRandomValues_3f44b700395062e5: function() { return handleError(function (arg0, arg1) {
            globalThis.crypto.getRandomValues(getArrayU8FromWasm0(arg0, arg1));
        }, arguments); },
        __wbg_getRandomValues_c44a50d8cfdaebeb: function() { return handleError(function (arg0, arg1) {
            arg0.getRandomValues(arg1);
        }, arguments); },
        __wbg_getRegistration_b906641ffa2f2b36: function(arg0) {
            const ret = arg0.getRegistration();
            return ret;
        },
        __wbg_get_40f7a583603da443: function(arg0, arg1) {
            const ret = arg0.get(arg1);
            return ret;
        },
        __wbg_get_44e98e27bda25b5b: function() { return handleError(function (arg0, arg1) {
            const ret = Reflect.get(arg0, arg1);
            return ret;
        }, arguments); },
        __wbg_get_4b90d6d8c5deb5d5: function(arg0, arg1) {
            const ret = arg0[arg1 >>> 0];
            return ret;
        },
        __wbg_get_52a8a619f7b88df6: function() { return handleError(function (arg0, arg1) {
            const ret = Reflect.get(arg0, arg1);
            return ret;
        }, arguments); },
        __wbg_get_9d317e2a0ac25f73: function() { return handleError(function (arg0, arg1) {
            const ret = arg0.get(arg1);
            return ret;
        }, arguments); },
        __wbg_get_provider_js_92bb8eb887ed425c: function() { return handleError(function () {
            const ret = get_provider_js();
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        }, arguments); },
        __wbg_get_unchecked_46e778e3cec74b5e: function(arg0, arg1) {
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
        __wbg_hasAttribute_d8d1a85080439fbf: function(arg0, arg1, arg2) {
            const ret = arg0.hasAttribute(getStringFromWasm0(arg1, arg2));
            return ret;
        },
        __wbg_has_14676d88c1340d2c: function() { return handleError(function (arg0, arg1) {
            const ret = Reflect.has(arg0, arg1);
            return ret;
        }, arguments); },
        __wbg_headers_5b19cf40627ae54d: function(arg0) {
            const ret = arg0.headers;
            return ret;
        },
        __wbg_history_825ffd9dcb4e7355: function() { return handleError(function (arg0) {
            const ret = arg0.history;
            return ret;
        }, arguments); },
        __wbg_href_a2b392b1e0fb012d: function() { return handleError(function (arg0, arg1) {
            const ret = arg1.href;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        }, arguments); },
        __wbg_indexedDB_3bc74a693e47b60c: function() { return handleError(function (arg0) {
            const ret = arg0.indexedDB;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        }, arguments); },
        __wbg_indexedDB_a2139150e2ea2a08: function() { return handleError(function (arg0) {
            const ret = arg0.indexedDB;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        }, arguments); },
        __wbg_indexedDB_abd0f8fd741f2ce8: function() { return handleError(function (arg0) {
            const ret = arg0.indexedDB;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        }, arguments); },
        __wbg_instanceof_ArrayBuffer_a581da923203f29f: function(arg0) {
            let result;
            try {
                result = arg0 instanceof ArrayBuffer;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_DomException_11c6fdda80861b83: function(arg0) {
            let result;
            try {
                result = arg0 instanceof DOMException;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Error_cb5ebd65d798655e: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Error;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_EventTarget_30ce6bc254937bf1: function(arg0) {
            let result;
            try {
                result = arg0 instanceof EventTarget;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_File_489db49ec17b41b6: function(arg0) {
            let result;
            try {
                result = arg0 instanceof File;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_HtmlButtonElement_cd3122ecb8b44348: function(arg0) {
            let result;
            try {
                result = arg0 instanceof HTMLButtonElement;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_HtmlElement_7bb4db4855f90e35: function(arg0) {
            let result;
            try {
                result = arg0 instanceof HTMLElement;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_HtmlInputElement_12988598f10298a5: function(arg0) {
            let result;
            try {
                result = arg0 instanceof HTMLInputElement;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_HtmlSelectElement_59255eea1e977271: function(arg0) {
            let result;
            try {
                result = arg0 instanceof HTMLSelectElement;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_HtmlSpanElement_45cd9ad29b768531: function(arg0) {
            let result;
            try {
                result = arg0 instanceof HTMLSpanElement;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_IdbDatabase_afc27b1fab8e9324: function(arg0) {
            let result;
            try {
                result = arg0 instanceof IDBDatabase;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_IdbRequest_91407310880470f7: function(arg0) {
            let result;
            try {
                result = arg0 instanceof IDBRequest;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Map_7f94c740225003e2: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Map;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_MessagePort_c15388ec13cb451b: function(arg0) {
            let result;
            try {
                result = arg0 instanceof MessagePort;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Object_34d30ae022f04c89: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Object;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Promise_aa24ea31000d4ee6: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Promise;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Response_36a65a969abaa7c6: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Response;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_ServiceWorkerContainer_f72b658502486990: function(arg0) {
            let result;
            try {
                result = arg0 instanceof ServiceWorkerContainer;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_ServiceWorkerRegistration_52bdccb38b43583a: function(arg0) {
            let result;
            try {
                result = arg0 instanceof ServiceWorkerRegistration;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Uint8Array_b6fe1ac89eba107e: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Uint8Array;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Window_9e0fe7d3d1ff4342: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Window;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_isArray_139f48e3c057ede8: function(arg0) {
            const ret = Array.isArray(arg0);
            return ret;
        },
        __wbg_isSafeInteger_c22ccb4af2201fe9: function(arg0) {
            const ret = Number.isSafeInteger(arg0);
            return ret;
        },
        __wbg_item_72beb465f8de271c: function(arg0, arg1) {
            const ret = arg0.item(arg1 >>> 0);
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_iterator_9b36cebf3be7b7cd: function() {
            const ret = Symbol.iterator;
            return ret;
        },
        __wbg_length_68a9d5278d084f4f: function(arg0) {
            const ret = arg0.length;
            return ret;
        },
        __wbg_length_fb04d16d7bdf6d4c: function(arg0) {
            const ret = arg0.length;
            return ret;
        },
        __wbg_location_4328ac1e8cc7a9f4: function(arg0) {
            const ret = arg0.location;
            return ret;
        },
        __wbg_location_4504ef5e465bf91b: function(arg0) {
            const ret = arg0.location;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_log_6614a4effdb4e983: function(arg0) {
            console.log(arg0);
        },
        __wbg_lowerBound_d55a63ccc10e595c: function() { return handleError(function (arg0, arg1) {
            const ret = IDBKeyRange.lowerBound(arg0, arg1 !== 0);
            return ret;
        }, arguments); },
        __wbg_matchMedia_c1e53cebdfcbacbe: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.matchMedia(getStringFromWasm0(arg1, arg2));
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        }, arguments); },
        __wbg_matches_530db209566ac149: function(arg0) {
            const ret = arg0.matches;
            return ret;
        },
        __wbg_message_12958526424a887b: function(arg0, arg1) {
            const ret = arg1.message;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_message_5c6ab4dd6c4b34e8: function(arg0) {
            const ret = arg0.message;
            return ret;
        },
        __wbg_msCrypto_bd5a034af96bcba6: function(arg0) {
            const ret = arg0.msCrypto;
            return ret;
        },
        __wbg_name_c408f285c22bffa8: function(arg0, arg1) {
            const ret = arg1.name;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_name_eb273e7492158915: function(arg0, arg1) {
            const ret = arg1.name;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_navigator_e8073f0771c8d619: function(arg0) {
            const ret = arg0.navigator;
            return ret;
        },
        __wbg_newVersion_b4b2cecf9266973c: function(arg0, arg1) {
            const ret = arg1.newVersion;
            getDataViewMemory0().setFloat64(arg0 + 8 * 1, isLikeNone(ret) ? 0 : ret, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, !isLikeNone(ret), true);
        },
        __wbg_new_0b303268aa395a38: function() {
            const ret = new Array();
            return ret;
        },
        __wbg_new_189262ff31890c23: function() { return handleError(function () {
            const ret = new Headers();
            return ret;
        }, arguments); },
        __wbg_new_20b778a4c5c691c3: function() {
            const ret = new Object();
            return ret;
        },
        __wbg_new_24739629d416f982: function() { return handleError(function () {
            const ret = new AbortController();
            return ret;
        }, arguments); },
        __wbg_new_5fae30e6b23db8df: function(arg0, arg1) {
            const ret = new Error(getStringFromWasm0(arg0, arg1));
            return ret;
        },
        __wbg_new_76bfed9f10bfda90: function() { return handleError(function (arg0, arg1) {
            const ret = new URL(getStringFromWasm0(arg0, arg1));
            return ret;
        }, arguments); },
        __wbg_new_883c0db065f06efd: function() {
            const ret = new Map();
            return ret;
        },
        __wbg_new_a3e5112401a82338: function() { return handleError(function (arg0, arg1) {
            const ret = new WebSocket(getStringFromWasm0(arg0, arg1));
            return ret;
        }, arguments); },
        __wbg_new_b06772b280cc6e52: function(arg0) {
            const ret = new Uint8Array(arg0);
            return ret;
        },
        __wbg_new_b3334f9cd9f51d36: function(arg0, arg1) {
            try {
                var state0 = {a: arg0, b: arg1};
                var cb0 = (arg0, arg1) => {
                    const a = state0.a;
                    state0.a = 0;
                    try {
                        return wasm_bindgen__convert__closures_____invoke__hd604c46010222b71(a, state0.b, arg0, arg1);
                    } finally {
                        state0.a = a;
                    }
                };
                const ret = new Promise(cb0);
                return ret;
            } finally {
                state0.a = 0;
            }
        },
        __wbg_new_from_slice_bb2d1778c0b87eb1: function(arg0, arg1) {
            const ret = new Uint8Array(getArrayU8FromWasm0(arg0, arg1));
            return ret;
        },
        __wbg_new_no_args_9d9148621c3ec45c: function(arg0, arg1) {
            const ret = new Function(getStringFromWasm0(arg0, arg1));
            return ret;
        },
        __wbg_new_typed_90c3f6c29ba36d19: function(arg0, arg1) {
            try {
                var state0 = {a: arg0, b: arg1};
                var cb0 = (arg0, arg1) => {
                    const a = state0.a;
                    state0.a = 0;
                    try {
                        return wasm_bindgen__convert__closures_____invoke__hd604c46010222b71(a, state0.b, arg0, arg1);
                    } finally {
                        state0.a = a;
                    }
                };
                const ret = new Promise(cb0);
                return ret;
            } finally {
                state0.a = 0;
            }
        },
        __wbg_new_with_args_f65ccf8b2cbd794c: function(arg0, arg1, arg2, arg3) {
            const ret = new Function(getStringFromWasm0(arg0, arg1), getStringFromWasm0(arg2, arg3));
            return ret;
        },
        __wbg_new_with_length_4b57a7a5dc67221c: function(arg0) {
            const ret = new Uint8Array(arg0 >>> 0);
            return ret;
        },
        __wbg_new_with_str_and_init_df876572184fe639: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = new Request(getStringFromWasm0(arg0, arg1), arg2);
            return ret;
        }, arguments); },
        __wbg_new_with_u8_array_sequence_and_options_8e130e6e30e5a363: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            const ret = new File(arg0, getStringFromWasm0(arg1, arg2), arg3);
            return ret;
        }, arguments); },
        __wbg_new_with_u8_array_sequence_and_options_970ebb474e7aebf3: function() { return handleError(function (arg0, arg1) {
            const ret = new Blob(arg0, arg1);
            return ret;
        }, arguments); },
        __wbg_next_8cb028b6ba50743f: function() { return handleError(function (arg0) {
            const ret = arg0.next();
            return ret;
        }, arguments); },
        __wbg_next_cfd0b146c9538df8: function(arg0) {
            const ret = arg0.next;
            return ret;
        },
        __wbg_node_84ea875411254db1: function(arg0) {
            const ret = arg0.node;
            return ret;
        },
        __wbg_now_bce4dc999095ea77: function() {
            const ret = Date.now();
            return ret;
        },
        __wbg_now_e7c6795a7f81e10f: function(arg0) {
            const ret = arg0.now();
            return ret;
        },
        __wbg_objectStore_b942b046d671625f: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.objectStore(getStringFromWasm0(arg1, arg2));
            return ret;
        }, arguments); },
        __wbg_oldVersion_a95baa391bece10a: function(arg0) {
            const ret = arg0.oldVersion;
            return ret;
        },
        __wbg_on_f4145ffe4fe22bf2: function(arg0, arg1, arg2, arg3) {
            arg0.on(getStringFromWasm0(arg1, arg2), arg3);
        },
        __wbg_open_1e153642dcb130a9: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            const ret = arg0.open(getStringFromWasm0(arg1, arg2), arg3 >>> 0);
            return ret;
        }, arguments); },
        __wbg_open_b8ea505fe0deaad4: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6) {
            const ret = arg0.open(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), getStringFromWasm0(arg5, arg6));
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        }, arguments); },
        __wbg_origin_b5445a524d0b5b7d: function() { return handleError(function (arg0, arg1) {
            const ret = arg1.origin;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        }, arguments); },
        __wbg_parentNode_f7ec4f9a8f8c6c09: function(arg0) {
            const ret = arg0.parentNode;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_pathname_0a37c6c8aa3d8bf8: function(arg0, arg1) {
            const ret = arg1.pathname;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_pathname_1869e01cee77d8c8: function() { return handleError(function (arg0, arg1) {
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
        __wbg_ports_98d54c5bfcaf7dd9: function(arg0) {
            const ret = arg0.ports;
            return ret;
        },
        __wbg_postMessage_baa87c96ec8a2c44: function() { return handleError(function (arg0, arg1) {
            arg0.postMessage(arg1);
        }, arguments); },
        __wbg_prepend_edbfc40fbc66dfed: function() { return handleError(function (arg0, arg1) {
            arg0.prepend(arg1);
        }, arguments); },
        __wbg_process_44c7a14e11e9f69e: function(arg0) {
            const ret = arg0.process;
            return ret;
        },
        __wbg_prototypesetcall_956c7493c68e29b4: function(arg0, arg1, arg2) {
            Uint8Array.prototype.set.call(getArrayU8FromWasm0(arg0, arg1), arg2);
        },
        __wbg_push_ceb8ef046afb2041: function(arg0, arg1) {
            const ret = arg0.push(arg1);
            return ret;
        },
        __wbg_put_74120191a6128e99: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.put(arg1, arg2);
            return ret;
        }, arguments); },
        __wbg_queueMicrotask_4698f900840e3286: function(arg0) {
            queueMicrotask(arg0);
        },
        __wbg_queueMicrotask_477a5533c7100338: function(arg0) {
            const ret = arg0.queueMicrotask;
            return ret;
        },
        __wbg_randomFillSync_6c25eac9869eb53c: function() { return handleError(function (arg0, arg1) {
            arg0.randomFillSync(arg1);
        }, arguments); },
        __wbg_readyState_25243ad6ce649774: function(arg0) {
            const ret = arg0.readyState;
            return ret;
        },
        __wbg_readyState_c2978ea35693d19f: function(arg0) {
            const ret = arg0.readyState;
            return (__wbindgen_enum_IdbRequestReadyState.indexOf(ret) + 1 || 3) - 1;
        },
        __wbg_ready_aadeb2bcce8de92a: function() { return handleError(function (arg0) {
            const ret = arg0.ready;
            return ret;
        }, arguments); },
        __wbg_register_cd07960bbe0c53d5: function(arg0, arg1, arg2) {
            const ret = arg0.register(getStringFromWasm0(arg1, arg2));
            return ret;
        },
        __wbg_removeAttribute_243127a32c2eb7c3: function() { return handleError(function (arg0, arg1, arg2) {
            arg0.removeAttribute(getStringFromWasm0(arg1, arg2));
        }, arguments); },
        __wbg_removeChild_956b75374e5198fe: function() { return handleError(function (arg0, arg1) {
            const ret = arg0.removeChild(arg1);
            return ret;
        }, arguments); },
        __wbg_removeListener_250a33965641c61b: function(arg0, arg1, arg2, arg3) {
            arg0.removeListener(getStringFromWasm0(arg1, arg2), arg3);
        },
        __wbg_remove_af8ebf6457731f08: function(arg0) {
            arg0.remove();
        },
        __wbg_replaceState_a802b0bcbca11655: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5) {
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
        __wbg_resolve_0183de2e8c6b1d54: function(arg0) {
            const ret = Promise.resolve(arg0);
            return ret;
        },
        __wbg_result_7dcb214152e75768: function() { return handleError(function (arg0) {
            const ret = arg0.result;
            return ret;
        }, arguments); },
        __wbg_send_99d049cae69c53cc: function() { return handleError(function (arg0, arg1, arg2) {
            arg0.send(getArrayU8FromWasm0(arg1, arg2));
        }, arguments); },
        __wbg_serviceWorker_fa50ae54bff98ab5: function(arg0) {
            const ret = arg0.serviceWorker;
            return ret;
        },
        __wbg_setAttribute_68c00da2a51113e3: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4) {
            arg0.setAttribute(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4));
        }, arguments); },
        __wbg_setInterval_6b9f03aaababe7e9: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            const ret = arg0.setInterval(arg1, arg2, ...arg3);
            return ret;
        }, arguments); },
        __wbg_setInterval_f79aa0c06e29af04: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            const ret = arg0.setInterval(arg1, arg2, ...arg3);
            return ret;
        }, arguments); },
        __wbg_setTimeout_0fa379e1cf0e30c4: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.setTimeout(arg1, arg2);
            return ret;
        }, arguments); },
        __wbg_setTimeout_56bcdccbad22fd44: function() { return handleError(function (arg0, arg1) {
            const ret = setTimeout(arg0, arg1);
            return ret;
        }, arguments); },
        __wbg_setTimeout_ef24d2fc3ad97385: function() { return handleError(function (arg0, arg1) {
            const ret = setTimeout(arg0, arg1);
            return ret;
        }, arguments); },
        __wbg_set_5f806304fb633ab3: function(arg0, arg1, arg2) {
            const ret = arg0.set(arg1, arg2);
            return ret;
        },
        __wbg_set_6be42768c690e380: function(arg0, arg1, arg2) {
            arg0[arg1] = arg2;
        },
        __wbg_set_86698c227e5b9dad: function(arg0, arg1, arg2) {
            arg0.set(getArrayU8FromWasm0(arg1, arg2));
        },
        __wbg_set_a6ba3ac0e634b822: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = Reflect.set(arg0, arg1, arg2);
            return ret;
        }, arguments); },
        __wbg_set_auto_increment_260559ed1c5bef0a: function(arg0, arg1) {
            arg0.autoIncrement = arg1 !== 0;
        },
        __wbg_set_binaryType_8c2dd2cf1cfc2e28: function(arg0, arg1) {
            arg0.binaryType = __wbindgen_enum_BinaryType[arg1];
        },
        __wbg_set_body_d1153b16a2b95ca1: function(arg0, arg1) {
            arg0.body = arg1;
        },
        __wbg_set_className_81d747d375d02d54: function(arg0, arg1, arg2) {
            arg0.className = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_credentials_e9120ed88b1fc0b8: function(arg0, arg1) {
            arg0.credentials = __wbindgen_enum_RequestCredentials[arg1];
        },
        __wbg_set_da33c120a6584674: function(arg0, arg1, arg2) {
            arg0[arg1 >>> 0] = arg2;
        },
        __wbg_set_headers_877ed41890d28af2: function(arg0, arg1) {
            arg0.headers = arg1;
        },
        __wbg_set_id_6fda818f6a94f87a: function(arg0, arg1, arg2) {
            arg0.id = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_innerHTML_a08278f19a31a4ac: function(arg0, arg1, arg2) {
            arg0.innerHTML = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_last_modified_66ce1123671f14b0: function(arg0, arg1) {
            arg0.lastModified = arg1;
        },
        __wbg_set_method_b4ede299b2064f37: function(arg0, arg1, arg2) {
            arg0.method = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_mode_d6c2e1991aa963c6: function(arg0, arg1) {
            arg0.mode = __wbindgen_enum_RequestMode[arg1];
        },
        __wbg_set_onabort_acfddca1cccaa280: function(arg0, arg1) {
            arg0.onabort = arg1;
        },
        __wbg_set_onclick_860b050dcfba15c7: function(arg0, arg1) {
            arg0.onclick = arg1;
        },
        __wbg_set_onclose_9cb5c3372e16c192: function(arg0, arg1) {
            arg0.onclose = arg1;
        },
        __wbg_set_oncomplete_5bf98eff492b9e40: function(arg0, arg1) {
            arg0.oncomplete = arg1;
        },
        __wbg_set_onerror_190fe67e911ebf28: function(arg0, arg1) {
            arg0.onerror = arg1;
        },
        __wbg_set_onerror_2063a6b52474f1a5: function(arg0, arg1) {
            arg0.onerror = arg1;
        },
        __wbg_set_onerror_f2d816cd689045dd: function(arg0, arg1) {
            arg0.onerror = arg1;
        },
        __wbg_set_oninput_b1681aec5a82e529: function(arg0, arg1) {
            arg0.oninput = arg1;
        },
        __wbg_set_onmessage_96337495f0bfb796: function(arg0, arg1) {
            arg0.onmessage = arg1;
        },
        __wbg_set_onopen_3e2bf6b11d434c2d: function(arg0, arg1) {
            arg0.onopen = arg1;
        },
        __wbg_set_onsuccess_b21f9f9f417b1552: function(arg0, arg1) {
            arg0.onsuccess = arg1;
        },
        __wbg_set_onupgradeneeded_068f2f7f20f19678: function(arg0, arg1) {
            arg0.onupgradeneeded = arg1;
        },
        __wbg_set_signal_9022b5aa06a6ce5b: function(arg0, arg1) {
            arg0.signal = arg1;
        },
        __wbg_set_textContent_17b887047c19ceec: function(arg0, arg1, arg2) {
            arg0.textContent = arg1 === 0 ? undefined : getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_type_38a356c417e6662e: function(arg0, arg1, arg2) {
            arg0.type = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_type_6cd15a1f280fcf3c: function(arg0, arg1, arg2) {
            arg0.type = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_value_767a2044f634988a: function(arg0, arg1, arg2) {
            arg0.value = getStringFromWasm0(arg1, arg2);
        },
        __wbg_signal_41dd1a8913cd0b7d: function(arg0) {
            const ret = arg0.signal;
            return ret;
        },
        __wbg_size_00d8134b0b6e8912: function(arg0) {
            const ret = arg0.size;
            return ret;
        },
        __wbg_slice_1ec808886f99e082: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.slice(arg1, arg2);
            return ret;
        }, arguments); },
        __wbg_static_accessor_GLOBAL_60a4124bab7dcc9a: function() {
            const ret = typeof global === 'undefined' ? null : global;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_static_accessor_GLOBAL_THIS_95ca6460658b5d13: function() {
            const ret = typeof globalThis === 'undefined' ? null : globalThis;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_static_accessor_SELF_4c95f759a91e9aae: function() {
            const ret = typeof self === 'undefined' ? null : self;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_static_accessor_WINDOW_44b435597f9e9ee7: function() {
            const ret = typeof window === 'undefined' ? null : window;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_status_9e6505cfa3e73a7c: function(arg0) {
            const ret = arg0.status;
            return ret;
        },
        __wbg_stringify_a42c95ea9a7591c9: function() { return handleError(function (arg0) {
            const ret = JSON.stringify(arg0);
            return ret;
        }, arguments); },
        __wbg_subarray_42216645a367cd7a: function(arg0, arg1, arg2) {
            const ret = arg0.subarray(arg1 >>> 0, arg2 >>> 0);
            return ret;
        },
        __wbg_target_170ff333c25d067d: function(arg0) {
            const ret = arg0.target;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_then_254bab9b266a77a5: function(arg0, arg1, arg2) {
            const ret = arg0.then(arg1, arg2);
            return ret;
        },
        __wbg_then_3ea18602c6a5123b: function(arg0, arg1) {
            const ret = arg0.then(arg1);
            return ret;
        },
        __wbg_toString_b09619b263823abf: function(arg0) {
            const ret = arg0.toString();
            return ret;
        },
        __wbg_transaction_14d612a203a7a2dc: function(arg0) {
            const ret = arg0.transaction;
            return ret;
        },
        __wbg_transaction_47924f7fc563e5be: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            const ret = arg0.transaction(getStringFromWasm0(arg1, arg2), __wbindgen_enum_IdbTransactionMode[arg3]);
            return ret;
        }, arguments); },
        __wbg_type_4dfb6fe71ed874b9: function(arg0, arg1) {
            const ret = arg1.type;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_update_bbc013a276538713: function() { return handleError(function (arg0) {
            const ret = arg0.update();
            return ret;
        }, arguments); },
        __wbg_upperBound_6d7777400edecb42: function() { return handleError(function (arg0, arg1) {
            const ret = IDBKeyRange.upperBound(arg0, arg1 !== 0);
            return ret;
        }, arguments); },
        __wbg_url_72c67c175685a054: function(arg0, arg1) {
            const ret = arg1.url;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_value_35609f6157664e7e: function(arg0, arg1) {
            const ret = arg1.value;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_value_3d3defe09fb1ffca: function(arg0) {
            const ret = arg0.value;
            return ret;
        },
        __wbg_value_7cba69015031eeff: function(arg0, arg1) {
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
        __wbg_warn_633bacc13ba7e6c3: function(arg0) {
            console.warn(arg0);
        },
        __wbindgen_cast_0000000000000001: function(arg0, arg1) {
            // Cast intrinsic for `Closure(Closure { owned: true, function: Function { arguments: [Externref], shim_idx: 1512, ret: Unit, inner_ret: Some(Unit) }, mutable: true }) -> Externref`.
            const ret = makeMutClosure(arg0, arg1, wasm_bindgen__convert__closures_____invoke__hf4b30bec4501eaeb);
            return ret;
        },
        __wbindgen_cast_0000000000000002: function(arg0, arg1) {
            // Cast intrinsic for `Closure(Closure { owned: true, function: Function { arguments: [Externref], shim_idx: 2652, ret: Result(Unit), inner_ret: Some(Result(Unit)) }, mutable: true }) -> Externref`.
            const ret = makeMutClosure(arg0, arg1, wasm_bindgen__convert__closures_____invoke__h2f0369c88ad10f2e);
            return ret;
        },
        __wbindgen_cast_0000000000000003: function(arg0, arg1) {
            // Cast intrinsic for `Closure(Closure { owned: true, function: Function { arguments: [NamedExternref("CloseEvent")], shim_idx: 1684, ret: Unit, inner_ret: Some(Unit) }, mutable: true }) -> Externref`.
            const ret = makeMutClosure(arg0, arg1, wasm_bindgen__convert__closures_____invoke__h0039b5b45ed3f748);
            return ret;
        },
        __wbindgen_cast_0000000000000004: function(arg0, arg1) {
            // Cast intrinsic for `Closure(Closure { owned: true, function: Function { arguments: [NamedExternref("Event")], shim_idx: 1301, ret: Unit, inner_ret: Some(Unit) }, mutable: true }) -> Externref`.
            const ret = makeMutClosure(arg0, arg1, wasm_bindgen__convert__closures_____invoke__hd7a0b59efaba0f6b);
            return ret;
        },
        __wbindgen_cast_0000000000000005: function(arg0, arg1) {
            // Cast intrinsic for `Closure(Closure { owned: true, function: Function { arguments: [NamedExternref("Event")], shim_idx: 1684, ret: Unit, inner_ret: Some(Unit) }, mutable: true }) -> Externref`.
            const ret = makeMutClosure(arg0, arg1, wasm_bindgen__convert__closures_____invoke__h0039b5b45ed3f748_4);
            return ret;
        },
        __wbindgen_cast_0000000000000006: function(arg0, arg1) {
            // Cast intrinsic for `Closure(Closure { owned: true, function: Function { arguments: [NamedExternref("IDBVersionChangeEvent")], shim_idx: 1274, ret: Result(Unit), inner_ret: Some(Result(Unit)) }, mutable: true }) -> Externref`.
            const ret = makeMutClosure(arg0, arg1, wasm_bindgen__convert__closures_____invoke__h66557adddda5cfae);
            return ret;
        },
        __wbindgen_cast_0000000000000007: function(arg0, arg1) {
            // Cast intrinsic for `Closure(Closure { owned: true, function: Function { arguments: [NamedExternref("MessageEvent")], shim_idx: 1684, ret: Unit, inner_ret: Some(Unit) }, mutable: true }) -> Externref`.
            const ret = makeMutClosure(arg0, arg1, wasm_bindgen__convert__closures_____invoke__h0039b5b45ed3f748_6);
            return ret;
        },
        __wbindgen_cast_0000000000000008: function(arg0, arg1) {
            // Cast intrinsic for `Closure(Closure { owned: true, function: Function { arguments: [], shim_idx: 1303, ret: Unit, inner_ret: Some(Unit) }, mutable: true }) -> Externref`.
            const ret = makeMutClosure(arg0, arg1, wasm_bindgen__convert__closures_____invoke__h2cd9441679fbd0c1);
            return ret;
        },
        __wbindgen_cast_0000000000000009: function(arg0, arg1) {
            // Cast intrinsic for `Closure(Closure { owned: true, function: Function { arguments: [], shim_idx: 2225, ret: Unit, inner_ret: Some(Unit) }, mutable: true }) -> Externref`.
            const ret = makeMutClosure(arg0, arg1, wasm_bindgen__convert__closures_____invoke__h6d93bd64a20bf1b3);
            return ret;
        },
        __wbindgen_cast_000000000000000a: function(arg0, arg1) {
            // Cast intrinsic for `Closure(Closure { owned: true, function: Function { arguments: [], shim_idx: 2631, ret: Unit, inner_ret: Some(Unit) }, mutable: true }) -> Externref`.
            const ret = makeMutClosure(arg0, arg1, wasm_bindgen__convert__closures_____invoke__hff87227ccf1c98a6);
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

function wasm_bindgen__convert__closures_____invoke__h2cd9441679fbd0c1(arg0, arg1) {
    wasm.wasm_bindgen__convert__closures_____invoke__h2cd9441679fbd0c1(arg0, arg1);
}

function wasm_bindgen__convert__closures_____invoke__h6d93bd64a20bf1b3(arg0, arg1) {
    wasm.wasm_bindgen__convert__closures_____invoke__h6d93bd64a20bf1b3(arg0, arg1);
}

function wasm_bindgen__convert__closures_____invoke__hff87227ccf1c98a6(arg0, arg1) {
    wasm.wasm_bindgen__convert__closures_____invoke__hff87227ccf1c98a6(arg0, arg1);
}

function wasm_bindgen__convert__closures_____invoke__hf4b30bec4501eaeb(arg0, arg1, arg2) {
    wasm.wasm_bindgen__convert__closures_____invoke__hf4b30bec4501eaeb(arg0, arg1, arg2);
}

function wasm_bindgen__convert__closures_____invoke__h0039b5b45ed3f748(arg0, arg1, arg2) {
    wasm.wasm_bindgen__convert__closures_____invoke__h0039b5b45ed3f748(arg0, arg1, arg2);
}

function wasm_bindgen__convert__closures_____invoke__hd7a0b59efaba0f6b(arg0, arg1, arg2) {
    wasm.wasm_bindgen__convert__closures_____invoke__hd7a0b59efaba0f6b(arg0, arg1, arg2);
}

function wasm_bindgen__convert__closures_____invoke__h0039b5b45ed3f748_4(arg0, arg1, arg2) {
    wasm.wasm_bindgen__convert__closures_____invoke__h0039b5b45ed3f748_4(arg0, arg1, arg2);
}

function wasm_bindgen__convert__closures_____invoke__h0039b5b45ed3f748_6(arg0, arg1, arg2) {
    wasm.wasm_bindgen__convert__closures_____invoke__h0039b5b45ed3f748_6(arg0, arg1, arg2);
}

function wasm_bindgen__convert__closures_____invoke__h2f0369c88ad10f2e(arg0, arg1, arg2) {
    const ret = wasm.wasm_bindgen__convert__closures_____invoke__h2f0369c88ad10f2e(arg0, arg1, arg2);
    if (ret[1]) {
        throw takeFromExternrefTable0(ret[0]);
    }
}

function wasm_bindgen__convert__closures_____invoke__h66557adddda5cfae(arg0, arg1, arg2) {
    const ret = wasm.wasm_bindgen__convert__closures_____invoke__h66557adddda5cfae(arg0, arg1, arg2);
    if (ret[1]) {
        throw takeFromExternrefTable0(ret[0]);
    }
}

function wasm_bindgen__convert__closures_____invoke__hd604c46010222b71(arg0, arg1, arg2, arg3) {
    wasm.wasm_bindgen__convert__closures_____invoke__hd604c46010222b71(arg0, arg1, arg2, arg3);
}


const __wbindgen_enum_BinaryType = ["blob", "arraybuffer"];


const __wbindgen_enum_IdbRequestReadyState = ["pending", "done"];


const __wbindgen_enum_IdbTransactionMode = ["readonly", "readwrite", "versionchange", "readwriteflush", "cleanup"];


const __wbindgen_enum_RequestCredentials = ["omit", "same-origin", "include"];


const __wbindgen_enum_RequestMode = ["same-origin", "no-cors", "cors", "navigate"];
const BootstrapNodeFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_bootstrapnode_free(ptr, 1));
const RequestArgumentsFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_requestarguments_free(ptr, 1));
const Weeb3Finalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_weeb3_free(ptr, 1));
const Weeb3No103Finalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_weeb3no103_free(ptr, 1));
const WingsFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_wings_free(ptr, 1));

function addToExternrefTable0(obj) {
    const idx = wasm.__externref_table_alloc();
    wasm.__wbindgen_externrefs.set(idx, obj);
    return idx;
}

const CLOSURE_DTORS = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(state => wasm.__wbindgen_destroy_closure(state.a, state.b));

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
    return decodeText(ptr >>> 0, len);
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

function makeMutClosure(arg0, arg1, f) {
    const state = { a: arg0, b: arg1, cnt: 1 };
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
            wasm.__wbindgen_destroy_closure(state.a, state.b);
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

let wasmModule, wasmInstance, wasm;
function __wbg_finalize_init(instance, module) {
    wasmInstance = instance;
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
