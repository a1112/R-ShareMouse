#import <Foundation/Foundation.h>
#import <AppKit/AppKit.h>
#import <CoreGraphics/CoreGraphics.h>
#import <ScreenCaptureKit/ScreenCaptureKit.h>
#import <VideoToolbox/VideoToolbox.h>
#include <atomic>
#include <cstring>
#include <unistd.h>

// CoreGraphics exposes these classes at runtime, but not in the public SDK.
// Resolve every class dynamically: unsupported OS versions fail closed.
// ABI reference: Chromium ui/display/mac/test/virtual_display_mac_util.mm.
@interface NSObject (RShareVirtualDisplayABI)
- (id)initWithDescriptor:(id)descriptor;
- (id)initWithWidth:(unsigned)width height:(unsigned)height refreshRate:(double)rate;
- (BOOL)applySettings:(id)settings;
- (unsigned)displayID;
- (void)setSizeInMillimeters:(CGSize)size;
- (void)setRedPrimary:(CGPoint)point;
- (void)setGreenPrimary:(CGPoint)point;
- (void)setBluePrimary:(CGPoint)point;
- (void)setWhitePoint:(CGPoint)point;
@end

static void fail(char *error, size_t size, NSString *message) {
    if (size) snprintf(error, size, "%s", message.UTF8String ?: "macOS display error");
}
static void displayChanged(CGDirectDisplayID, CGDisplayChangeSummaryFlags, void *) {}

extern "C" int rshare_mac_display_online(unsigned target) {
    CGDirectDisplayID ids[64]; uint32_t count = 0;
    if (CGGetOnlineDisplayList(64, ids, &count) != kCGErrorSuccess) return -1;
    for (unsigned i = 0; i < count; i++) if (ids[i] == target) return 1;
    return 0;
}

extern "C" bool rshare_mac_session_unlocked() {
    CFDictionaryRef session = CGSessionCopyCurrentDictionary();
    if (!session) return false;
    bool onConsole = CFDictionaryGetValue(session, kCGSessionOnConsoleKey) == kCFBooleanTrue;
    bool locked = CFDictionaryGetValue(session, CFSTR("CGSSessionScreenIsLocked")) == kCFBooleanTrue;
    CFRelease(session);
    return onConsole && !locked;
}

extern "C" void *rshare_mac_display_create(unsigned width, unsigned height, double hz,
    const char *name, unsigned *displayID, char *error, size_t errorSize) {
    @autoreleasepool { @try {
        static dispatch_once_t notifications;
        dispatch_once(&notifications, ^{ CGDisplayRegisterReconfigurationCallback(displayChanged, nullptr); });
        if (!rshare_mac_session_unlocked()) {
            fail(error, errorSize, @"Unlock the Mac desktop before starting an extended display"); return nullptr;
        }
        Class descriptorClass = NSClassFromString(@"CGVirtualDisplayDescriptor");
        Class displayClass = NSClassFromString(@"CGVirtualDisplay");
        Class modeClass = NSClassFromString(@"CGVirtualDisplayMode");
        Class settingsClass = NSClassFromString(@"CGVirtualDisplaySettings");
        if (!descriptorClass || !displayClass || !modeClass || !settingsClass) {
            fail(error, errorSize, @"This macOS version has no compatible CGVirtualDisplay API"); return nullptr;
        }
        id descriptor = [descriptorClass new];
        [descriptor setValue:dispatch_get_global_queue(QOS_CLASS_USER_INTERACTIVE, 0) forKey:@"queue"];
        [descriptor setValue:[NSString stringWithUTF8String:name] forKey:@"name"];
        [descriptor setValue:@(width) forKey:@"maxPixelsWide"];
        [descriptor setValue:@(height) forKey:@"maxPixelsHigh"];
        // Recent macOS requires a nonzero vendor and a unique nonzero serial.
        [descriptor setValue:@(0x5253) forKey:@"vendorID"];
        [descriptor setValue:@(1) forKey:@"productID"];
        NSNumber *serial = @(arc4random() | 1);
        [descriptor setValue:serial forKey:@"serialNum"];
        if ([descriptor respondsToSelector:NSSelectorFromString(@"setSerialNumber:")]) [descriptor setValue:serial forKey:@"serialNumber"];
        [descriptor setSizeInMillimeters:CGSizeMake(width * 25.4 / 110, height * 25.4 / 110)];
        [descriptor setRedPrimary:CGPointMake(.64, .33)];
        [descriptor setGreenPrimary:CGPointMake(.30, .60)];
        [descriptor setBluePrimary:CGPointMake(.15, .06)];
        [descriptor setWhitePoint:CGPointMake(.3127, .3290)];
        id display = [[displayClass alloc] initWithDescriptor:descriptor];
        if (!display) { fail(error, errorSize, @"WindowServer rejected the virtual display"); return nullptr; }
        id settings = [settingsClass new];
        [settings setValue:@(0) forKey:@"hiDPI"];
        id mode = [[modeClass alloc] initWithWidth:width height:height refreshRate:hz];
        [settings setValue:@[mode] forKey:@"modes"];
        if (![display applySettings:settings]) { fail(error, errorSize, @"Virtual display mode was rejected"); return nullptr; }
        unsigned target = [display displayID];
        for (int i = 0; i < 100 && rshare_mac_display_online(target) != 1; i++) {
            CFRunLoopRunInMode(kCFRunLoopDefaultMode, .02, false); usleep(10000);
        }
        // A new unique identity normally joins as an extended screen. Avoid a
        // CG display-configuration transaction here: on recent macOS even a
        // redundant transaction can keep a virtual screen alive after release.
        if (rshare_mac_display_online(target) != 1 || CGDisplayMirrorsDisplay(target)) {
            fail(error, errorSize, @"WindowServer did not attach an independent extended screen"); return nullptr;
        }
        *displayID = target;
        return (__bridge_retained void *)display;
    } @catch (NSException *exception) { fail(error, errorSize, exception.reason); return nullptr; } }
}

