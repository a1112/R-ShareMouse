#import <Foundation/Foundation.h>
#import <Network/Network.h>
#import <Security/Security.h>
#include <atomic>

@interface RShareLANConnection : NSObject {
@public
    std::atomic<bool> cancelled;
}
@property(strong) nw_connection_t connection;
@end
@implementation RShareLANConnection
@end

static dispatch_data_t copiedData(const void *bytes, size_t size) {
    void *copy = malloc(size);
    memcpy(copy, bytes, size);
    return dispatch_data_create(copy, size, nullptr, DISPATCH_DATA_DESTRUCTOR_FREE);
}

extern "C" void *rshare_lan_connect(const char *host, const uint8_t *key, char *error, size_t errorSize) {
    @autoreleasepool {
        // A fresh 256-bit out-of-band key authenticates both ends. Never disable
        // certificate verification or fall back to unencrypted TCP.
        dispatch_data_t psk = copiedData(key, 32);
        const char identity[] = "rshare-display-v1";
        dispatch_data_t label = copiedData(identity, sizeof(identity)-1);
        nw_parameters_t parameters = nw_parameters_create_secure_tcp(^(nw_protocol_options_t tls) {
            sec_protocol_options_t options = nw_tls_copy_sec_protocol_options(tls);
            sec_protocol_options_set_min_tls_protocol_version(options, tls_protocol_version_TLSv12);
            sec_protocol_options_set_max_tls_protocol_version(options, tls_protocol_version_TLSv12);
            sec_protocol_options_append_tls_ciphersuite(options, (tls_ciphersuite_t)TLS_PSK_WITH_AES_128_GCM_SHA256);
            sec_protocol_options_add_pre_shared_key(options, psk, label);
        }, ^(nw_protocol_options_t tcp) { nw_tcp_options_set_no_delay(tcp, true); });
        nw_parameters_set_prefer_no_proxy(parameters, true);
        RShareLANConnection *stream = [RShareLANConnection new];
        stream.connection = nw_connection_create(nw_endpoint_create_host(host, "27439"), parameters);
        dispatch_queue_t queue = dispatch_queue_create("com.rshare.display.lan", DISPATCH_QUEUE_SERIAL);
        nw_connection_set_queue(stream.connection, queue);
        dispatch_semaphore_t ready = dispatch_semaphore_create(0);
        __block bool connected = false;
        __block int failure = 0;
        nw_connection_set_state_changed_handler(stream.connection, ^(nw_connection_state_t state, nw_error_t reason) {
            if (state == nw_connection_state_ready) { connected = true; dispatch_semaphore_signal(ready); }
            if (state == nw_connection_state_failed || state == nw_connection_state_cancelled || state == nw_connection_state_waiting) {
                failure = reason ? nw_error_get_error_code(reason) : -1; dispatch_semaphore_signal(ready);
            }
        });
        nw_connection_start(stream.connection);
        long timedOut = dispatch_semaphore_wait(ready, dispatch_time(DISPATCH_TIME_NOW, 6 * NSEC_PER_SEC));
        nw_connection_set_state_changed_handler(stream.connection, nil);
        if (timedOut || !connected) {
            nw_connection_cancel(stream.connection);
            snprintf(error, errorSize, "LAN TLS pairing failed (%d); check the phone link, local-network permission and Wi-Fi", failure);
            return nullptr;
        }
        return (__bridge_retained void *)stream;
    }
}

extern "C" long rshare_lan_read(void *handle, uint8_t *output, size_t capacity) {
    @autoreleasepool {
        RShareLANConnection *stream = (__bridge RShareLANConnection *)handle;
        if (stream->cancelled) return -1;
        dispatch_semaphore_t done = dispatch_semaphore_create(0);
        __block NSData *received = nil; __block bool failed = false;
        nw_connection_receive(stream.connection, 1, (uint32_t)MIN(capacity, 65536),
            ^(dispatch_data_t content, nw_content_context_t, bool, nw_error_t error) {
                failed = error != nil;
                if (content) {
                    const void *bytes = nullptr; size_t size = 0;
                    dispatch_data_t mapped = dispatch_data_create_map(content, &bytes, &size);
                    received = [NSData dataWithBytes:bytes length:size]; (void)mapped;
                }
                dispatch_semaphore_signal(done);
            });
        if (dispatch_semaphore_wait(done, dispatch_time(DISPATCH_TIME_NOW, 5 * NSEC_PER_SEC))) {
            stream->cancelled = true; nw_connection_cancel(stream.connection); return -1;
        }
        if (failed || stream->cancelled) return -1;
        if (received.length) memcpy(output, received.bytes, received.length);
        return (long)received.length;
    }
}
extern "C" bool rshare_lan_write(void *handle, const uint8_t *bytes, size_t size) {
    @autoreleasepool {
        RShareLANConnection *stream = (__bridge RShareLANConnection *)handle;
        if (stream->cancelled) return false;
        dispatch_semaphore_t done = dispatch_semaphore_create(0);
        __block bool failed = false;
        nw_connection_send(stream.connection, copiedData(bytes, size), NW_CONNECTION_DEFAULT_MESSAGE_CONTEXT, true,
            ^(nw_error_t error) { failed = error != nil; dispatch_semaphore_signal(done); });
        if (dispatch_semaphore_wait(done, dispatch_time(DISPATCH_TIME_NOW, 5 * NSEC_PER_SEC))) {
            stream->cancelled = true; nw_connection_cancel(stream.connection); return false;
        }
        return !failed && !stream->cancelled;
    }
}
extern "C" void rshare_lan_cancel(void *handle) {
    RShareLANConnection *stream = (__bridge RShareLANConnection *)handle;
    stream->cancelled = true; nw_connection_cancel(stream.connection);
}
extern "C" void rshare_lan_release(void *handle) {
    @autoreleasepool { RShareLANConnection *stream = CFBridgingRelease(handle); nw_connection_cancel(stream.connection); }
}
