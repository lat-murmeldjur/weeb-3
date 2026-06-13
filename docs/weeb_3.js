/* @ts-self-types="./weeb_3.d.ts" */
import { get_provider_js } from './snippets/web3-0742d85b024bb6f5/inline0.js';


export class BootstrapNode {
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
     * @returns {object}
     */
    openSecureVault() {
        const ret = wasm.weeb3no103_openSecureVault(this.__wbg_ptr);
        return ret;
    }
    /**
     * @returns {object}
     */
    open_secure_vault() {
        const ret = wasm.weeb3no103_open_secure_vault(this.__wbg_ptr);
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
     * @param {any | null} [options]
     */
    start(options) {
        wasm.weeb3no103_start(this.__wbg_ptr, isLikeNone(options) ? 0 : addToExternrefTable0(options));
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

export function init_wasm_runtime() {
    wasm.init_wasm_runtime();
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
function __wbg_get_imports() {
    const import0 = {
        __proto__: null,
        __wbg_Error_fdd633d4bb5dd76a: function(arg0, arg1) {
            const ret = Error(getStringFromWasm0(arg0, arg1));
            return ret;
        },
        __wbg_Number_c4bdf66bb78f7977: function(arg0) {
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
        __wbg___wbindgen_bigint_get_as_i64_d9e915702856f831: function(arg0, arg1) {
            const v = arg1;
            const ret = typeof(v) === 'bigint' ? v : undefined;
            getDataViewMemory0().setBigInt64(arg0 + 8 * 1, isLikeNone(ret) ? BigInt(0) : ret, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, !isLikeNone(ret), true);
        },
        __wbg___wbindgen_boolean_get_edaed31a367ce1bd: function(arg0) {
            const v = arg0;
            const ret = typeof(v) === 'boolean' ? v : undefined;
            return isLikeNone(ret) ? 0xFFFFFF : ret ? 1 : 0;
        },
        __wbg___wbindgen_debug_string_8a447059637473e2: function(arg0, arg1) {
            const ret = debugString(arg1);
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg___wbindgen_in_4990f46af709e33c: function(arg0, arg1) {
            const ret = arg0 in arg1;
            return ret;
        },
        __wbg___wbindgen_is_bigint_90b5ccfe67c78460: function(arg0) {
            const ret = typeof(arg0) === 'bigint';
            return ret;
        },
        __wbg___wbindgen_is_function_acc5528be2b923f2: function(arg0) {
            const ret = typeof(arg0) === 'function';
            return ret;
        },
        __wbg___wbindgen_is_null_6d937fbfb6478470: function(arg0) {
            const ret = arg0 === null;
            return ret;
        },
        __wbg___wbindgen_is_object_0beba4a1980d3eea: function(arg0) {
            const val = arg0;
            const ret = typeof(val) === 'object' && val !== null;
            return ret;
        },
        __wbg___wbindgen_is_string_1fca8072260dd261: function(arg0) {
            const ret = typeof(arg0) === 'string';
            return ret;
        },
        __wbg___wbindgen_is_undefined_721f8decd50c87a3: function(arg0) {
            const ret = arg0 === undefined;
            return ret;
        },
        __wbg___wbindgen_jsval_eq_4e8c38722cb8ff51: function(arg0, arg1) {
            const ret = arg0 === arg1;
            return ret;
        },
        __wbg___wbindgen_jsval_loose_eq_4b9aba9e5b3c4582: function(arg0, arg1) {
            const ret = arg0 == arg1;
            return ret;
        },
        __wbg___wbindgen_number_get_1cc01dd708740256: function(arg0, arg1) {
            const obj = arg1;
            const ret = typeof(obj) === 'number' ? obj : undefined;
            getDataViewMemory0().setFloat64(arg0 + 8 * 1, isLikeNone(ret) ? 0 : ret, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, !isLikeNone(ret), true);
        },
        __wbg___wbindgen_string_get_71bb4348194e31f0: function(arg0, arg1) {
            const obj = arg1;
            const ret = typeof(obj) === 'string' ? obj : undefined;
            var ptr1 = isLikeNone(ret) ? 0 : passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            var len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg___wbindgen_throw_ea4887a5f8f9a9db: function(arg0, arg1) {
            throw new Error(getStringFromWasm0(arg0, arg1));
        },
        __wbg__wbg_cb_unref_33c39e13d73b25f6: function(arg0) {
            arg0._wbg_cb_unref();
        },
        __wbg_abort_6e6ea7d259504afc: function(arg0) {
            arg0.abort();
        },
        __wbg_abort_d5eabf44c636be6c: function() { return handleError(function (arg0) {
            arg0.abort();
        }, arguments); },
        __wbg_active_ed291bd256d703d3: function(arg0) {
            const ret = arg0.active;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_addEventListener_ea90bc131475777e: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            arg0.addEventListener(getStringFromWasm0(arg1, arg2), arg3);
        }, arguments); },
        __wbg_alert_0ae34973c332d676: function() { return handleError(function (arg0, arg1, arg2) {
            arg0.alert(getStringFromWasm0(arg1, arg2));
        }, arguments); },
        __wbg_appendChild_acb7691406591783: function() { return handleError(function (arg0, arg1) {
            const ret = arg0.appendChild(arg1);
            return ret;
        }, arguments); },
        __wbg_append_912a8705e9b6a483: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4) {
            arg0.append(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4));
        }, arguments); },
        __wbg_apply_b37ddce350829e95: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.apply(arg1, arg2);
            return ret;
        }, arguments); },
        __wbg_arrayBuffer_e3174a1300c67c95: function(arg0) {
            const ret = arg0.arrayBuffer();
            return ret;
        },
        __wbg_arrayBuffer_ff96d08b7b6be32e: function() { return handleError(function (arg0) {
            const ret = arg0.arrayBuffer();
            return ret;
        }, arguments); },
        __wbg_assign_d8ffd388013c98fb: function() { return handleError(function (arg0, arg1, arg2) {
            arg0.assign(getStringFromWasm0(arg1, arg2));
        }, arguments); },
        __wbg_body_c94f9310613c153b: function(arg0) {
            const ret = arg0.body;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_bound_494804635962f0ea: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            const ret = IDBKeyRange.bound(arg0, arg1, arg2 !== 0, arg3 !== 0);
            return ret;
        }, arguments); },
        __wbg_bufferedAmount_fbe2be0178ce5841: function(arg0) {
            const ret = arg0.bufferedAmount;
            return ret;
        },
        __wbg_call_5575218572ead796: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.call(arg1, arg2);
            return ret;
        }, arguments); },
        __wbg_call_8e98ed2f3c86c4b5: function() { return handleError(function (arg0, arg1) {
            const ret = arg0.call(arg1);
            return ret;
        }, arguments); },
        __wbg_checked_55b8214328709363: function(arg0) {
            const ret = arg0.checked;
            return ret;
        },
        __wbg_clearInterval_8e40ba4cb97095f8: function(arg0, arg1) {
            arg0.clearInterval(arg1);
        },
        __wbg_clearInterval_b44ecb2eaa6c745e: function(arg0, arg1) {
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
        __wbg_click_7041a732c15425d9: function(arg0) {
            arg0.click();
        },
        __wbg_close_2821c17f5a9246a9: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            arg0.close(arg1, getStringFromWasm0(arg2, arg3));
        }, arguments); },
        __wbg_commit_e9c1332714c53826: function() { return handleError(function (arg0) {
            arg0.commit();
        }, arguments); },
        __wbg_confirm_5dfe89a6fe29c068: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.confirm(getStringFromWasm0(arg1, arg2));
            return ret;
        }, arguments); },
        __wbg_createElement_9e23ac95e40e302c: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.createElement(getStringFromWasm0(arg1, arg2));
            return ret;
        }, arguments); },
        __wbg_createObjectStore_8b41c8907c72da4a: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            const ret = arg0.createObjectStore(getStringFromWasm0(arg1, arg2), arg3);
            return ret;
        }, arguments); },
        __wbg_createObjectURL_d8fd80787743974d: function() { return handleError(function (arg0, arg1) {
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
        __wbg_data_4a7f1308dbd33a21: function(arg0) {
            const ret = arg0.data;
            return ret;
        },
        __wbg_decodeURIComponent_196a0d1d33a1893b: function() { return handleError(function (arg0, arg1) {
            const ret = decodeURIComponent(getStringFromWasm0(arg0, arg1));
            return ret;
        }, arguments); },
        __wbg_document_2634180a4c694068: function(arg0) {
            const ret = arg0.document;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_done_b62d4a7d2286852a: function(arg0) {
            const ret = arg0.done;
            return ret;
        },
        __wbg_encodeURIComponent_963d3e9b36ef7fe1: function(arg0, arg1) {
            const ret = encodeURIComponent(getStringFromWasm0(arg0, arg1));
            return ret;
        },
        __wbg_entries_34349b1f852046f9: function(arg0) {
            const ret = arg0.entries();
            return ret;
        },
        __wbg_entries_c261c3fa1f281256: function(arg0) {
            const ret = Object.entries(arg0);
            return ret;
        },
        __wbg_error_2730901eef46e484: function() { return handleError(function (arg0) {
            const ret = arg0.error;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        }, arguments); },
        __wbg_error_a6fa202b58aa1cd3: function(arg0, arg1) {
            let deferred0_0;
            let deferred0_1;
            try {
                deferred0_0 = arg0;
                deferred0_1 = arg1;
                console.error(getStringFromWasm0(arg0, arg1));
            } finally {
                wasm.__wbindgen_free(deferred0_0, deferred0_1, 1);
            }
        },
        __wbg_fetch_db87be8a748781a2: function(arg0, arg1) {
            const ret = arg0.fetch(arg1);
            return ret;
        },
        __wbg_fetch_fda7bc27c982b1f3: function(arg0) {
            const ret = fetch(arg0);
            return ret;
        },
        __wbg_files_e8965ead04936623: function(arg0) {
            const ret = arg0.files;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_focus_5c78aa4851b43915: function() { return handleError(function (arg0) {
            arg0.focus();
        }, arguments); },
        __wbg_from_50138b2ca136f50c: function(arg0) {
            const ret = Array.from(arg0);
            return ret;
        },
        __wbg_getAttribute_9f23675ef6df0d6b: function(arg0, arg1, arg2, arg3) {
            const ret = arg1.getAttribute(getStringFromWasm0(arg2, arg3));
            var ptr1 = isLikeNone(ret) ? 0 : passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            var len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_getElementById_c7aba6b93b34bf01: function(arg0, arg1, arg2) {
            const ret = arg0.getElementById(getStringFromWasm0(arg1, arg2));
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_getRandomValues_3f44b700395062e5: function() { return handleError(function (arg0, arg1) {
            globalThis.crypto.getRandomValues(getArrayU8FromWasm0(arg0, arg1));
        }, arguments); },
        __wbg_getRandomValues_c44a50d8cfdaebeb: function() { return handleError(function (arg0, arg1) {
            arg0.getRandomValues(arg1);
        }, arguments); },
        __wbg_getRegistration_ea0a2dbf26577713: function(arg0) {
            const ret = arg0.getRegistration();
            return ret;
        },
        __wbg_get_197a3fe98f169e38: function(arg0, arg1) {
            const ret = arg0[arg1 >>> 0];
            return ret;
        },
        __wbg_get_9a29be2cb383ed9a: function() { return handleError(function (arg0, arg1) {
            const ret = Reflect.get(arg0, arg1);
            return ret;
        }, arguments); },
        __wbg_get_cc81af70c9fceaea: function() { return handleError(function (arg0, arg1) {
            const ret = arg0.get(arg1);
            return ret;
        }, arguments); },
        __wbg_get_dddb90ff5d27a080: function() { return handleError(function (arg0, arg1) {
            const ret = Reflect.get(arg0, arg1);
            return ret;
        }, arguments); },
        __wbg_get_f6f94ffcaf26eed1: function(arg0, arg1) {
            const ret = arg0.get(arg1);
            return ret;
        },
        __wbg_get_provider_js_92bb8eb887ed425c: function() { return handleError(function () {
            const ret = get_provider_js();
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        }, arguments); },
        __wbg_get_unchecked_54a4374c38e08460: function(arg0, arg1) {
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
        __wbg_hasAttribute_73e1230c9c107e79: function(arg0, arg1, arg2) {
            const ret = arg0.hasAttribute(getStringFromWasm0(arg1, arg2));
            return ret;
        },
        __wbg_has_4f060fe202ad7e87: function() { return handleError(function (arg0, arg1) {
            const ret = Reflect.has(arg0, arg1);
            return ret;
        }, arguments); },
        __wbg_headers_d9123c649c85d441: function(arg0) {
            const ret = arg0.headers;
            return ret;
        },
        __wbg_history_9b3f6609f67a02ee: function() { return handleError(function (arg0) {
            const ret = arg0.history;
            return ret;
        }, arguments); },
        __wbg_href_9829630092aecfff: function() { return handleError(function (arg0, arg1) {
            const ret = arg1.href;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        }, arguments); },
        __wbg_indexedDB_24a7a59b7e7d76f2: function() { return handleError(function (arg0) {
            const ret = arg0.indexedDB;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        }, arguments); },
        __wbg_indexedDB_6d53c9cf3d668233: function() { return handleError(function (arg0) {
            const ret = arg0.indexedDB;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        }, arguments); },
        __wbg_indexedDB_a2139150e2ea2a08: function() { return handleError(function (arg0) {
            const ret = arg0.indexedDB;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        }, arguments); },
        __wbg_instanceof_ArrayBuffer_2a7bb09fee70c2da: function(arg0) {
            let result;
            try {
                result = arg0 instanceof ArrayBuffer;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_DomException_7e2ce66bfb012d37: function(arg0) {
            let result;
            try {
                result = arg0 instanceof DOMException;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Error_77d0cf0b4f31a32f: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Error;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_EventTarget_72098ffe4ac5efa2: function(arg0) {
            let result;
            try {
                result = arg0 instanceof EventTarget;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_File_2d5bf7d3a7b931e9: function(arg0) {
            let result;
            try {
                result = arg0 instanceof File;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_HtmlButtonElement_ed9bdefa1c407160: function(arg0) {
            let result;
            try {
                result = arg0 instanceof HTMLButtonElement;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_HtmlElement_c91c44c5fc58a478: function(arg0) {
            let result;
            try {
                result = arg0 instanceof HTMLElement;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_HtmlInputElement_5a4ec7d390cf0cfc: function(arg0) {
            let result;
            try {
                result = arg0 instanceof HTMLInputElement;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_HtmlSelectElement_dcf326cd531e9e5d: function(arg0) {
            let result;
            try {
                result = arg0 instanceof HTMLSelectElement;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_HtmlSpanElement_a9d2bdacba435f81: function(arg0) {
            let result;
            try {
                result = arg0 instanceof HTMLSpanElement;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_IdbDatabase_514495af00e5eab0: function(arg0) {
            let result;
            try {
                result = arg0 instanceof IDBDatabase;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_IdbRequest_a93f4449fab02673: function(arg0) {
            let result;
            try {
                result = arg0 instanceof IDBRequest;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Map_afa18d5840c04c15: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Map;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_MessagePort_8ae6ab500a37ad38: function(arg0) {
            let result;
            try {
                result = arg0 instanceof MessagePort;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Object_60be3eaa7a661141: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Object;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Promise_4614a0df6220bf3f: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Promise;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Response_79948c98d1d2ba75: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Response;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_ServiceWorkerContainer_b652459c915dba41: function(arg0) {
            let result;
            try {
                result = arg0 instanceof ServiceWorkerContainer;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_ServiceWorkerRegistration_857dc81dce12045f: function(arg0) {
            let result;
            try {
                result = arg0 instanceof ServiceWorkerRegistration;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Uint8Array_f080092dc70f5d58: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Uint8Array;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Window_0d356b88a2f77c42: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Window;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_isArray_145a34fd0a38d37b: function(arg0) {
            const ret = Array.isArray(arg0);
            return ret;
        },
        __wbg_isSafeInteger_a3389a198582f5f6: function(arg0) {
            const ret = Number.isSafeInteger(arg0);
            return ret;
        },
        __wbg_item_8d18c283826e39d3: function(arg0, arg1) {
            const ret = arg0.item(arg1 >>> 0);
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_iterator_cc47ba25a2be735a: function() {
            const ret = Symbol.iterator;
            return ret;
        },
        __wbg_length_589238bdcf171f0e: function(arg0) {
            const ret = arg0.length;
            return ret;
        },
        __wbg_length_c6054974c0a6cdb9: function(arg0) {
            const ret = arg0.length;
            return ret;
        },
        __wbg_location_31d6a0d86e2de805: function(arg0) {
            const ret = arg0.location;
            return ret;
        },
        __wbg_location_33177a2bd075d3ff: function(arg0) {
            const ret = arg0.location;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_log_6b5af08dd293697f: function(arg0) {
            console.log(arg0);
        },
        __wbg_lowerBound_1d29c85a03e93baf: function() { return handleError(function (arg0, arg1) {
            const ret = IDBKeyRange.lowerBound(arg0, arg1 !== 0);
            return ret;
        }, arguments); },
        __wbg_matchMedia_049ef5c8f8f904f2: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.matchMedia(getStringFromWasm0(arg1, arg2));
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        }, arguments); },
        __wbg_matches_a4c6202857b1dac9: function(arg0) {
            const ret = arg0.matches;
            return ret;
        },
        __wbg_message_6fc0a1f59fcc247b: function(arg0) {
            const ret = arg0.message;
            return ret;
        },
        __wbg_message_bc855ffd1f7f1de1: function(arg0, arg1) {
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
        __wbg_name_42366c160e14cac7: function(arg0, arg1) {
            const ret = arg1.name;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_name_e8b9038b31214119: function(arg0, arg1) {
            const ret = arg1.name;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_navigator_935098efd1dc7fe5: function(arg0) {
            const ret = arg0.navigator;
            return ret;
        },
        __wbg_newVersion_f3e3088bdcedc509: function(arg0, arg1) {
            const ret = arg1.newVersion;
            getDataViewMemory0().setFloat64(arg0 + 8 * 1, isLikeNone(ret) ? 0 : ret, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, !isLikeNone(ret), true);
        },
        __wbg_new_10e2f2ad134f940f: function() { return handleError(function () {
            const ret = new Headers();
            return ret;
        }, arguments); },
        __wbg_new_227d7c05414eb861: function() {
            const ret = new Error();
            return ret;
        },
        __wbg_new_26880d0ec07a54ec: function() { return handleError(function (arg0, arg1) {
            const ret = new URL(getStringFromWasm0(arg0, arg1));
            return ret;
        }, arguments); },
        __wbg_new_2e117a478906f062: function() {
            const ret = new Object();
            return ret;
        },
        __wbg_new_3444eb7412549f0b: function() {
            const ret = new Map();
            return ret;
        },
        __wbg_new_36e147a8ced3c6e0: function() {
            const ret = new Array();
            return ret;
        },
        __wbg_new_51233fa2a760b272: function() { return handleError(function () {
            const ret = new AbortController();
            return ret;
        }, arguments); },
        __wbg_new_5a19eef57e9178b5: function() { return handleError(function (arg0, arg1) {
            const ret = new WebSocket(getStringFromWasm0(arg0, arg1));
            return ret;
        }, arguments); },
        __wbg_new_81880fb5002cb255: function(arg0) {
            const ret = new Uint8Array(arg0);
            return ret;
        },
        __wbg_new_e66a4b7758dd2e5c: function(arg0, arg1) {
            const ret = new Error(getStringFromWasm0(arg0, arg1));
            return ret;
        },
        __wbg_new_f85beb941dc6d8aa: function(arg0, arg1) {
            try {
                var state0 = {a: arg0, b: arg1};
                var cb0 = (arg0, arg1) => {
                    const a = state0.a;
                    state0.a = 0;
                    try {
                        return wasm_bindgen__convert__closures_____invoke__h2dbd1dfa5eb2515d(a, state0.b, arg0, arg1);
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
        __wbg_new_from_slice_543b875b27789a8f: function(arg0, arg1) {
            const ret = new Uint8Array(getArrayU8FromWasm0(arg0, arg1));
            return ret;
        },
        __wbg_new_no_args_2d7333de60de5ac7: function(arg0, arg1) {
            const ret = new Function(getStringFromWasm0(arg0, arg1));
            return ret;
        },
        __wbg_new_typed_00a409eb4ec4f2d9: function(arg0, arg1) {
            try {
                var state0 = {a: arg0, b: arg1};
                var cb0 = (arg0, arg1) => {
                    const a = state0.a;
                    state0.a = 0;
                    try {
                        return wasm_bindgen__convert__closures_____invoke__h2dbd1dfa5eb2515d(a, state0.b, arg0, arg1);
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
        __wbg_new_with_args_e4a8af93bb714318: function(arg0, arg1, arg2, arg3) {
            const ret = new Function(getStringFromWasm0(arg0, arg1), getStringFromWasm0(arg2, arg3));
            return ret;
        },
        __wbg_new_with_length_9b650f44b5c44a4e: function(arg0) {
            const ret = new Uint8Array(arg0 >>> 0);
            return ret;
        },
        __wbg_new_with_str_and_init_5b299538bdeeec64: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = new Request(getStringFromWasm0(arg0, arg1), arg2);
            return ret;
        }, arguments); },
        __wbg_new_with_u8_array_sequence_and_options_319f40db8953dead: function() { return handleError(function (arg0, arg1) {
            const ret = new Blob(arg0, arg1);
            return ret;
        }, arguments); },
        __wbg_new_with_u8_array_sequence_and_options_6c342b44d8788e9a: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            const ret = new File(arg0, getStringFromWasm0(arg1, arg2), arg3);
            return ret;
        }, arguments); },
        __wbg_next_0c4066e251d2eff9: function() { return handleError(function (arg0) {
            const ret = arg0.next();
            return ret;
        }, arguments); },
        __wbg_next_402fa10b59ab20c3: function(arg0) {
            const ret = arg0.next;
            return ret;
        },
        __wbg_node_84ea875411254db1: function(arg0) {
            const ret = arg0.node;
            return ret;
        },
        __wbg_now_d2e0afbad4edbe82: function() {
            const ret = Date.now();
            return ret;
        },
        __wbg_now_e7c6795a7f81e10f: function(arg0) {
            const ret = arg0.now();
            return ret;
        },
        __wbg_objectStore_3468a23e50c7e125: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.objectStore(getStringFromWasm0(arg1, arg2));
            return ret;
        }, arguments); },
        __wbg_oldVersion_5056451b542903d6: function(arg0) {
            const ret = arg0.oldVersion;
            return ret;
        },
        __wbg_on_f4145ffe4fe22bf2: function(arg0, arg1, arg2, arg3) {
            arg0.on(getStringFromWasm0(arg1, arg2), arg3);
        },
        __wbg_open_07d64a5e2d11e344: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            const ret = arg0.open(getStringFromWasm0(arg1, arg2), arg3 >>> 0);
            return ret;
        }, arguments); },
        __wbg_open_912e1d9c76c425c1: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6) {
            const ret = arg0.open(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), getStringFromWasm0(arg5, arg6));
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        }, arguments); },
        __wbg_origin_e9c087f790860e56: function() { return handleError(function (arg0, arg1) {
            const ret = arg1.origin;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        }, arguments); },
        __wbg_parentNode_31eaeb2d9a747844: function(arg0) {
            const ret = arg0.parentNode;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_pathname_321d02ae383a4644: function(arg0, arg1) {
            const ret = arg1.pathname;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_pathname_934ddc995a86dfc6: function() { return handleError(function (arg0, arg1) {
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
        __wbg_ports_d81c8cd179f684c5: function(arg0) {
            const ret = arg0.ports;
            return ret;
        },
        __wbg_postMessage_5b1dc9f8de88488a: function() { return handleError(function (arg0, arg1) {
            arg0.postMessage(arg1);
        }, arguments); },
        __wbg_prepend_93060cd4d2fc4139: function() { return handleError(function (arg0, arg1) {
            arg0.prepend(arg1);
        }, arguments); },
        __wbg_process_44c7a14e11e9f69e: function(arg0) {
            const ret = arg0.process;
            return ret;
        },
        __wbg_prototypesetcall_d721637c7ca66eb8: function(arg0, arg1, arg2) {
            Uint8Array.prototype.set.call(getArrayU8FromWasm0(arg0, arg1), arg2);
        },
        __wbg_push_f724b5db8acf89d2: function(arg0, arg1) {
            const ret = arg0.push(arg1);
            return ret;
        },
        __wbg_put_738d34320465aaf3: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.put(arg1, arg2);
            return ret;
        }, arguments); },
        __wbg_queueMicrotask_1c9b3800e321a967: function(arg0) {
            const ret = arg0.queueMicrotask;
            return ret;
        },
        __wbg_queueMicrotask_311744e534a929a3: function(arg0) {
            queueMicrotask(arg0);
        },
        __wbg_randomFillSync_6c25eac9869eb53c: function() { return handleError(function (arg0, arg1) {
            arg0.randomFillSync(arg1);
        }, arguments); },
        __wbg_readyState_82f116ed130415ca: function(arg0) {
            const ret = arg0.readyState;
            return (__wbindgen_enum_IdbRequestReadyState.indexOf(ret) + 1 || 3) - 1;
        },
        __wbg_readyState_97951098f8995393: function(arg0) {
            const ret = arg0.readyState;
            return ret;
        },
        __wbg_ready_ccb740bbbabf19c3: function() { return handleError(function (arg0) {
            const ret = arg0.ready;
            return ret;
        }, arguments); },
        __wbg_register_94d5c686ace4d45e: function(arg0, arg1, arg2) {
            const ret = arg0.register(getStringFromWasm0(arg1, arg2));
            return ret;
        },
        __wbg_removeAttribute_f13ae85cfd9fbd58: function() { return handleError(function (arg0, arg1, arg2) {
            arg0.removeAttribute(getStringFromWasm0(arg1, arg2));
        }, arguments); },
        __wbg_removeChild_8e93fdf43472d328: function() { return handleError(function (arg0, arg1) {
            const ret = arg0.removeChild(arg1);
            return ret;
        }, arguments); },
        __wbg_removeListener_250a33965641c61b: function(arg0, arg1, arg2, arg3) {
            arg0.removeListener(getStringFromWasm0(arg1, arg2), arg3);
        },
        __wbg_remove_627151898f1aafc8: function(arg0) {
            arg0.remove();
        },
        __wbg_replaceState_9dbb811c2a513322: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5) {
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
        __wbg_resolve_d82363d90af6928a: function(arg0) {
            const ret = Promise.resolve(arg0);
            return ret;
        },
        __wbg_result_ad4d0eede558cd6c: function() { return handleError(function (arg0) {
            const ret = arg0.result;
            return ret;
        }, arguments); },
        __wbg_send_982c819b9a1b34a5: function() { return handleError(function (arg0, arg1, arg2) {
            arg0.send(getArrayU8FromWasm0(arg1, arg2));
        }, arguments); },
        __wbg_serviceWorker_8b8b54241ccc2d16: function(arg0) {
            const ret = arg0.serviceWorker;
            return ret;
        },
        __wbg_setAttribute_8bccfbabf2a83682: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4) {
            arg0.setAttribute(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4));
        }, arguments); },
        __wbg_setInterval_b71f681a73dc6f24: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            const ret = arg0.setInterval(arg1, arg2, ...arg3);
            return ret;
        }, arguments); },
        __wbg_setInterval_c824f49666854b47: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            const ret = arg0.setInterval(arg1, arg2, ...arg3);
            return ret;
        }, arguments); },
        __wbg_setTimeout_56bcdccbad22fd44: function() { return handleError(function (arg0, arg1) {
            const ret = setTimeout(arg0, arg1);
            return ret;
        }, arguments); },
        __wbg_setTimeout_8afa0b5ed243c77d: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.setTimeout(arg1, arg2);
            return ret;
        }, arguments); },
        __wbg_setTimeout_ef24d2fc3ad97385: function() { return handleError(function (arg0, arg1) {
            const ret = setTimeout(arg0, arg1);
            return ret;
        }, arguments); },
        __wbg_set_0bf1fca872bc6d18: function(arg0, arg1, arg2) {
            arg0.set(getArrayU8FromWasm0(arg1, arg2));
        },
        __wbg_set_4564f7dc44fcb0c9: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = Reflect.set(arg0, arg1, arg2);
            return ret;
        }, arguments); },
        __wbg_set_6be42768c690e380: function(arg0, arg1, arg2) {
            arg0[arg1] = arg2;
        },
        __wbg_set_9a1d61e17de7054c: function(arg0, arg1, arg2) {
            const ret = arg0.set(arg1, arg2);
            return ret;
        },
        __wbg_set_auto_increment_5a55ad6938b60e2c: function(arg0, arg1) {
            arg0.autoIncrement = arg1 !== 0;
        },
        __wbg_set_binaryType_148427b11a8e6551: function(arg0, arg1) {
            arg0.binaryType = __wbindgen_enum_BinaryType[arg1];
        },
        __wbg_set_body_97c25d1c0051cb04: function(arg0, arg1) {
            arg0.body = arg1;
        },
        __wbg_set_className_19e05f9bbe754550: function(arg0, arg1, arg2) {
            arg0.className = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_credentials_8dece1804391d22f: function(arg0, arg1) {
            arg0.credentials = __wbindgen_enum_RequestCredentials[arg1];
        },
        __wbg_set_dc601f4a69da0bc2: function(arg0, arg1, arg2) {
            arg0[arg1 >>> 0] = arg2;
        },
        __wbg_set_headers_6751c09a8e579ff7: function(arg0, arg1) {
            arg0.headers = arg1;
        },
        __wbg_set_id_ee963d49b4125c0b: function(arg0, arg1, arg2) {
            arg0.id = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_innerHTML_30fcedd016ac76ab: function(arg0, arg1, arg2) {
            arg0.innerHTML = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_last_modified_7987f3a09fd03106: function(arg0, arg1) {
            arg0.lastModified = arg1;
        },
        __wbg_set_method_1120482abe0934aa: function(arg0, arg1, arg2) {
            arg0.method = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_mode_e41f820af904cdaa: function(arg0, arg1) {
            arg0.mode = __wbindgen_enum_RequestMode[arg1];
        },
        __wbg_set_onabort_aa769067996be236: function(arg0, arg1) {
            arg0.onabort = arg1;
        },
        __wbg_set_onclick_3488240f3d66707b: function(arg0, arg1) {
            arg0.onclick = arg1;
        },
        __wbg_set_onclose_8134952b2a9ec104: function(arg0, arg1) {
            arg0.onclose = arg1;
        },
        __wbg_set_oncomplete_56fb9939584534f1: function(arg0, arg1) {
            arg0.oncomplete = arg1;
        },
        __wbg_set_onerror_0803e0826d3abdc4: function(arg0, arg1) {
            arg0.onerror = arg1;
        },
        __wbg_set_onerror_3f68563f77d362f1: function(arg0, arg1) {
            arg0.onerror = arg1;
        },
        __wbg_set_onerror_ba8db3530f46f30a: function(arg0, arg1) {
            arg0.onerror = arg1;
        },
        __wbg_set_oninput_e07eb7932c7c582c: function(arg0, arg1) {
            arg0.oninput = arg1;
        },
        __wbg_set_onmessage_397a79f643011142: function(arg0, arg1) {
            arg0.onmessage = arg1;
        },
        __wbg_set_onopen_ca8d311fe5282041: function(arg0, arg1) {
            arg0.onopen = arg1;
        },
        __wbg_set_onsuccess_2cdfe0be022e28fa: function(arg0, arg1) {
            arg0.onsuccess = arg1;
        },
        __wbg_set_onupgradeneeded_b840f47064664247: function(arg0, arg1) {
            arg0.onupgradeneeded = arg1;
        },
        __wbg_set_signal_4a69430cb12800f3: function(arg0, arg1) {
            arg0.signal = arg1;
        },
        __wbg_set_textContent_5c5fef072bd24f7a: function(arg0, arg1, arg2) {
            arg0.textContent = arg1 === 0 ? undefined : getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_type_3194793b7f2a05be: function(arg0, arg1, arg2) {
            arg0.type = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_type_f676716eda212ed4: function(arg0, arg1, arg2) {
            arg0.type = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_value_3cd64c88136b460c: function(arg0, arg1, arg2) {
            arg0.value = getStringFromWasm0(arg1, arg2);
        },
        __wbg_signal_4d9d567be73ea52c: function(arg0) {
            const ret = arg0.signal;
            return ret;
        },
        __wbg_size_3a5e8d210d038789: function(arg0) {
            const ret = arg0.size;
            return ret;
        },
        __wbg_slice_faf194aa1e582f72: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.slice(arg1, arg2);
            return ret;
        }, arguments); },
        __wbg_stack_3b0d974bbf31e44f: function(arg0, arg1) {
            const ret = arg1.stack;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_static_accessor_GLOBAL_THIS_2fee5048bcca5938: function() {
            const ret = typeof globalThis === 'undefined' ? null : globalThis;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_static_accessor_GLOBAL_ce44e66a4935da8c: function() {
            const ret = typeof global === 'undefined' ? null : global;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_static_accessor_SELF_44f6e0cb5e67cdad: function() {
            const ret = typeof self === 'undefined' ? null : self;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_static_accessor_WINDOW_168f178805d978fe: function() {
            const ret = typeof window === 'undefined' ? null : window;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_status_0053aa6239760447: function(arg0) {
            const ret = arg0.status;
            return ret;
        },
        __wbg_stringify_747a843de2eb6359: function() { return handleError(function (arg0) {
            const ret = JSON.stringify(arg0);
            return ret;
        }, arguments); },
        __wbg_subarray_b0e8ac4ed313fea8: function(arg0, arg1, arg2) {
            const ret = arg0.subarray(arg1 >>> 0, arg2 >>> 0);
            return ret;
        },
        __wbg_target_4387d5c508f1ecbd: function(arg0) {
            const ret = arg0.target;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_then_05edfc8a4fea5106: function(arg0, arg1, arg2) {
            const ret = arg0.then(arg1, arg2);
            return ret;
        },
        __wbg_then_591b6b3a75ee817a: function(arg0, arg1) {
            const ret = arg0.then(arg1);
            return ret;
        },
        __wbg_toString_73456969ad00f2fa: function(arg0) {
            const ret = arg0.toString();
            return ret;
        },
        __wbg_transaction_b5b9bb20a5d6d686: function(arg0) {
            const ret = arg0.transaction;
            return ret;
        },
        __wbg_transaction_e1fa871de48c3ddf: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            const ret = arg0.transaction(getStringFromWasm0(arg1, arg2), __wbindgen_enum_IdbTransactionMode[arg3]);
            return ret;
        }, arguments); },
        __wbg_type_0040ed884cfbbe36: function(arg0, arg1) {
            const ret = arg1.type;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_update_3c7dbcf0a1486332: function() { return handleError(function (arg0) {
            const ret = arg0.update();
            return ret;
        }, arguments); },
        __wbg_upperBound_8ea9eab1a84b6c95: function() { return handleError(function (arg0, arg1) {
            const ret = IDBKeyRange.upperBound(arg0, arg1 !== 0);
            return ret;
        }, arguments); },
        __wbg_url_0e0eeabf01fb5519: function(arg0, arg1) {
            const ret = arg1.url;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_value_1686b2f0ffb64a18: function(arg0, arg1) {
            const ret = arg1.value;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_value_18f5592a194a3ef2: function(arg0, arg1) {
            const ret = arg1.value;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_value_49f783bb59765962: function(arg0) {
            const ret = arg0.value;
            return ret;
        },
        __wbg_versions_276b2795b1c6a219: function(arg0) {
            const ret = arg0.versions;
            return ret;
        },
        __wbg_warn_cd671287bc02594a: function(arg0) {
            console.warn(arg0);
        },
        __wbindgen_cast_0000000000000001: function(arg0, arg1) {
            // Cast intrinsic for `Closure(Closure { owned: true, function: Function { arguments: [Externref], shim_idx: 1509, ret: Unit, inner_ret: Some(Unit) }, mutable: true }) -> Externref`.
            const ret = makeMutClosure(arg0, arg1, wasm_bindgen__convert__closures_____invoke__h26ff64c82913faeb);
            return ret;
        },
        __wbindgen_cast_0000000000000002: function(arg0, arg1) {
            // Cast intrinsic for `Closure(Closure { owned: true, function: Function { arguments: [Externref], shim_idx: 2650, ret: Result(Unit), inner_ret: Some(Result(Unit)) }, mutable: true }) -> Externref`.
            const ret = makeMutClosure(arg0, arg1, wasm_bindgen__convert__closures_____invoke__h47cda684020a036d);
            return ret;
        },
        __wbindgen_cast_0000000000000003: function(arg0, arg1) {
            // Cast intrinsic for `Closure(Closure { owned: true, function: Function { arguments: [NamedExternref("CloseEvent")], shim_idx: 1681, ret: Unit, inner_ret: Some(Unit) }, mutable: true }) -> Externref`.
            const ret = makeMutClosure(arg0, arg1, wasm_bindgen__convert__closures_____invoke__h379d59770944ec5d);
            return ret;
        },
        __wbindgen_cast_0000000000000004: function(arg0, arg1) {
            // Cast intrinsic for `Closure(Closure { owned: true, function: Function { arguments: [NamedExternref("Event")], shim_idx: 1303, ret: Unit, inner_ret: Some(Unit) }, mutable: true }) -> Externref`.
            const ret = makeMutClosure(arg0, arg1, wasm_bindgen__convert__closures_____invoke__h95b033a19c54e7b9);
            return ret;
        },
        __wbindgen_cast_0000000000000005: function(arg0, arg1) {
            // Cast intrinsic for `Closure(Closure { owned: true, function: Function { arguments: [NamedExternref("Event")], shim_idx: 1681, ret: Unit, inner_ret: Some(Unit) }, mutable: true }) -> Externref`.
            const ret = makeMutClosure(arg0, arg1, wasm_bindgen__convert__closures_____invoke__h379d59770944ec5d_4);
            return ret;
        },
        __wbindgen_cast_0000000000000006: function(arg0, arg1) {
            // Cast intrinsic for `Closure(Closure { owned: true, function: Function { arguments: [NamedExternref("IDBVersionChangeEvent")], shim_idx: 1165, ret: Result(Unit), inner_ret: Some(Result(Unit)) }, mutable: true }) -> Externref`.
            const ret = makeMutClosure(arg0, arg1, wasm_bindgen__convert__closures_____invoke__h2201e44ca17b925f);
            return ret;
        },
        __wbindgen_cast_0000000000000007: function(arg0, arg1) {
            // Cast intrinsic for `Closure(Closure { owned: true, function: Function { arguments: [NamedExternref("MessageEvent")], shim_idx: 1681, ret: Unit, inner_ret: Some(Unit) }, mutable: true }) -> Externref`.
            const ret = makeMutClosure(arg0, arg1, wasm_bindgen__convert__closures_____invoke__h379d59770944ec5d_6);
            return ret;
        },
        __wbindgen_cast_0000000000000008: function(arg0, arg1) {
            // Cast intrinsic for `Closure(Closure { owned: true, function: Function { arguments: [], shim_idx: 1301, ret: Unit, inner_ret: Some(Unit) }, mutable: true }) -> Externref`.
            const ret = makeMutClosure(arg0, arg1, wasm_bindgen__convert__closures_____invoke__h425b69cf20c84c8b);
            return ret;
        },
        __wbindgen_cast_0000000000000009: function(arg0, arg1) {
            // Cast intrinsic for `Closure(Closure { owned: true, function: Function { arguments: [], shim_idx: 2223, ret: Unit, inner_ret: Some(Unit) }, mutable: true }) -> Externref`.
            const ret = makeMutClosure(arg0, arg1, wasm_bindgen__convert__closures_____invoke__he2f39d22c2064455);
            return ret;
        },
        __wbindgen_cast_000000000000000a: function(arg0, arg1) {
            // Cast intrinsic for `Closure(Closure { owned: true, function: Function { arguments: [], shim_idx: 2628, ret: Unit, inner_ret: Some(Unit) }, mutable: true }) -> Externref`.
            const ret = makeMutClosure(arg0, arg1, wasm_bindgen__convert__closures_____invoke__h5ec3bf3c1e3b528c);
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

function wasm_bindgen__convert__closures_____invoke__h425b69cf20c84c8b(arg0, arg1) {
    wasm.wasm_bindgen__convert__closures_____invoke__h425b69cf20c84c8b(arg0, arg1);
}

function wasm_bindgen__convert__closures_____invoke__he2f39d22c2064455(arg0, arg1) {
    wasm.wasm_bindgen__convert__closures_____invoke__he2f39d22c2064455(arg0, arg1);
}

function wasm_bindgen__convert__closures_____invoke__h5ec3bf3c1e3b528c(arg0, arg1) {
    wasm.wasm_bindgen__convert__closures_____invoke__h5ec3bf3c1e3b528c(arg0, arg1);
}

function wasm_bindgen__convert__closures_____invoke__h26ff64c82913faeb(arg0, arg1, arg2) {
    wasm.wasm_bindgen__convert__closures_____invoke__h26ff64c82913faeb(arg0, arg1, arg2);
}

function wasm_bindgen__convert__closures_____invoke__h379d59770944ec5d(arg0, arg1, arg2) {
    wasm.wasm_bindgen__convert__closures_____invoke__h379d59770944ec5d(arg0, arg1, arg2);
}

function wasm_bindgen__convert__closures_____invoke__h95b033a19c54e7b9(arg0, arg1, arg2) {
    wasm.wasm_bindgen__convert__closures_____invoke__h95b033a19c54e7b9(arg0, arg1, arg2);
}

function wasm_bindgen__convert__closures_____invoke__h379d59770944ec5d_4(arg0, arg1, arg2) {
    wasm.wasm_bindgen__convert__closures_____invoke__h379d59770944ec5d_4(arg0, arg1, arg2);
}

function wasm_bindgen__convert__closures_____invoke__h379d59770944ec5d_6(arg0, arg1, arg2) {
    wasm.wasm_bindgen__convert__closures_____invoke__h379d59770944ec5d_6(arg0, arg1, arg2);
}

function wasm_bindgen__convert__closures_____invoke__h47cda684020a036d(arg0, arg1, arg2) {
    const ret = wasm.wasm_bindgen__convert__closures_____invoke__h47cda684020a036d(arg0, arg1, arg2);
    if (ret[1]) {
        throw takeFromExternrefTable0(ret[0]);
    }
}

function wasm_bindgen__convert__closures_____invoke__h2201e44ca17b925f(arg0, arg1, arg2) {
    const ret = wasm.wasm_bindgen__convert__closures_____invoke__h2201e44ca17b925f(arg0, arg1, arg2);
    if (ret[1]) {
        throw takeFromExternrefTable0(ret[0]);
    }
}

function wasm_bindgen__convert__closures_____invoke__h2dbd1dfa5eb2515d(arg0, arg1, arg2, arg3) {
    wasm.wasm_bindgen__convert__closures_____invoke__h2dbd1dfa5eb2515d(arg0, arg1, arg2, arg3);
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