extern "C" void rshare_mac_display_remove(void *handle) {
    unsigned target = 0;
    @autoreleasepool {
        target = [(__bridge id)handle displayID];
        CFRelease(handle);
    }
    for (int i = 0; i < 200 && rshare_mac_display_online(target) != 0; i++) {
        CFRunLoopRunInMode(kCFRunLoopDefaultMode, .02, false); usleep(10000);
    }
}

typedef bool (*RShareFrameSink)(void *, const unsigned char *, size_t);
@interface RShareDisplayCapture : NSObject <SCStreamOutput, SCStreamDelegate> {
@public
    VTCompressionSessionRef encoder;
    RShareFrameSink sink;
    void *context;
    std::atomic<bool> stopping;
    std::atomic<bool> encoding;
    std::atomic<bool> forceKey;
    std::atomic<int> captureError;
}
@property(strong) SCStream *stream;
@property(strong) dispatch_queue_t queue;
@end

static void encodedFrame(void *refcon, void *, OSStatus status, VTEncodeInfoFlags, CMSampleBufferRef sample) {
    RShareDisplayCapture *capture = (__bridge RShareDisplayCapture *)refcon;
    if (capture->stopping) { capture->encoding = false; return; }
    if (status || !sample || !CMSampleBufferDataIsReady(sample)) {
        if (status) capture->captureError = status;
        capture->encoding = false; capture->forceKey = true; return;
    }
    @autoreleasepool {
        CFArrayRef attachments = CMSampleBufferGetSampleAttachmentsArray(sample, false);
        bool key = !attachments || !CFArrayGetCount(attachments) || !CFDictionaryContainsKey((CFDictionaryRef)CFArrayGetValueAtIndex(attachments, 0), kCMSampleAttachmentKey_NotSync);
        const uint8_t *sps = nullptr, *pps = nullptr; size_t spsSize = 0, ppsSize = 0, count = 0; int nalSize = 0;
        if (key) {
            CMFormatDescriptionRef format = CMSampleBufferGetFormatDescription(sample);
            CMVideoFormatDescriptionGetH264ParameterSetAtIndex(format, 0, &sps, &spsSize, &count, &nalSize);
            CMVideoFormatDescriptionGetH264ParameterSetAtIndex(format, 1, &pps, &ppsSize, &count, &nalSize);
        }
        CMBlockBufferRef block = CMSampleBufferGetDataBuffer(sample);
        size_t length = block ? CMBlockBufferGetDataLength(block) : 0;
        if (!length || length > 8 * 1024 * 1024 - 131072 || spsSize > UINT16_MAX || ppsSize > UINT16_MAX || (key && (!spsSize || !ppsSize || nalSize != 4))) {
            capture->captureError = -1; capture->encoding = false; return;
        }
        uint8_t header[4] = {(uint8_t)(spsSize >> 8), (uint8_t)spsSize, (uint8_t)(ppsSize >> 8), (uint8_t)ppsSize};
        NSMutableData *packet = [NSMutableData dataWithBytes:header length:4];
        if (spsSize) [packet appendBytes:sps length:spsSize];
        if (ppsSize) [packet appendBytes:pps length:ppsSize];
        size_t offset = packet.length; [packet increaseLengthBy:length];
        if (CMBlockBufferCopyDataBytes(block, 0, length, (uint8_t *)packet.mutableBytes + offset) != noErr ||
            !capture->sink(capture->context, (const unsigned char *)packet.bytes, packet.length)) capture->forceKey = true;
    }
    capture->encoding = false;
}

@implementation RShareDisplayCapture
- (void)stream:(SCStream *)stream didStopWithError:(NSError *)error { captureError = (int)error.code ?: -1; }
- (void)stream:(SCStream *)stream didOutputSampleBuffer:(CMSampleBufferRef)sample ofType:(SCStreamOutputType)type {
    if (type != SCStreamOutputTypeScreen || stopping || !CMSampleBufferIsValid(sample)) return;
    NSArray *attachments = (__bridge NSArray *)CMSampleBufferGetSampleAttachmentsArray(sample, false);
    NSNumber *status = attachments.firstObject[SCStreamFrameInfoStatus];
    if (status && status.integerValue != SCFrameStatusComplete) return;
    CVImageBufferRef image = CMSampleBufferGetImageBuffer(sample);
    if (!image || encoding.exchange(true)) return;
    NSDictionary *options = forceKey.exchange(false) ? @{(__bridge NSString *)kVTEncodeFrameOptionKey_ForceKeyFrame: @YES} : nil;
    OSStatus result = VTCompressionSessionEncodeFrame(encoder, image, CMSampleBufferGetPresentationTimeStamp(sample),
        kCMTimeInvalid, (__bridge CFDictionaryRef)options, nullptr, nullptr);
    if (result) { captureError = result; encoding = false; }
}
@end

static OSStatus createEncoder(RShareDisplayCapture *capture, unsigned width, unsigned height, unsigned fps) {
    OSStatus code = VTCompressionSessionCreate(kCFAllocatorDefault, width, height, kCMVideoCodecType_H264,
        nullptr, nullptr, nullptr, encodedFrame, (__bridge void *)capture, &capture->encoder);
    if (code) return code;
    NSDictionary *properties = @{
        (__bridge NSString *)kVTCompressionPropertyKey_RealTime: @YES,
        (__bridge NSString *)kVTCompressionPropertyKey_AllowFrameReordering: @NO,
        (__bridge NSString *)kVTCompressionPropertyKey_ProfileLevel: (__bridge NSString *)kVTProfileLevel_H264_Baseline_AutoLevel,
        (__bridge NSString *)kVTCompressionPropertyKey_AverageBitRate: @(width >= 1920 ? 12000000 : 6000000),
        (__bridge NSString *)kVTCompressionPropertyKey_ExpectedFrameRate: @(fps),
        (__bridge NSString *)kVTCompressionPropertyKey_MaxKeyFrameInterval: @(fps)
    };
    code = VTSessionSetProperties(capture->encoder, (__bridge CFDictionaryRef)properties);
    if (!code) code = VTCompressionSessionPrepareToEncodeFrames(capture->encoder);
    if (code) { VTCompressionSessionInvalidate(capture->encoder); CFRelease(capture->encoder); capture->encoder = nullptr; }
    return code;
}

// Drain producers before the Rust callback context can be freed, including when
// startup fails after registering the output. Called off the capture queue.
static void stopCapture(RShareDisplayCapture *capture) {
    if (!capture) return;
    capture->stopping = true;
    if (capture.stream) {
        dispatch_semaphore_t done = dispatch_semaphore_create(0);
        [capture.stream stopCaptureWithCompletionHandler:^(NSError *) { dispatch_semaphore_signal(done); }];
        dispatch_semaphore_wait(done, dispatch_time(DISPATCH_TIME_NOW, 3 * NSEC_PER_SEC));
        [capture.stream removeStreamOutput:capture type:SCStreamOutputTypeScreen error:nil];
    }
    if (capture.queue) dispatch_sync(capture.queue, ^{});
    if (capture->encoder) {
        VTCompressionSessionCompleteFrames(capture->encoder, kCMTimeInvalid);
        VTCompressionSessionInvalidate(capture->encoder);
        CFRelease(capture->encoder); capture->encoder = nullptr;
    }
    capture.stream = nil;
}

extern "C" void *rshare_mac_capture_start(unsigned displayID, unsigned width, unsigned height, unsigned fps,
    RShareFrameSink sink, void *context, char *error, size_t errorSize) {
    @autoreleasepool {
    RShareDisplayCapture *capture = nil;
    @try {
        if (!rshare_mac_session_unlocked()) { fail(error, errorSize, @"Unlock the Mac before starting screen capture"); return nullptr; }
        if (!CGPreflightScreenCaptureAccess() && !CGRequestScreenCaptureAccess()) {
            fail(error, errorSize, @"Allow Screen Recording for R-ShareMouse in System Settings, then restart the app"); return nullptr;
        }
        __block SCShareableContent *content = nil; __block NSError *failure = nil;
        dispatch_semaphore_t ready = dispatch_semaphore_create(0);
        [SCShareableContent getShareableContentExcludingDesktopWindows:NO onScreenWindowsOnly:NO completionHandler:^(SCShareableContent *result, NSError *e) {
            content = result; failure = e; dispatch_semaphore_signal(ready);
        }];
        if (dispatch_semaphore_wait(ready, dispatch_time(DISPATCH_TIME_NOW, 5 * NSEC_PER_SEC)) || failure) {
            fail(error, errorSize, failure.localizedDescription ?: @"ScreenCaptureKit enumeration timed out"); return nullptr;
        }
        SCDisplay *target = nil;
        for (SCDisplay *display in content.displays) if (display.displayID == displayID) target = display;
        if (!target) { fail(error, errorSize, @"The virtual screen is not available to ScreenCaptureKit"); return nullptr; }
        capture = [RShareDisplayCapture new];
        capture->sink = sink; capture->context = context; capture->forceKey = true;
        capture.queue = dispatch_queue_create("com.rshare.display.capture", DISPATCH_QUEUE_SERIAL);
        OSStatus code = createEncoder(capture, width, height, fps);
        if (code) { fail(error, errorSize, [NSString stringWithFormat:@"H.264 encoder configuration failed: %d", (int)code]); return nullptr; }
        SCStreamConfiguration *config = [SCStreamConfiguration new];
        config.width = width; config.height = height; config.minimumFrameInterval = CMTimeMake(1, fps);
        config.queueDepth = 3; config.showsCursor = YES; config.pixelFormat = kCVPixelFormatType_32BGRA;
        SCContentFilter *filter = [[SCContentFilter alloc] initWithDisplay:target excludingWindows:@[]];
        capture.stream = [[SCStream alloc] initWithFilter:filter configuration:config delegate:capture];
        NSError *outputError = nil;
        BOOL added = [capture.stream addStreamOutput:capture type:SCStreamOutputTypeScreen sampleHandlerQueue:capture.queue error:&outputError];
        if (added) {
            [capture.stream startCaptureWithCompletionHandler:^(NSError *e) { failure = e; dispatch_semaphore_signal(ready); }];
            if (dispatch_semaphore_wait(ready, dispatch_time(DISPATCH_TIME_NOW, 5 * NSEC_PER_SEC))) failure = [NSError errorWithDomain:@"RShare" code:-1 userInfo:@{NSLocalizedDescriptionKey:@"Screen capture startup timed out"}];
        } else failure = outputError ?: [NSError errorWithDomain:@"RShare" code:-1 userInfo:@{NSLocalizedDescriptionKey:@"Screen capture output registration failed"}];
        if (failure) {
            stopCapture(capture);
            fail(error, errorSize, failure.localizedDescription); return nullptr;
        }
        return (__bridge_retained void *)capture;
    } @catch (NSException *exception) {
        stopCapture(capture); fail(error, errorSize, exception.reason); return nullptr;
    } }
}

extern "C" int rshare_mac_capture_error(void *handle) { return ((__bridge RShareDisplayCapture *)handle)->captureError; }
extern "C" void rshare_mac_capture_stop(void *handle) {
    @autoreleasepool {
        RShareDisplayCapture *capture = CFBridgingRelease(handle);
        stopCapture(capture);
    }
}

// Native codec regression seam: generated pixels, no screen capture or USB.
extern "C" int rshare_mac_encoder_probe(RShareFrameSink sink, void *context) {
    @autoreleasepool {
        RShareDisplayCapture *capture = [RShareDisplayCapture new];
        capture->sink = sink; capture->context = context;
        OSStatus result = createEncoder(capture, 1280, 720, 30);
        if (result) return result;
        CVPixelBufferRef pixels = nullptr;
        result = CVPixelBufferCreate(kCFAllocatorDefault, 1280, 720, kCVPixelFormatType_32BGRA, nullptr, &pixels);
        if (!result) {
            CVPixelBufferLockBaseAddress(pixels, 0);
            for (size_t y = 0; y < 720; y++) {
                uint8_t *row = (uint8_t *)CVPixelBufferGetBaseAddress(pixels) + y * CVPixelBufferGetBytesPerRow(pixels);
                for (size_t x = 0; x < 1280; x++) { row[x*4] = 60; row[x*4+1] = 160; row[x*4+2] = 30; row[x*4+3] = 255; }
            }
            CVPixelBufferUnlockBaseAddress(pixels, 0);
            result = VTCompressionSessionEncodeFrame(capture->encoder, pixels, CMTimeMake(0,30), CMTimeMake(1,30), nullptr, nullptr, nullptr);
            if (!result) result = VTCompressionSessionCompleteFrames(capture->encoder, kCMTimeInvalid);
            CFRelease(pixels);
        }
        VTCompressionSessionInvalidate(capture->encoder); CFRelease(capture->encoder); capture->encoder = nullptr;
        return result ?: capture->captureError.load();
    }
}
